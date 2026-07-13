use std::fs;

use heck::{ToKebabCase, ToLowerCamelCase, ToShoutyKebabCase, ToShoutySnakeCase, ToSnakeCase};
use serde::{Deserialize, Serialize};
use syn::{
    Attribute, Fields, GenericArgument, Item, LitStr, PathArguments, Type, Variant, Visibility,
};

#[derive(Debug, Deserialize)]
pub struct ExtractRequest {
    pub targets: Vec<TargetRequest>,
}

#[derive(Debug, Deserialize)]
pub struct TargetRequest {
    pub id: String,
    pub file: String,
    pub item: String,
    pub kind: TargetKind,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Struct,
    Enum,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ExtractResponse {
    pub targets: Vec<TargetSchema>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct TargetSchema {
    pub id: String,
    pub file: String,
    pub item: String,
    pub kind: TargetKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<FieldSchema>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variants: Option<Vec<String>>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct FieldSchema {
    pub name: String,
    pub optional: bool,
    pub wire_type: WireType,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireType {
    Number,
    String,
    Boolean,
    Reference {
        name: String,
    },
    Array {
        element: Box<WireType>,
    },
    Tuple {
        elements: Vec<WireType>,
    },
    Map {
        key: Box<WireType>,
        value: Box<WireType>,
    },
    Nullable {
        inner: Box<WireType>,
    },
}

#[derive(Debug, Default)]
struct SerdeAttributes {
    rename: Option<String>,
    rename_all: Option<String>,
    skip_serializing: bool,
    skip_serializing_if: bool,
    flatten: bool,
}

pub fn extract_contract_schema(request: ExtractRequest) -> Result<ExtractResponse, String> {
    let targets = request
        .targets
        .into_iter()
        .map(extract_target)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ExtractResponse { targets })
}

fn extract_target(target: TargetRequest) -> Result<TargetSchema, String> {
    let source = fs::read_to_string(&target.file)
        .map_err(|error| format!("failed to read {}: {error}", target.file))?;
    extract_target_from_source(target, &source)
}

fn extract_target_from_source(target: TargetRequest, source: &str) -> Result<TargetSchema, String> {
    let syntax = syn::parse_file(source)
        .map_err(|error| format!("failed to parse {}: {error}", target.file))?;

    match target.kind {
        TargetKind::Struct => {
            let item = syntax
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Struct(item) if item.ident == target.item => Some(item),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let item = only_item(item, &target)?;
            ensure_public(&item.vis, &target)?;
            let container = serde_attributes(&item.attrs, &target)?;
            let fields = extract_struct_fields(&item.fields, &container, &target)?;
            Ok(TargetSchema {
                id: target.id,
                file: target.file,
                item: target.item,
                kind: target.kind,
                fields: Some(fields),
                variants: None,
            })
        }
        TargetKind::Enum => {
            let item = syntax
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Enum(item) if item.ident == target.item => Some(item),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let item = only_item(item, &target)?;
            ensure_public(&item.vis, &target)?;
            let container = serde_attributes(&item.attrs, &target)?;
            let variants = extract_enum_variants(&item.variants, &container, &target)?;
            Ok(TargetSchema {
                id: target.id,
                file: target.file,
                item: target.item,
                kind: target.kind,
                fields: None,
                variants: Some(variants),
            })
        }
    }
}

fn only_item<'a, T>(items: Vec<&'a T>, target: &TargetRequest) -> Result<&'a T, String> {
    match items.as_slice() {
        [item] => Ok(*item),
        [] => Err(format!(
            "could not find public {:?} {} in {}",
            target.kind, target.item, target.file
        )),
        _ => Err(format!(
            "found duplicate {:?} {} declarations in {}",
            target.kind, target.item, target.file
        )),
    }
}

fn ensure_public(visibility: &Visibility, target: &TargetRequest) -> Result<(), String> {
    if matches!(visibility, Visibility::Public(_)) {
        Ok(())
    } else {
        Err(format!(
            "contract {:?} {} in {} is not public",
            target.kind, target.item, target.file
        ))
    }
}

fn extract_struct_fields(
    fields: &Fields,
    container: &SerdeAttributes,
    target: &TargetRequest,
) -> Result<Vec<FieldSchema>, String> {
    let Fields::Named(fields) = fields else {
        return Err(format!(
            "contract struct {} in {} must use named fields",
            target.item, target.file
        ));
    };
    let mut output = Vec::with_capacity(fields.named.len());
    for field in &fields.named {
        let attributes = serde_attributes(&field.attrs, target)?;
        if attributes.flatten {
            return Err(format!(
                "contract field {} in {} uses unsupported serde(flatten)",
                field
                    .ident
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "<unnamed>".to_owned()),
                target.file
            ));
        }
        if attributes.skip_serializing {
            continue;
        }
        let ident = field
            .ident
            .as_ref()
            .ok_or_else(|| format!("unnamed field in {}", target.file))?
            .to_string();
        let name = attributes.rename.unwrap_or_else(|| {
            container
                .rename_all
                .as_deref()
                .map(|rule| rename(&ident, rule))
                .unwrap_or(ident)
        });
        let optional = attributes.skip_serializing_if;
        let mut wire_type = parse_wire_type(&field.ty, target)?;
        if optional && let WireType::Nullable { inner } = wire_type {
            wire_type = *inner;
        }
        output.push(FieldSchema {
            name,
            optional,
            wire_type,
        });
    }
    if output.is_empty() {
        return Err(format!(
            "contract struct {} in {} has no serialized fields",
            target.item, target.file
        ));
    }
    Ok(output)
}

