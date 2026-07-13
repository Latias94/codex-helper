use serde_json::Value;

pub(crate) enum DecodedSseData {
    Missing,
    Done,
    Json(Value),
    Invalid,
}

pub(crate) struct DecodedSseEvent {
    pub event_type: Option<String>,
    pub data: DecodedSseData,
}

pub(crate) fn find_sse_event_end(
    bytes: &[u8],
    start: usize,
    inspected_bytes: &mut usize,
) -> Option<usize> {
    for index in start.min(bytes.len())..bytes.len() {
        *inspected_bytes = inspected_bytes.saturating_add(1);
        let Some(first_len) = sse_line_ending_len(bytes, index) else {
            continue;
        };
        let second_start = index + first_len;
        if let Some(second_len) = sse_line_ending_len(bytes, second_start) {
            return Some(second_start + second_len);
        }
    }
    None
}

pub(crate) fn decode_sse_event(event: &[u8]) -> DecodedSseEvent {
    let mut event_type = None;
    let mut data = Vec::new();
    let mut has_data = false;

    for line in event.split(|byte| matches!(byte, b'\n' | b'\r')) {
        let (field, value) = line
            .iter()
            .position(|byte| *byte == b':')
            .map(|index| (&line[..index], &line[index + 1..]))
            .unwrap_or((line, &[]));
        let value = value.strip_prefix(b" ").unwrap_or(value);
        if field == b"event" {
            event_type = Some(String::from_utf8_lossy(trim_ascii(value)).into_owned());
        } else if field == b"data" {
            if has_data {
                data.push(b'\n');
            }
            data.extend_from_slice(value);
            has_data = true;
        }
    }

    let data = if !has_data {
        DecodedSseData::Missing
    } else {
        let data = trim_ascii(&data);
        if data == b"[DONE]" {
            DecodedSseData::Done
        } else {
            serde_json::from_slice(data)
                .map(DecodedSseData::Json)
                .unwrap_or(DecodedSseData::Invalid)
        }
    };
    DecodedSseEvent { event_type, data }
}

pub(crate) fn visit_sse_json_values(data: &[u8], mut visit: impl FnMut(&Value)) {
    let mut event_start = 0;
    let mut search_pos = 0;
    let mut inspected_bytes = 0;

    while let Some(event_end) = find_sse_event_end(data, search_pos, &mut inspected_bytes) {
        if let DecodedSseData::Json(value) = decode_sse_event(&data[event_start..event_end]).data {
            visit(&value);
        }
        event_start = event_end;
        search_pos = event_end;
    }
    if event_start < data.len()
        && let DecodedSseData::Json(value) = decode_sse_event(&data[event_start..]).data
    {
        visit(&value);
    }
}

fn sse_line_ending_len(bytes: &[u8], index: usize) -> Option<usize> {
    match bytes.get(index) {
        Some(b'\n') => Some(1),
        Some(b'\r') if bytes.get(index + 1) == Some(&b'\n') => Some(2),
        Some(b'\r') if bytes.get(index + 1).is_some() => Some(1),
        _ => None,
    }
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visits_multiline_json_and_an_unterminated_final_event() {
        let sse = concat!(
            "data: {\"type\":\"first\",\n",
            "data: \"value\":1}\r\n\r\n",
            "data: {\"type\":\"second\",\"value\":2}",
        );
        let mut values = Vec::new();

        visit_sse_json_values(sse.as_bytes(), |value| values.push(value.clone()));

        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["value"], 1);
        assert_eq!(values[1]["value"], 2);
    }
}
