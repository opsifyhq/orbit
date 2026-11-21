//! X/Twitter `posts.create` client invoked only through the ORBIT tool broker.
//!
//! `posts.search` deliberately stays a read-only pre-execution hydration step in
//! [`crate::connectors`]; it is not a broker tool. Posting needs a **user-context** token
//! with `tweet.write` (OAuth 2.0 user context or OAuth 1.0a user context). An app-only
//! bearer token is enough for recent search, but X rejects it for `POST /tweets`; the
//! credential on the connection is treated as a user access token and is never logged.

use axum::http::{header, StatusCode};
use serde_json::{json, Value};

const MAX_POST_LENGTH: usize = 280;
const URL_WEIGHT: usize = 23;
const DEFAULT_BASE_URL: &str = "https://api.x.com/2";

const AUTH_HINT: &str = "X posting requires a user-context access token with tweet.write \
     (OAuth 2.0 user context or OAuth 1.0a user context); an app-only bearer token can search \
     but cannot post. Store the user access token as the connection credential.";

#[derive(Clone, Debug)]
pub struct XClientConfig {
    pub base_url: String,
    /// The connection credential, treated as a user access token. Never logged or returned.
    pub token: Option<String>,
}

impl XClientConfig {
    pub fn from_connection(config: &Value, credential: Option<String>) -> Self {
        Self {
            base_url: config
                .get("baseUrl")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_BASE_URL)
                .trim_end_matches('/')
                .to_string(),
            token: credential.filter(|token| !token.trim().is_empty()),
        }
    }
}

pub async fn invoke(
    http: &reqwest::Client,
    config: &XClientConfig,
    capability: &str,
    args: &Value,
) -> Result<Value, String> {
    if capability != "posts.create" {
        return Err(format!(
            "unsupported X capability '{capability}'; posts.search is read-only context hydration, not a broker tool"
        ));
    }
    let token = config.token.as_deref().ok_or_else(|| auth_error(None))?;
    let text = post_text(args)?;
    let reply_to = reply_target(args)?;

    let mut payload = json!({ "text": text.clone() });
    if let Some(id) = reply_to {
        payload["reply"] = json!({ "in_reply_to_tweet_id": id });
    }

    let response = http
        .post(format!("{}/tweets", config.base_url))
        .bearer_auth(token)
        .header(header::USER_AGENT, "orbit-agent-platform")
        .json(&payload)
        .send()
        .await
        .map_err(|error| {
            format!(
                "X request failed: {}",
                orbit_security::describe_request_error(&error)
            )
        })?;

    let status = response.status();
    let body: Value = response.json().await.unwrap_or_else(|_| json!({}));
    if !status.is_success() {
        if is_auth_error(status, &body) {
            return Err(auth_error(Some(status)));
        }
        return Err(format!(
            "X API returned {status}: {}.",
            redacted_detail(&body, token)
        ));
    }

    let data = body.get("data");
    let id = data
        .and_then(|post| post.get("id"))
        .and_then(Value::as_str)
        .ok_or_else(|| "X API response did not contain a post id".to_string())?;
    Ok(json!({
        "id": id,
        "text": data.and_then(|post| post.get("text")).and_then(Value::as_str).unwrap_or(&text),
        "url": format!("https://x.com/i/web/status/{id}"),
    }))
}

fn auth_error(status: Option<StatusCode>) -> String {
    match status {
        Some(status) => format!("X API returned {status}. {AUTH_HINT}"),
        None => AUTH_HINT.to_string(),
    }
}

fn is_auth_error(status: StatusCode, body: &Value) -> bool {
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return true;
    }
    let detail = body
        .get("detail")
        .or_else(|| body.get("title"))
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    [
        "invalid token",
        "unauthorized",
        "app-only",
        "app only",
        "could not authenticate",
        "insufficient_scope",
        "insufficient scope",
        "bad authentication",
    ]
    .iter()
    .any(|needle| detail.contains(*needle))
}

fn redacted_detail(body: &Value, token: &str) -> String {
    let raw = body
        .get("detail")
        .or_else(|| body.get("title"))
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("X API error");
    let redacted = if token.is_empty() {
        raw.to_string()
    } else {
        raw.replace(token, "[redacted]")
    };
    truncate(&redacted, 200)
}

fn truncate(value: &str, limit: usize) -> String {
    let mut end = limit.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut result = value[..end].to_string();
    if end < value.len() {
        result.push('…');
    }
    result
}

fn post_text(args: &Value) -> Result<String, String> {
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| "post text is required".to_string())?;
    if text.trim().is_empty() {
        return Err("post text must not be empty".to_string());
    }
    let length = weighted_length(text);
    if length > MAX_POST_LENGTH {
        return Err(format!(
            "X post text is {length} weighted characters; the limit is {MAX_POST_LENGTH}"
        ));
    }
    Ok(text.to_string())
}

/// X counts most characters as one, URLs as 23 (t.co wrapping), and CJK/emoji as two.
fn weighted_length(text: &str) -> usize {
    let words = text
        .split_whitespace()
        .map(|word| {
            if word.starts_with("http://") || word.starts_with("https://") {
                URL_WEIGHT
            } else {
                word.chars()
                    .map(|character| usize::from(is_double_width(character)) + 1)
                    .sum()
            }
        })
        .sum::<usize>();
    words
        + text
            .chars()
            .filter(|character| character.is_whitespace())
            .count()
}