fn extract_enum_variants(
    variants: &syn::punctuated::Punctuated<Variant, syn::token::Comma>,
    container: &SerdeAttributes,
    target: &TargetRequest,
) -> Result<Vec<String>, String> {
    let mut output = Vec::with_capacity(variants.len());
    for variant in variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(format!(
                "contract enum {} in {} has non-unit variant {}",
                target.item, target.file, variant.ident
            ));
        }
        let attributes = serde_attributes(&variant.attrs, target)?;
        if attributes.skip_serializing {
            continue;
        }
        let ident = variant.ident.to_string();
        output.push(attributes.rename.unwrap_or_else(|| {
            container
                .rename_all
                .as_deref()
                .map(|rule| rename(&ident, rule))
                .unwrap_or(ident)
        }));
    }
    if output.is_empty() {
        return Err(format!(
            "contract enum {} in {} has no serialized variants",
            target.item, target.file
        ));
    }
    Ok(output)
}

fn parse_wire_type(ty: &Type, target: &TargetRequest) -> Result<WireType, String> {
    match ty {
        Type::Group(group) => parse_wire_type(&group.elem, target),
        Type::Paren(paren) => parse_wire_type(&paren.elem, target),
        Type::Reference(reference) => parse_wire_type(&reference.elem, target),
        Type::Slice(slice) => Ok(WireType::Array {
            element: Box::new(parse_wire_type(&slice.elem, target)?),
        }),
        Type::Array(array) => Ok(WireType::Array {
            element: Box::new(parse_wire_type(&array.elem, target)?),
        }),
        Type::Tuple(tuple) => Ok(WireType::Tuple {
            elements: tuple
                .elems
                .iter()
                .map(|element| parse_wire_type(element, target))
                .collect::<Result<Vec<_>, _>>()?,
        }),
        Type::Path(path) if path.qself.is_none() => {
            let segment = path
                .path
                .segments
                .last()
                .ok_or_else(|| format!("empty type path in {}:{}", target.file, target.item))?;
            let name = segment.ident.to_string();
            match name.as_str() {
                "Option" => Ok(WireType::Nullable {
                    inner: Box::new(single_type_argument(segment, target)?),
                }),
                "Vec" => Ok(WireType::Array {
                    element: Box::new(single_type_argument(segment, target)?),
                }),
                "Box" => single_type_argument(segment, target),
                "HashMap" | "BTreeMap" => {
                    let (key, value) = two_type_arguments(segment, target)?;
                    Ok(WireType::Map {
                        key: Box::new(key),
                        value: Box::new(value),
                    })
                }
                "String" | "str" => Ok(WireType::String),
                "bool" => Ok(WireType::Boolean),
                primitive if is_numeric_primitive(primitive) => Ok(WireType::Number),
                _ if matches!(segment.arguments, PathArguments::None) => {
                    Ok(WireType::Reference { name })
                }
                _ => Err(format!(
                    "unsupported Rust type for {}:{}: {}",
                    target.file, target.item, name
                )),
            }
        }
        _ => Err(format!(
            "unsupported Rust type syntax in {}:{}",
            target.file, target.item
        )),
    }
}

fn two_type_arguments(
    segment: &syn::PathSegment,
    target: &TargetRequest,
) -> Result<(WireType, WireType), String> {
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(format!(
            "{} in {}:{} requires two type arguments",
            segment.ident, target.file, target.item
        ));
    };
    let types = arguments
        .args
        .iter()
        .filter_map(|argument| match argument {
            GenericArgument::Type(ty) => Some(ty),
            _ => None,
        })
        .collect::<Vec<_>>();
    if types.len() != 2 || arguments.args.len() != 2 {
        return Err(format!(
            "{} in {}:{} must have exactly two type arguments",
            segment.ident, target.file, target.item
        ));
    }
    Ok((
        parse_wire_type(types[0], target)?,
        parse_wire_type(types[1], target)?,
    ))
}

fn single_type_argument(
    segment: &syn::PathSegment,
    target: &TargetRequest,
) -> Result<WireType, String> {
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(format!(
            "{} in {}:{} requires one type argument",
            segment.ident, target.file, target.item
        ));
    };
    let mut types = arguments.args.iter().filter_map(|argument| match argument {
        GenericArgument::Type(ty) => Some(ty),
        _ => None,
    });
    let ty = types.next().ok_or_else(|| {
        format!(
            "{} in {}:{} requires one type argument",
            segment.ident, target.file, target.item
        )
    })?;
    if types.next().is_some() || arguments.args.len() != 1 {
        return Err(format!(
            "{} in {}:{} must have exactly one type argument",
            segment.ident, target.file, target.item
        ));
    }
    parse_wire_type(ty, target)
}

