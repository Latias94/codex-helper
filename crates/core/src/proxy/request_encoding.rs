use std::io::{Cursor, Read};

use axum::body::Bytes;
use axum::http::{HeaderMap, header};

const REQUEST_BODY_ENCODING_ENV: &str = "CODEX_HELPER_REQUEST_BODY_ENCODING";
const MAX_DECOMPRESSED_BODY_BYTES: u64 = 64 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestBodyEncodingMode {
    Auto,
    Passthrough,
}

impl RequestBodyEncodingMode {
    fn from_env() -> Self {
        match std::env::var(REQUEST_BODY_ENCODING_ENV)
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("passthrough" | "preserve" | "raw") => Self::Passthrough,
            _ => Self::Auto,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RequestContentEncodingError {
    encoding: String,
    message: String,
}

impl RequestContentEncodingError {
    fn new(encoding: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            encoding: encoding.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RequestContentEncodingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "decode request Content-Encoding {:?} failed: {}",
            self.encoding, self.message
        )
    }
}

impl std::error::Error for RequestContentEncodingError {}

pub(super) fn normalize_request_content_encoding(
    headers: &mut HeaderMap,
    body: Bytes,
) -> Result<Bytes, RequestContentEncodingError> {
    normalize_request_content_encoding_with_mode(headers, body, RequestBodyEncodingMode::from_env())
}

fn normalize_request_content_encoding_with_mode(
    headers: &mut HeaderMap,
    body: Bytes,
    mode: RequestBodyEncodingMode,
) -> Result<Bytes, RequestContentEncodingError> {
    if mode == RequestBodyEncodingMode::Passthrough {
        return Ok(body);
    }

    let mut encodings = Vec::new();
    for value in headers.get_all(header::CONTENT_ENCODING).iter() {
        let value = value.to_str().map_err(|err| {
            RequestContentEncodingError::new("<invalid header value>", err.to_string())
        })?;
        encodings.extend(
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("identity"))
                .map(str::to_ascii_lowercase),
        );
    }
    if encodings.is_empty() {
        return Ok(body);
    }

    let mut decoded = body.to_vec();
    for item in encodings.iter().rev() {
        decoded = decode_one(item, &decoded)?;
    }

    headers.remove(header::CONTENT_ENCODING);
    headers.remove(header::CONTENT_LENGTH);

    Ok(Bytes::from(decoded))
}

fn decode_one(encoding: &str, body: &[u8]) -> Result<Vec<u8>, RequestContentEncodingError> {
    match encoding {
        "zstd" | "zst" => decode_zstd(encoding, body),
        "gzip" | "x-gzip" => decode_gzip(encoding, body),
        "br" => decode_brotli(encoding, body),
        "deflate" => decode_zlib(encoding, body).or_else(|_| decode_deflate(encoding, body)),
        other => Err(RequestContentEncodingError::new(
            other,
            "unsupported request content encoding",
        )),
    }
}

fn read_limited<R: Read>(
    encoding: &str,
    reader: R,
) -> Result<Vec<u8>, RequestContentEncodingError> {
    let mut limited = reader.take(MAX_DECOMPRESSED_BODY_BYTES + 1);
    let mut out = Vec::new();
    limited
        .read_to_end(&mut out)
        .map_err(|err| RequestContentEncodingError::new(encoding, err.to_string()))?;
    if out.len() as u64 > MAX_DECOMPRESSED_BODY_BYTES {
        return Err(RequestContentEncodingError::new(
            encoding,
            "decompressed request body exceeds 64 MiB",
        ));
    }
    Ok(out)
}

fn decode_zstd(encoding: &str, body: &[u8]) -> Result<Vec<u8>, RequestContentEncodingError> {
    let decoder = zstd::stream::read::Decoder::new(Cursor::new(body))
        .map_err(|err| RequestContentEncodingError::new(encoding, err.to_string()))?;
    read_limited(encoding, decoder)
}

fn decode_gzip(encoding: &str, body: &[u8]) -> Result<Vec<u8>, RequestContentEncodingError> {
    read_limited(encoding, flate2::read::GzDecoder::new(Cursor::new(body)))
}

