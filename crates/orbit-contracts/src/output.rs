//! Contracts for execution result outputs shared by the API, delivery adapters and UI.
//!
//! These types contain no HTTP or SQL details. In particular, credentials are never part
//! of a contract value: the API stores them as encrypted `secret_ref` handles and resolves
//! them only at send time.

use uuid::Uuid;

/// Destinations supported by the execution output outbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    Slack,
    Mattermost,
    Webhook,
    Telegram,
}

impl OutputKind {
    pub const ALL: &'static [&'static str] = &["slack", "mattermost", "webhook", "telegram"];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Slack => "slack",
            Self::Mattermost => "mattermost",
            Self::Webhook => "webhook",
            Self::Telegram => "telegram",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "slack" => Some(Self::Slack),
            "mattermost" => Some(Self::Mattermost),
            "webhook" => Some(Self::Webhook),
            "telegram" => Some(Self::Telegram),
            _ => None,
        }
    }
}

/// Terminal execution statuses an output may subscribe to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputEvent {
    Succeeded,
    Failed,
    Cancelled,
}

impl OutputEvent {
    pub const ALL: &'static [&'static str] = &["succeeded", "failed", "cancelled"];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// Telegram Bot API text limit per `sendMessage` call.
pub const TELEGRAM_TEXT_LIMIT: usize = 4096;

/// Non-secret Telegram destination. The bot token is deliberately absent; it is resolved
/// from the encrypted secret store by the delivery adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramTarget {
    pub chat_id: String,
    pub message_thread_id: Option<String>,
}

impl TelegramTarget {
    /// Validate and normalize a Telegram destination. Chat ids must be numeric because
    /// `@channel` usernames are not accepted by the Bot API `sendMessage` endpoint.
    pub fn parse(chat_id: &str, message_thread_id: Option<&str>) -> Result<Self, &'static str> {
        let chat_id = chat_id.trim();
        if !is_i64(chat_id) || chat_id.parse::<i64>().is_ok_and(|id| id == 0) {
            return Err("chatId must be a numeric Telegram chat id (for example -100…)");
        }
        let message_thread_id = message_thread_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                if !is_i64(value) || value.parse::<i64>().is_ok_and(|id| id <= 0) {
                    Err("messageThreadId must be a positive numeric Telegram topic id")
                } else {
                    Ok(value.to_string())
                }
            })
            .transpose()?;
        Ok(Self {
            chat_id: chat_id.to_string(),
            message_thread_id,
        })
    }
}

fn is_i64(value: &str) -> bool {
    let digits = value.strip_prefix('-').unwrap_or(value);
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) && value.parse::<i64>().is_ok()
}

/// Status codes worth retrying. Other 4xx responses indicate a bad target or payload.
pub fn output_status_is_retryable(status: u16) -> bool {
    status >= 500 || status == 408 || status == 429
}

/// Remove a credential from any user-facing text before it can reach an error response or
/// delivery log. Telegram error descriptions never contain the token, but defense in depth
/// is cheap here.
pub fn redact_secret(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        return text.to_string();
    }
    text.replace(secret, "[redacted]")
}

/// A normalized send failure for the output outbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSendError {
    pub message: String,
    pub retryable: bool,
}

/// Map a Telegram Bot API error into an actionable, token-free message.
pub fn telegram_error(status: u16, description: Option<&str>, token: &str) -> OutputSendError {
    let description = redact_secret(description.unwrap_or("").trim(), token);
    let suffix = if description.is_empty() {
        String::new()
    } else {
        format!(": {description}")
    };
    let (message, retryable) = match status {
        401 | 403 => (
            format!("telegram bot token or chat access invalid{suffix}"),
            false,
        ),
        400 if is_chat_access_error(&description) => (
            format!(
                "telegram chat is not reachable{suffix}. Add the bot to the target group/channel; \
                 for channels make it an administrator and use the numeric chat id (for example -100…)."
            ),
            false,
        ),
        400 => (format!("telegram rejected the message{suffix}"), false),
        _ => (
            format!("telegram returned HTTP {status}{suffix}"),
            output_status_is_retryable(status),
        ),
    };
    OutputSendError { message, retryable }
}

fn is_chat_access_error(description: &str) -> bool {
    let description = description.to_ascii_lowercase();
    [
        "chat not found",
        "bot is not a member",
        "not enough rights",
        "have no rights",
        "can't send",
        "migrate",
        "channel",
        "supergroup",
        "group chat",
    ]
    .iter()
    .any(|hint| description.contains(hint))
}