fn serde_attributes(
    attributes: &[Attribute],
    target: &TargetRequest,
) -> Result<SerdeAttributes, String> {
    let mut output = SerdeAttributes::default();
    for attribute in attributes
        .iter()
        .filter(|attribute| attribute.path().is_ident("serde"))
    {
        attribute
            .parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    output.rename = Some(meta.value()?.parse::<LitStr>()?.value());
                } else if meta.path.is_ident("rename_all") {
                    output.rename_all = Some(meta.value()?.parse::<LitStr>()?.value());
                } else if meta.path.is_ident("skip_serializing_if") {
                    let _predicate = meta.value()?.parse::<LitStr>()?;
                    output.skip_serializing_if = true;
                } else if meta.path.is_ident("skip") || meta.path.is_ident("skip_serializing") {
                    output.skip_serializing = true;
                } else if meta.path.is_ident("flatten") {
                    output.flatten = true;
                } else if meta.path.is_ident("default") {
                    if meta.input.peek(syn::Token![=]) {
                        let _default = meta.value()?.parse::<LitStr>()?;
                    }
                } else if meta.path.is_ident("alias") {
                    let _alias = meta.value()?.parse::<LitStr>()?;
                } else if meta.path.is_ident("deny_unknown_fields") {
                    // This changes deserialization only and has no wire-shape effect.
                } else if meta.path.is_ident("other") {
                    // This changes deserialization fallback only; the variant still serializes.
                } else {
                    return Err(meta.error("unsupported serde attribute in desktop contract DTO"));
                }
                Ok(())
            })
            .map_err(|error| {
                format!(
                    "failed to inspect serde attributes for {}:{}: {error}",
                    target.file, target.item
                )
            })?;
    }
    Ok(output)
}

fn rename(value: &str, rule: &str) -> String {
    match rule {
        "lowercase" => value.to_lowercase(),
        "UPPERCASE" => value.to_uppercase(),
        "PascalCase" => heck::AsUpperCamelCase(value).to_string(),
        "camelCase" => value.to_lower_camel_case(),
        "snake_case" => value.to_snake_case(),
        "SCREAMING_SNAKE_CASE" => value.to_shouty_snake_case(),
        "kebab-case" => value.to_kebab_case(),
        "SCREAMING-KEBAB-CASE" => value.to_shouty_kebab_case(),
        _ => value.to_owned(),
    }
}

fn is_numeric_primitive(value: &str) -> bool {
    matches!(
        value,
        "u8" | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "f32"
            | "f64"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(item: &str, kind: TargetKind) -> TargetRequest {
        TargetRequest {
            id: "target".to_owned(),
            file: "fixture.rs".to_owned(),
            item: item.to_owned(),
            kind,
        }
    }

    #[test]
    fn parses_multiline_struct_from_rust_ast_and_serde_wire_rules() {
        let source = r#"
            // pub struct Wire { pub forged: String }
            #[derive(serde::Serialize)]
            #[serde(rename_all = "camelCase")]
            pub struct Wire {
                pub request_id: u64,
                #[serde(default, skip_serializing_if = "Option::is_none")]
                pub nested_value: Option<
                    Vec<String>
                >,
                pub required_value: Option<String>,
            }
        "#;

        let schema = extract_target_from_source(request("Wire", TargetKind::Struct), source)
            .expect("parse structured Rust contract");
        assert_eq!(
            schema.fields,
            Some(vec![
                FieldSchema {
                    name: "requestId".to_owned(),
                    optional: false,
                    wire_type: WireType::Number,
                },
                FieldSchema {
                    name: "nestedValue".to_owned(),
                    optional: true,
                    wire_type: WireType::Array {
                        element: Box::new(WireType::String),
                    },
                },
                FieldSchema {
                    name: "requiredValue".to_owned(),
                    optional: false,
                    wire_type: WireType::Nullable {
                        inner: Box::new(WireType::String),
                    },
                },
            ])
        );
    }

    #[test]
    fn parses_enum_rename_all_and_explicit_variant_rename() {
        let source = r#"
            #[serde(rename_all = "snake_case")]
            pub enum Status {
                ReadyNow,
                #[serde(rename = "offline")]
                Disconnected,
            }
        "#;

        let schema = extract_target_from_source(request("Status", TargetKind::Enum), source)
            .expect("parse enum contract");
        assert_eq!(
            schema.variants,
            Some(vec!["ready_now".to_owned(), "offline".to_owned()])
        );
    }

    #[test]
    fn rejects_serde_flatten_in_contract_structs() {
        let source = r#"
            pub struct Wire {
                #[serde(flatten)]
                pub nested: Nested,
            }
        "#;

        let error = extract_target_from_source(request("Wire", TargetKind::Struct), source)
            .expect_err("flatten must fail closed");
        assert!(error.contains("serde(flatten)"));
    }
}