fn decode_brotli(encoding: &str, body: &[u8]) -> Result<Vec<u8>, RequestContentEncodingError> {
    read_limited(encoding, brotli::Decompressor::new(Cursor::new(body), 4096))
}

fn decode_zlib(encoding: &str, body: &[u8]) -> Result<Vec<u8>, RequestContentEncodingError> {
    read_limited(encoding, flate2::read::ZlibDecoder::new(Cursor::new(body)))
}

fn decode_deflate(encoding: &str, body: &[u8]) -> Result<Vec<u8>, RequestContentEncodingError> {
    read_limited(
        encoding,
        flate2::read::DeflateDecoder::new(Cursor::new(body)),
    )
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use axum::http::{HeaderMap, HeaderValue, header};
    use flate2::Compression;
    use flate2::write::{DeflateEncoder, GzEncoder, ZlibEncoder};

    use super::*;

    fn assert_decodes(encoding: &'static str, compressed: Vec<u8>) {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static(encoding));
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("123"));

        let decoded = normalize_request_content_encoding_with_mode(
            &mut headers,
            Bytes::from(compressed),
            RequestBodyEncodingMode::Auto,
        )
        .expect("decode");

        assert_eq!(decoded.as_ref(), br#"{"model":"gpt-5"}"#);
        assert!(!headers.contains_key(header::CONTENT_ENCODING));
        assert!(!headers.contains_key(header::CONTENT_LENGTH));
    }

    #[test]
    fn request_content_encoding_decodes_zstd() {
        let compressed =
            zstd::stream::encode_all(Cursor::new(br#"{"model":"gpt-5"}"#), 0).expect("zstd encode");
        assert_decodes("zstd", compressed);
    }

    #[test]
    fn request_content_encoding_decodes_gzip() {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(br#"{"model":"gpt-5"}"#)
            .expect("gzip write");
        assert_decodes("gzip", encoder.finish().expect("gzip finish"));
    }

    #[test]
    fn request_content_encoding_decodes_brotli() {
        let mut compressed = Vec::new();
        {
            let mut writer = brotli::CompressorWriter::new(&mut compressed, 4096, 5, 22);
            writer
                .write_all(br#"{"model":"gpt-5"}"#)
                .expect("brotli write");
        }
        assert_decodes("br", compressed);
    }

    #[test]
    fn request_content_encoding_decodes_zlib_deflate() {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(br#"{"model":"gpt-5"}"#)
            .expect("zlib write");
        assert_decodes("deflate", encoder.finish().expect("zlib finish"));
    }

    #[test]
    fn request_content_encoding_decodes_raw_deflate() {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(br#"{"model":"gpt-5"}"#)
            .expect("deflate write");
        assert_decodes("deflate", encoder.finish().expect("deflate finish"));
    }

    #[test]
    fn request_content_encoding_decodes_stacked_values() {
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(br#"{"model":"gpt-5"}"#).expect("gzip write");
        let gzip_body = gzip.finish().expect("gzip finish");
        let compressed = zstd::stream::encode_all(Cursor::new(gzip_body), 0).expect("zstd encode");

        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_ENCODING,
            HeaderValue::from_static("gzip, zstd"),
        );
        let decoded = normalize_request_content_encoding_with_mode(
            &mut headers,
            Bytes::from(compressed),
            RequestBodyEncodingMode::Auto,
        )
        .expect("decode");

        assert_eq!(decoded.as_ref(), br#"{"model":"gpt-5"}"#);
        assert!(!headers.contains_key(header::CONTENT_ENCODING));
    }

    #[test]
    fn request_content_encoding_passthrough_env_preserves_body_and_header() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("zstd"));
        let compressed = Bytes::from_static(b"not-json-but-preserved");

        let out = normalize_request_content_encoding_with_mode(
            &mut headers,
            compressed.clone(),
            RequestBodyEncodingMode::Passthrough,
        )
        .expect("passthrough");

        assert_eq!(out, compressed);
        assert_eq!(
            headers.get(header::CONTENT_ENCODING),
            Some(&HeaderValue::from_static("zstd"))
        );
    }
}
