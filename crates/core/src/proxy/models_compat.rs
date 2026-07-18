use std::io::Read;

use axum::body::Bytes;
use axum::http::{HeaderMap, header};
use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};

const MAX_DECODED_MODELS_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn codex_path_is_models(path: &str) -> bool {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .is_some_and(|segment| segment == "models")
}

pub(super) fn maybe_decode_models_response_body(
    service_name: &str,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Bytes {
    if service_name != "codex" || !codex_path_is_models(path) {
        return body;
    }

    if looks_like_json(body.as_ref()) {
        body
    } else if let Some(decoded) = decode_from_content_encoding(headers, body.as_ref())
        .or_else(|| decode_from_signature(body.as_ref()))
    {
        Bytes::from(decoded)
    } else {
        body
    }
}

fn decode_from_content_encoding(headers: &HeaderMap, body: &[u8]) -> Option<Vec<u8>> {
    let mut encodings = Vec::new();
    for value in headers.get_all(header::CONTENT_ENCODING).iter() {
        let Ok(value) = value.to_str() else {
            continue;
        };
        encodings.extend(
            value
                .split(',')
                .map(|part| part.trim().to_ascii_lowercase())
                .filter(|part| !part.is_empty() && part != "identity"),
        );
    }
    if encodings.is_empty() {
        return None;
    }

    let mut decoded = body.to_vec();
    for encoding in encodings.iter().rev() {
        decoded = decode_one(encoding, &decoded)?;
    }
    looks_like_json(&decoded).then_some(decoded)
}

fn decode_from_signature(body: &[u8]) -> Option<Vec<u8>> {
    if body.starts_with(&[0x1f, 0x8b]) {
        return decode_gzip(body);
    }
    if body.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        return decode_zstd(body);
    }

    // Brotli and raw deflate do not have a reliable magic prefix. Only accept a
    // decoded result when it is JSON, which keeps this fallback scoped to /models.
    decode_brotli(body)
        .or_else(|| decode_zlib(body))
        .or_else(|| decode_deflate(body))
}

fn decode_one(encoding: &str, body: &[u8]) -> Option<Vec<u8>> {
    match encoding {
        "gzip" | "x-gzip" => decode_gzip(body),
        "br" => decode_brotli(body),
        "zstd" | "zst" => decode_zstd(body),
        "deflate" => decode_zlib(body).or_else(|| decode_deflate(body)),
        _ => None,
    }
}

fn decode_gzip(body: &[u8]) -> Option<Vec<u8>> {
    read_jsonish(GzDecoder::new(body))
}

fn decode_brotli(body: &[u8]) -> Option<Vec<u8>> {
    read_jsonish(brotli::Decompressor::new(body, 4096))
}

fn decode_zstd(body: &[u8]) -> Option<Vec<u8>> {
    read_jsonish(zstd::stream::read::Decoder::new(body).ok()?)
}

fn decode_zlib(body: &[u8]) -> Option<Vec<u8>> {
    read_jsonish(ZlibDecoder::new(body))
}

fn decode_deflate(body: &[u8]) -> Option<Vec<u8>> {
    read_jsonish(DeflateDecoder::new(body))
}

fn read_jsonish<R: Read>(reader: R) -> Option<Vec<u8>> {
    let mut limited = reader.take((MAX_DECODED_MODELS_BYTES + 1) as u64);
    let mut out = Vec::new();
    limited.read_to_end(&mut out).ok()?;
    if out.len() > MAX_DECODED_MODELS_BYTES || !looks_like_json(&out) {
        return None;
    }
    Some(out)
}

fn looks_like_json(bytes: &[u8]) -> bool {
    let Some(first) = bytes.iter().find(|byte| !byte.is_ascii_whitespace()) else {
        return false;
    };
    matches!(first, b'{' | b'[')
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn models_response_preserves_openai_data_list_shape() {
        let body = Bytes::from_static(
            br#"{
                "object": "list",
                "data": [{ "id": "gpt-5.6-sol", "object": "model" }]
            }"#,
        );

        let decoded =
            maybe_decode_models_response_body("codex", "/models", &HeaderMap::new(), body.clone());

        assert_eq!(decoded, body);
    }

    #[test]
    fn models_response_preserves_codex_catalog_byte_for_byte() {
        let body = Bytes::from_static(
            br#"{
                "models": [{
                    "slug": "gpt-5.6-sol",
                    "supports_reasoning_summaries": true,
                    "input_modalities": ["text", "image"]
                }]
            }"#,
        );

        let decoded =
            maybe_decode_models_response_body("codex", "/models", &HeaderMap::new(), body.clone());

        assert_eq!(decoded, body);
    }

    #[test]
    fn codex_models_path_matches_supported_prefixes_and_trailing_slashes() {
        for path in [
            "/models",
            "/models/",
            "/v1/models",
            "/backend-api/codex/models/",
        ] {
            assert!(codex_path_is_models(path), "{path}");
        }
        for path in ["/model", "/models/item", "/v1/responses", "/"] {
            assert!(!codex_path_is_models(path), "{path}");
        }
    }

    #[test]
    fn models_response_decodes_declared_gzip() {
        let raw = br#"{"object":"list","data":[{"id":"gpt-5.6-sol"}]}"#;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, raw).expect("write gzip body");
        let compressed = encoder.finish().expect("finish gzip body");
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));

        let decoded = maybe_decode_models_response_body(
            "codex",
            "/models",
            &headers,
            Bytes::from(compressed),
        );

        assert_eq!(decoded.as_ref(), raw);
    }
}
