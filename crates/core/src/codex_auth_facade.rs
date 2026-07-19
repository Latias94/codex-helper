use base64::Engine as _;
use thiserror::Error;

use crate::config::CodexAuthFacadeStrategy;

#[derive(Debug, Error)]
pub(crate) enum CodexAuthFacadeError {
    #[error("Codex auth.json is required; run `codex login` first, then enable chatgpt-bridge")]
    MissingChatGptLogin,
    #[error("Codex auth.json is not valid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("Codex auth.json root must be a JSON object")]
    InvalidRoot,
    #[error("Codex auth.json tokens.id_token is not a valid JWT: {0}")]
    InvalidIdToken(String),
    #[error(
        "Codex auth.json does not contain a complete ChatGPT login state required for chatgpt-bridge (missing: {missing}). Open Codex and sign in with ChatGPT first, then enable chatgpt-bridge again"
    )]
    IncompleteChatGptLogin { missing: String },
}

pub(crate) fn render_auth_facade(
    strategy: CodexAuthFacadeStrategy,
    original: Option<&str>,
) -> Result<Option<String>, CodexAuthFacadeError> {
    match strategy {
        CodexAuthFacadeStrategy::Preserve => Ok(None),
        CodexAuthFacadeStrategy::EmptyChatGpt => {
            serde_json::to_string_pretty(&serde_json::json!({}))
                .map(Some)
                .map_err(CodexAuthFacadeError::from)
        }
        CodexAuthFacadeStrategy::ChatGpt => {
            let original = original.ok_or(CodexAuthFacadeError::MissingChatGptLogin)?;
            let mut value = serde_json::from_str::<serde_json::Value>(original)?;
            validate_chatgpt_login(&value)?;
            let object = value
                .as_object_mut()
                .ok_or(CodexAuthFacadeError::InvalidRoot)?;
            object.insert(
                "auth_mode".to_string(),
                serde_json::Value::String("chatgpt".to_string()),
            );
            object.insert("OPENAI_API_KEY".to_string(), serde_json::Value::Null);
            serde_json::to_string_pretty(&value)
                .map(Some)
                .map_err(CodexAuthFacadeError::from)
        }
    }
}

fn validate_chatgpt_login(value: &serde_json::Value) -> Result<(), CodexAuthFacadeError> {
    let object = value.as_object().ok_or(CodexAuthFacadeError::InvalidRoot)?;
    let tokens = object.get("tokens").and_then(serde_json::Value::as_object);
    let mut missing = Vec::new();

    let id_token = tokens
        .and_then(|tokens| tokens.get("id_token"))
        .and_then(non_empty_json_string);
    let id_token_payload = match id_token {
        Some(token) => Some(decode_jwt_payload(token)?),
        None => {
            missing.push("tokens.id_token");
            None
        }
    };

    for (key, label) in [
        ("access_token", "tokens.access_token"),
        ("refresh_token", "tokens.refresh_token"),
    ] {
        if tokens
            .and_then(|tokens| tokens.get(key))
            .and_then(non_empty_json_string)
            .is_none()
        {
            missing.push(label);
        }
    }
    if object
        .get("last_refresh")
        .is_none_or(serde_json::Value::is_null)
    {
        missing.push("last_refresh");
    }

    if let Some(payload) = id_token_payload.as_ref() {
        let has_email = json_string_at_path(payload, &["email"])
            .or_else(|| json_string_at_path(payload, &["https://api.openai.com/profile", "email"]))
            .is_some();
        if !has_email {
            missing.push("tokens.id_token.email");
        }

        let has_account_id = tokens
            .and_then(|tokens| tokens.get("account_id"))
            .and_then(non_empty_json_string)
            .is_some()
            || json_string_at_path(
                payload,
                &["https://api.openai.com/auth", "chatgpt_account_id"],
            )
            .is_some();
        if !has_account_id {
            missing.push("tokens.account_id or tokens.id_token.chatgpt_account_id");
        }
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(CodexAuthFacadeError::IncompleteChatGptLogin {
            missing: missing.join(", "),
        })
    }
}

fn non_empty_json_string(value: &serde_json::Value) -> Option<&str> {
    value.as_str().filter(|text| !text.trim().is_empty())
}

fn json_string_at_path<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a str> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
        .and_then(non_empty_json_string)
}

fn decode_jwt_payload(token: &str) -> Result<serde_json::Value, CodexAuthFacadeError> {
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(signature)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(CodexAuthFacadeError::InvalidIdToken(
            "invalid JWT format".to_string(),
        ));
    };
    if header.is_empty() || payload.is_empty() || signature.is_empty() {
        return Err(CodexAuthFacadeError::InvalidIdToken(
            "invalid JWT format".to_string(),
        ));
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload))
        .map_err(|error| CodexAuthFacadeError::InvalidIdToken(error.to_string()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| CodexAuthFacadeError::InvalidIdToken(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_auth() -> String {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({
                "email": "user@example.com",
                "https://api.openai.com/auth": { "chatgpt_account_id": "acct_1" }
            }))
            .expect("serialize JWT payload"),
        );
        serde_json::json!({
            "auth_mode": "apikey",
            "OPENAI_API_KEY": "secret",
            "tokens": {
                "id_token": format!("header.{payload}.signature"),
                "access_token": "access",
                "refresh_token": "refresh"
            },
            "last_refresh": "2026-07-19T00:00:00Z"
        })
        .to_string()
    }

    #[test]
    fn chatgpt_facade_preserves_tokens_and_nulls_api_key() {
        let rendered = render_auth_facade(CodexAuthFacadeStrategy::ChatGpt, Some(&valid_auth()))
            .expect("render ChatGPT facade")
            .expect("facade text");
        let value = serde_json::from_str::<serde_json::Value>(&rendered).expect("parse facade");
        assert_eq!(value["tokens"]["access_token"], "access");
        assert_eq!(value["auth_mode"], "chatgpt");
        assert!(value["OPENAI_API_KEY"].is_null());
    }

    #[test]
    fn empty_facade_does_not_require_an_existing_auth_file() {
        let rendered = render_auth_facade(CodexAuthFacadeStrategy::EmptyChatGpt, None)
            .expect("render empty facade")
            .expect("facade text");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&rendered).expect("parse facade"),
            serde_json::json!({})
        );
    }
}
