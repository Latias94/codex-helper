pub(super) fn extract_reasoning_effort_from_request_body(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    value
        .get("reasoning")
        .and_then(|reasoning| reasoning.get("effort"))
        .and_then(|effort| effort.as_str())
        .map(ToOwned::to_owned)
}

pub(super) fn extract_model_from_request_body(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    value
        .get("model")
        .and_then(|model| model.as_str())
        .map(ToOwned::to_owned)
}

pub(super) fn extract_service_tier_from_request_body(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    value
        .get("service_tier")
        .and_then(|service_tier| service_tier.as_str())
        .map(ToOwned::to_owned)
}

fn extract_service_tier_from_response_value(value: &serde_json::Value) -> Option<String> {
    value
        .get("service_tier")
        .and_then(|service_tier| service_tier.as_str())
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("service_tier"))
                .and_then(|service_tier| service_tier.as_str())
        })
        .map(ToOwned::to_owned)
}

pub(super) fn extract_service_tier_from_response_body(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    extract_service_tier_from_response_value(&value)
}

pub(super) fn scan_service_tier_from_sse_bytes_incremental(
    data: &[u8],
    scan_pos: &mut usize,
    last: &mut Option<String>,
) {
    let mut i = (*scan_pos).min(data.len());

    while i < data.len() {
        let Some(rel_end) = data[i..].iter().position(|b| *b == b'\n') else {
            break;
        };
        let end = i + rel_end;
        let mut line = &data[i..end];
        i = end.saturating_add(1);

        if line.ends_with(b"\r") {
            line = &line[..line.len().saturating_sub(1)];
        }
        if line.is_empty() {
            continue;
        }

        const DATA_PREFIX: &[u8] = b"data:";
        if !line.starts_with(DATA_PREFIX) {
            continue;
        }
        let mut payload = &line[DATA_PREFIX.len()..];
        while !payload.is_empty() && payload[0].is_ascii_whitespace() {
            payload = &payload[1..];
        }
        if payload.is_empty() || payload == b"[DONE]" {
            continue;
        }

        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(payload)
            && let Some(service_tier) = extract_service_tier_from_response_value(&value)
        {
            *last = Some(service_tier);
        }
    }

    *scan_pos = i;
}

pub(super) fn apply_reasoning_effort_override(body: &[u8], effort: &str) -> Option<Vec<u8>> {
    let mut value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let reasoning = value
        .get_mut("reasoning")
        .and_then(|reasoning| reasoning.as_object_mut());
    if let Some(object) = reasoning {
        object.insert(
            "effort".to_string(),
            serde_json::Value::String(effort.to_string()),
        );
    } else {
        let mut object = serde_json::Map::new();
        object.insert(
            "effort".to_string(),
            serde_json::Value::String(effort.to_string()),
        );
        value
            .as_object_mut()?
            .insert("reasoning".to_string(), serde_json::Value::Object(object));
    }
    serde_json::to_vec(&value).ok()
}

pub(super) fn apply_model_override(body: &[u8], model: &str) -> Option<Vec<u8>> {
    let mut value: serde_json::Value = serde_json::from_slice(body).ok()?;
    value.as_object_mut()?.insert(
        "model".to_string(),
        serde_json::Value::String(model.to_string()),
    );
    serde_json::to_vec(&value).ok()
}

pub(super) fn apply_service_tier_override(body: &[u8], service_tier: &str) -> Option<Vec<u8>> {
    let mut value: serde_json::Value = serde_json::from_slice(body).ok()?;
    value.as_object_mut()?.insert(
        "service_tier".to_string(),
        serde_json::Value::String(service_tier.to_string()),
    );
    serde_json::to_vec(&value).ok()
}

#[cfg(test)]
mod tests {
    use super::{
        apply_model_override, apply_reasoning_effort_override, apply_service_tier_override,
        extract_model_from_request_body, extract_reasoning_effort_from_request_body,
        extract_service_tier_from_request_body, extract_service_tier_from_response_body,
        scan_service_tier_from_sse_bytes_incremental,
    };

    #[test]
    fn extracts_request_fields() {
        let body = br#"{
            "model":"gpt-5",
            "service_tier":"priority",
            "reasoning":{"effort":"high"}
        }"#;

        assert_eq!(
            extract_reasoning_effort_from_request_body(body).as_deref(),
            Some("high")
        );
        assert_eq!(
            extract_model_from_request_body(body).as_deref(),
            Some("gpt-5")
        );
        assert_eq!(
            extract_service_tier_from_request_body(body).as_deref(),
            Some("priority")
        );
    }

    #[test]
    fn applies_request_overrides() {
        let body = br#"{"input":"hello"}"#;

        let reasoning =
            apply_reasoning_effort_override(body, "medium").expect("reasoning override");
        let model = apply_model_override(&reasoning, "gpt-5.4").expect("model override");
        let service_tier =
            apply_service_tier_override(&model, "flex").expect("service tier override");

        assert_eq!(
            extract_reasoning_effort_from_request_body(&service_tier).as_deref(),
            Some("medium")
        );
        assert_eq!(
            extract_model_from_request_body(&service_tier).as_deref(),
            Some("gpt-5.4")
        );
        assert_eq!(
            extract_service_tier_from_request_body(&service_tier).as_deref(),
            Some("flex")
        );
    }

    #[test]
    fn extracts_service_tier_from_response_shapes() {
        assert_eq!(
            extract_service_tier_from_response_body(br#"{"service_tier":"priority"}"#).as_deref(),
            Some("priority")
        );
        assert_eq!(
            extract_service_tier_from_response_body(br#"{"response":{"service_tier":"flex"}}"#)
                .as_deref(),
            Some("flex")
        );
    }

    #[test]
    fn scans_service_tier_from_sse_incrementally() {
        let chunk1 = b"data: {\"response\":{\"service_tier\":\"priority\"}}\n";
        let chunk2 = b"data: {\"service_tier\":\"flex\"}\n\ndata: [DONE]\n";
        let mut scan_pos = 0;
        let mut last = None;
        let mut data = Vec::new();

        data.extend_from_slice(chunk1);
        scan_service_tier_from_sse_bytes_incremental(&data, &mut scan_pos, &mut last);
        assert_eq!(last.as_deref(), Some("priority"));

        data.extend_from_slice(chunk2);
        scan_service_tier_from_sse_bytes_incremental(&data, &mut scan_pos, &mut last);
        assert_eq!(last.as_deref(), Some("flex"));
    }
}
