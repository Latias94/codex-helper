use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use sha2::{Digest, Sha256};

const CONTENT_MD5: &str = "content-md5";
const DIGEST: &str = "digest";
const CONTENT_DIGEST: &str = "content-digest";
const REPR_DIGEST: &str = "repr-digest";

/// Builds an upstream client that leaves response entity transformations to the relay.
pub fn upstream_http_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_gzip()
        .no_brotli()
        .no_zstd()
        .no_deflate()
}

/// Captures the upstream entity before any relay-owned transformation.
pub(super) struct UpstreamResponseEntity {
    status: StatusCode,
    body: Bytes,
}

impl UpstreamResponseEntity {
    pub(super) fn capture(status: StatusCode, body: &Bytes) -> Self {
        Self {
            status,
            body: body.clone(),
        }
    }

    /// Reconciles byte-dependent headers after all buffered transformations.
    pub(super) fn reconcile_headers(
        &self,
        emitted_status: StatusCode,
        emitted_body: &Bytes,
        headers: &mut HeaderMap,
    ) {
        let same_body = self.body.len() == emitted_body.len()
            && (self.body.as_ptr() == emitted_body.as_ptr() || self.body == *emitted_body);
        if self.status == emitted_status && same_body {
            return;
        }

        for name in [
            header::ETAG.as_str(),
            CONTENT_MD5,
            DIGEST,
            CONTENT_DIGEST,
            REPR_DIGEST,
            header::CONTENT_LENGTH.as_str(),
            header::CONTENT_ENCODING.as_str(),
            header::CONTENT_RANGE.as_str(),
            header::ACCEPT_RANGES.as_str(),
            header::LAST_MODIFIED.as_str(),
        ] {
            headers.remove(name);
        }

        headers.insert(header::ETAG, relay_strong_etag(emitted_body));
    }
}

fn relay_strong_etag(body: &[u8]) -> HeaderValue {
    let digest = Sha256::digest(body);
    let value = format!("\"sha256-{digest:x}\"");
    HeaderValue::try_from(value).expect("SHA-256 always produces a valid ETag header value")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transformed_entity_replaces_byte_dependent_headers() {
        let original = Bytes::from_static(b"compressed");
        let emitted = Bytes::from_static(b"decoded");
        let entity = UpstreamResponseEntity::capture(StatusCode::OK, &original);
        let mut headers = HeaderMap::new();
        headers.insert(header::ETAG, HeaderValue::from_static("\"upstream\""));
        headers.insert(CONTENT_MD5, HeaderValue::from_static("stale-md5"));
        headers.insert(DIGEST, HeaderValue::from_static("sha-256=stale"));
        headers.insert(CONTENT_DIGEST, HeaderValue::from_static("sha-256=:stale:"));
        headers.insert(REPR_DIGEST, HeaderValue::from_static("sha-256=:stale:"));
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("10"));
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_static("bytes 0-9/10"),
        );
        headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        headers.insert(
            header::LAST_MODIFIED,
            HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"),
        );

        entity.reconcile_headers(StatusCode::OK, &emitted, &mut headers);

        assert_eq!(
            headers.get(header::ETAG),
            Some(&HeaderValue::from_static(
                "\"sha256-174985e4dd3b2df2a316acc62b781b8712734467cc92f94ddccc6530acb14f1e\""
            ))
        );
        assert!(!headers.contains_key(CONTENT_MD5));
        assert!(!headers.contains_key(DIGEST));
        assert!(!headers.contains_key(CONTENT_DIGEST));
        assert!(!headers.contains_key(REPR_DIGEST));
        assert!(!headers.contains_key(header::CONTENT_LENGTH));
        assert!(!headers.contains_key(header::CONTENT_ENCODING));
        assert!(!headers.contains_key(header::CONTENT_RANGE));
        assert!(!headers.contains_key(header::ACCEPT_RANGES));
        assert!(!headers.contains_key(header::LAST_MODIFIED));
    }

    #[test]
    fn unchanged_entity_preserves_upstream_etag() {
        let body = Bytes::from_static(b"unchanged");
        let emitted = Bytes::copy_from_slice(body.as_ref());
        let entity = UpstreamResponseEntity::capture(StatusCode::OK, &body);
        let mut headers = HeaderMap::new();
        headers.insert(header::ETAG, HeaderValue::from_static("\"upstream\""));

        entity.reconcile_headers(StatusCode::OK, &emitted, &mut headers);

        assert_eq!(
            headers.get(header::ETAG),
            Some(&HeaderValue::from_static("\"upstream\""))
        );
    }

    #[test]
    fn status_change_replaces_upstream_etag_even_when_body_is_unchanged() {
        let body = Bytes::from_static(b"same bytes");
        let entity = UpstreamResponseEntity::capture(StatusCode::OK, &body);
        let mut headers = HeaderMap::new();
        headers.insert(header::ETAG, HeaderValue::from_static("\"upstream\""));

        entity.reconcile_headers(StatusCode::BAD_GATEWAY, &body, &mut headers);

        assert_ne!(
            headers.get(header::ETAG),
            Some(&HeaderValue::from_static("\"upstream\""))
        );
    }
}