/// Split long output into Telegram-sized messages. When a delivery id is known, every part
/// gets a diagnostic footer so a receiver can correlate retries and multiple parts.
pub fn telegram_message_parts(text: &str, delivery_id: Option<Uuid>) -> Vec<String> {
    let Some(id) = delivery_id else {
        return split_chunks(text, TELEGRAM_TEXT_LIMIT);
    };
    // Reserve room for the footer, then shrink the body budget if an unusually long
    // delivery id/part count would otherwise exceed the limit.
    let mut budget = TELEGRAM_TEXT_LIMIT - 64;
    loop {
        let chunks = split_chunks(text, budget);
        let total = chunks.len();
        let parts: Vec<String> = chunks
            .into_iter()
            .enumerate()
            .map(|(index, chunk)| {
                format!(
                    "{chunk}\n\n[ORBIT delivery {id} — part {}/{}]",
                    index + 1,
                    total
                )
            })
            .collect();
        if parts
            .iter()
            .all(|part| part.chars().count() <= TELEGRAM_TEXT_LIMIT)
        {
            return parts;
        }
        budget -= 16;
        if budget == 0 {
            return parts;
        }
    }
}

/// Split on a line boundary where possible so messages stay readable.
fn split_chunks(text: &str, budget: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    let mut parts = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = (start + budget).min(chars.len());
        if end < chars.len() {
            if let Some(offset) = chars[start..end].iter().rposition(|c| *c == '\n') {
                end = start + offset + 1;
            }
        }
        parts.push(chars[start..end].iter().collect());
        start = end;
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_kinds_and_events_round_trip() {
        for kind in OutputKind::ALL {
            assert_eq!(OutputKind::parse(kind).map(OutputKind::as_str), Some(*kind));
        }
        assert_eq!(OutputKind::parse("signal"), None);
        for event in OutputEvent::ALL {
            assert_eq!(
                OutputEvent::parse(event).map(OutputEvent::as_str),
                Some(*event)
            );
        }
        assert_eq!(OutputEvent::parse("started"), None);
    }

    #[test]
    fn telegram_target_requires_numeric_ids() {
        let target = TelegramTarget::parse(" -1001234567890 ", Some(" 42 ")).unwrap();
        assert_eq!(target.chat_id, "-1001234567890");
        assert_eq!(target.message_thread_id.as_deref(), Some("42"));
        assert!(TelegramTarget::parse("@channel", None).is_err());
        assert!(TelegramTarget::parse("123", Some("topic")).is_err());
        assert!(TelegramTarget::parse("", None).is_err());
        assert!(TelegramTarget::parse("-0", None).is_err());
    }

    #[test]
    fn long_text_is_split_below_the_telegram_limit() {
        let text = "line of output\n".repeat(1000);
        let parts = telegram_message_parts(&text, Some(Uuid::now_v7()));
        assert!(parts.len() > 1);
        assert!(parts
            .iter()
            .all(|p| p.chars().count() <= TELEGRAM_TEXT_LIMIT));
        assert!(parts.iter().all(|p| p.contains("[ORBIT delivery ")));
        assert!(parts.last().unwrap().contains("part "));
    }

    #[test]
    fn short_text_gets_a_single_diagnostic_footer() {
        let id = Uuid::now_v7();
        let parts = telegram_message_parts("all good", Some(id));
        assert_eq!(parts.len(), 1);
        assert_eq!(
            parts[0],
            format!("all good\n\n[ORBIT delivery {id} — part 1/1]")
        );
        assert_eq!(telegram_message_parts("all good", None), vec!["all good"]);
    }

    #[test]
    fn telegram_errors_are_actionable_and_token_free() {
        let token = "123456:super-secret-token";
        let auth = telegram_error(401, Some("Unauthorized"), token);
        assert!(!auth.retryable);
        assert_eq!(
            auth.message,
            "telegram bot token or chat access invalid: Unauthorized"
        );

        let not_found = telegram_error(400, Some("Chat not found"), token);
        assert!(!not_found.retryable);
        assert!(not_found
            .message
            .contains("Add the bot to the target group/channel"));

        let server = telegram_error(503, Some("try later"), token);
        assert!(server.retryable);

        let leaked = telegram_error(400, Some("bad token 123456:super-secret-token"), token);
        assert!(!leaked.message.contains(token));
        assert!(leaked.message.contains("[redacted]"));
    }
}
