use std::io::{Cursor, Read};

use axum::body::Bytes;
use axum::http::{HeaderMap, header};
use flate2::read::GzDecoder;

const MAX_REPAIRED_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

pub(super) fn maybe_repair_codex_response_body(
    service_name: &str,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Bytes {
    if service_name != "codex" || !is_codex_responses_path(path) || looks_like_json(body.as_ref()) {
        return body;
    }

    if response_content_encoding_contains(headers, "gzip") || body.starts_with(&[0x1f, 0x8b]) {
        return decode_gzip_json(body.as_ref())
            .map(Bytes::from)
            .unwrap_or(body);
    }

    body
}

fn is_codex_responses_path(path: &str) -> bool {
    path.ends_with("/responses") || path.ends_with("/responses/compact")
}

fn response_content_encoding_contains(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get_all(header::CONTENT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|encoding| encoding.trim().eq_ignore_ascii_case(expected))
}

fn decode_gzip_json(body: &[u8]) -> Option<Vec<u8>> {
    let mut limited =
        GzDecoder::new(Cursor::new(body)).take((MAX_REPAIRED_RESPONSE_BYTES + 1) as u64);
    let mut out = Vec::new();
    limited.read_to_end(&mut out).ok()?;
    if out.len() > MAX_REPAIRED_RESPONSE_BYTES || !looks_like_json(&out) {
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
    use std::io::Write;

    use axum::http::{HeaderMap, HeaderValue, header};
    use flate2::Compression;
    use flate2::write::GzEncoder;

    use super::*;

    fn gzip_json(body: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(body).expect("gzip write");
        encoder.finish().expect("gzip finish")
    }

    #[test]
    fn response_fixer_decodes_codex_responses_gzip_by_header() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        let body = Bytes::from(gzip_json(br#"{"ok":true}"#));

        let repaired = maybe_repair_codex_response_body("codex", "/v1/responses", &headers, body);

        assert_eq!(repaired.as_ref(), br#"{"ok":true}"#);
    }

    #[test]
    fn response_fixer_decodes_codex_responses_gzip_by_signature() {
        let headers = HeaderMap::new();
        let body = Bytes::from(gzip_json(br#"{"ok":true}"#));

        let repaired = maybe_repair_codex_response_body("codex", "/v1/responses", &headers, body);

        assert_eq!(repaired.as_ref(), br#"{"ok":true}"#);
    }

    #[test]
    fn response_fixer_leaves_non_json_or_non_codex_bodies_untouched() {
        let headers = HeaderMap::new();
        let body = Bytes::from(gzip_json(b"plain text"));

        let repaired =
            maybe_repair_codex_response_body("codex", "/v1/responses", &headers, body.clone());
        assert_eq!(repaired, body);

        let repaired =
            maybe_repair_codex_response_body("claude", "/v1/messages", &headers, body.clone());
        assert_eq!(repaired, body);
    }
}