fn is_double_width(character: char) -> bool {
    matches!(character as u32,
        0x1100..=0x115F
        | 0x2E80..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1FAFF
        | 0x20000..=0x3FFFD
    )
}

fn reply_target(args: &Value) -> Result<Option<String>, String> {
    let direct = as_tweet_id(args.get("replyTo"));
    let nested = args
        .get("reply")
        .and_then(|reply| as_tweet_id(reply.get("in_reply_to_tweet_id")));
    match (direct, nested) {
        (Some(direct), Some(nested)) if direct != nested => Err(
            "replyTo and reply.in_reply_to_tweet_id must match when both are provided".to_string(),
        ),
        (Some(id), _) | (_, Some(id)) => Ok(Some(validate_tweet_id(&id)?)),
        (None, None) => Ok(None),
    }
}

fn as_tweet_id(value: Option<&Value>) -> Option<String> {
    value.and_then(|value| {
        value
            .as_str()
            .map(str::to_string)
            .or_else(|| value.as_u64().map(|number| number.to_string()))
    })
}

fn validate_tweet_id(id: &str) -> Result<String, String> {
    let id = id.trim();
    if id.is_empty() || id.len() > 19 || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("reply target must be a numeric X tweet ID".to_string());
    }
    Ok(id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_to_the_x_api_and_keeps_user_token() {
        let config = XClientConfig::from_connection(&json!({}), Some("user-token".into()));
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        assert_eq!(config.token.as_deref(), Some("user-token"));
        let custom =
            XClientConfig::from_connection(&json!({ "baseUrl": "https://x.example/2/" }), None);
        assert_eq!(custom.base_url, "https://x.example/2");
        assert!(custom.token.is_none());
    }

    #[test]
    fn text_validation_uses_x_weighted_length() {
        assert_eq!(weighted_length("hello world"), 11);
        assert_eq!(weighted_length("hello\tworld"), 11);
        assert_eq!(
            weighted_length("https://example.com/a/very/long/path"),
            URL_WEIGHT
        );
        assert_eq!(weighted_length("📣"), 2);
        assert_eq!(weighted_length("日本語"), 6);
        assert!(post_text(&json!({ "text": "x".repeat(280) })).is_ok());
        assert!(post_text(&json!({ "text": "x".repeat(281) })).is_err());
        // X wraps URLs to 23 weighted characters, however long the raw URL is.
        assert!(post_text(&json!({
            "text": format!("https://example.com/{}", "x".repeat(280))
        }))
        .is_ok());
        assert!(post_text(&json!({
            "text": format!("{} https://example.com", "x".repeat(280))
        }))
        .is_err());
        assert!(post_text(&json!({})).is_err());
        assert!(post_text(&json!({ "text": "   " })).is_err());
    }

    #[test]
    fn reply_accepts_both_shapes_and_rejects_conflicts() {
        assert_eq!(
            reply_target(&json!({ "replyTo": "123" })).unwrap(),
            Some("123".into())
        );
        assert_eq!(
            reply_target(&json!({ "reply": { "in_reply_to_tweet_id": 456 } })).unwrap(),
            Some("456".into())
        );
        assert!(reply_target(&json!({
            "replyTo": "123",
            "reply": { "in_reply_to_tweet_id": "456" }
        }))
        .is_err());
        assert!(reply_target(&json!({ "replyTo": "not-an-id" })).is_err());
    }

    #[test]
    fn auth_errors_are_actionable_and_never_include_the_credential() {
        let message = auth_error(Some(StatusCode::FORBIDDEN));
        assert!(message.contains("403"));
        assert!(message.contains("user-context"));
        assert!(message.contains("tweet.write"));
        assert!(message.contains("app-only"));
        assert!(is_auth_error(
            StatusCode::FORBIDDEN,
            &json!({ "detail": "Forbidden" })
        ));
        assert!(is_auth_error(
            StatusCode::BAD_REQUEST,
            &json!({ "detail": "Invalid token" })
        ));
        assert!(!is_auth_error(
            StatusCode::TOO_MANY_REQUESTS,
            &json!({ "detail": "Too many requests" })
        ));
        let detail = redacted_detail(
            &json!({ "detail": "token super-secret-value rejected" }),
            "super-secret-value",
        );
        assert!(!detail.contains("super-secret-value"));
        assert!(detail.contains("[redacted]"));
    }

    /// Opt-in live check against the real X API. It posts a public tweet, so it is ignored
    /// by default. See docs/guide/x-connector.md for the required user-context setup.
    #[tokio::test]
    #[ignore = "posts a real tweet; requires ORBIT_X_LIVE=1 and a user-context token"]
    async fn live_post_requires_user_context_token() {
        if std::env::var("ORBIT_X_LIVE").ok().as_deref() != Some("1") {
            return;
        }
        let Ok(token) = std::env::var("ORBIT_X_USER_TOKEN") else {
            panic!("set ORBIT_X_USER_TOKEN to a user-context access token with tweet.write");
        };
        let config = XClientConfig::from_connection(&json!({}), Some(token));
        let http = orbit_security::guarded_client(std::time::Duration::from_secs(20));
        let text = format!(
            "ORBIT X connector live check {}",
            chrono::Utc::now().timestamp()
        );
        let result = invoke(&http, &config, "posts.create", &json!({ "text": text }))
            .await
            .expect("user-context token can post");
        assert!(result["id"].as_str().is_some_and(|id| !id.is_empty()));
    }
}
