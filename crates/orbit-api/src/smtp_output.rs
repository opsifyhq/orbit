//! SMTP output: validate the non-secret settings and send one plain-text message.
//!
//! Credentials live in the encrypted secret store. The SMTP client always validates the
//! server certificate; custom roots come from the same CA bundle variables as the HTTP
//! clients (`SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE`, `NODE_EXTRA_CA_CERTS`).

use std::path::Path;

use lettre::message::{header::ContentType, Mailbox};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Certificate, Tls, TlsParameters};
use lettre::{Message, SmtpTransport, Transport};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Security {
    /// Plain SMTP with mandatory STARTTLS.
    Starttls,
    /// SMTPS: TLS from the first byte.
    Tls,
    /// Plaintext. Only for an explicitly trusted local relay.
    None,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub host: String,
    #[serde(default)]
    pub port: Option<u16>,
    pub from: String,
    pub to: OneOrMany,
    #[serde(default)]
    pub cc: Option<OneOrMany>,
    #[serde(default)]
    pub bcc: Option<OneOrMany>,
    #[serde(default)]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default = "default_security")]
    pub security: Security,
}

fn default_security() -> Security {
    Security::Starttls
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(value) => vec![value],
            Self::Many(values) => values,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub from: Mailbox,
    pub to: Vec<Mailbox>,
    pub cc: Vec<Mailbox>,
    pub bcc: Vec<Mailbox>,
    pub reply_to: Option<Mailbox>,
    pub subject: String,
    pub security: Security,
}

impl Settings {
    pub fn parse(config: &serde_json::Value) -> Result<Self, String> {
        let raw: Config = serde_json::from_value(config.clone())
            .map_err(|error| format!("invalid SMTP config: {error}"))?;
        let host = raw.host.trim().to_string();
        if host.is_empty() || host.chars().any(char::is_whitespace) {
            return Err("host must be a hostname without spaces".into());
        }
        let from = parse_mailbox(&raw.from, "from")?;
        let mut to = parse_mailboxes(raw.to, "to")?;
        if to.is_empty() {
            return Err("at least one recipient is required".into());
        }
        let cc = raw
            .cc
            .map(|v| parse_mailboxes(v, "cc"))
            .transpose()?
            .unwrap_or_default();
        let bcc = raw
            .bcc
            .map(|v| parse_mailboxes(v, "bcc"))
            .transpose()?
            .unwrap_or_default();
        let reply_to = raw
            .reply_to
            .as_deref()
            .map(|value| parse_mailbox(value, "replyTo"))
            .transpose()?;
        to.extend(cc.iter().cloned());
        to.extend(bcc.iter().cloned());
        let port = raw.port.unwrap_or(match raw.security {
            Security::Tls => 465,
            Security::Starttls => 587,
            Security::None => 25,
        });
        if port == 0 {
            return Err("port must be between 1 and 65535".into());
        }
        let subject = raw
            .subject
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("ORBIT notification")
            .to_string();
        if subject.contains(['\r', '\n']) {
            return Err("subject must not contain line breaks".into());
        }
        Ok(Self {
            host,
            port,
            from,
            to,
            cc,
            bcc,
            reply_to,
            subject,
            security: raw.security,
        })
    }
}

fn parse_mailbox(value: &str, field: &str) -> Result<Mailbox, String> {
    value
        .trim()
        .parse()
        .map_err(|_| format!("{field} must be a valid email address"))
}

fn parse_mailboxes(value: OneOrMany, field: &str) -> Result<Vec<Mailbox>, String> {
    value
        .into_vec()
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .map(|value| parse_mailbox(&value, field))
        .collect()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credential {
    pub username: String,
    pub password: String,
}

impl Credential {
    pub fn parse(secret: &str) -> Result<Self, String> {
        let credential: Self = serde_json::from_str(secret)
            .map_err(|_| "SMTP credential must be JSON with username and password".to_string())?;
        if credential.username.trim().is_empty() || credential.password.is_empty() {
            return Err("SMTP username and password must not be empty".into());
        }
        Ok(credential)
    }
}

#[derive(Debug)]
pub struct SendError {
    pub message: String,
    pub retryable: bool,
}

pub fn send(settings: &Settings, credential: &Credential, text: &str) -> Result<(), SendError> {
    let mut builder = Message::builder()
        .from(settings.from.clone())
        .subject(settings.subject.clone());
    for mailbox in &settings.to {
        builder = builder.to(mailbox.clone());
    }
    for mailbox in &settings.cc {
        builder = builder.cc(mailbox.clone());
    }
    for mailbox in &settings.bcc {
        builder = builder.bcc(mailbox.clone());
    }
    if let Some(reply_to) = &settings.reply_to {
        builder = builder.reply_to(reply_to.clone());
    }
    let email = builder
        .header(ContentType::TEXT_PLAIN)
        .body(text.to_string())
        .map_err(|error| SendError {
            message: format!("could not build the email: {error}"),
            retryable: false,
        })?;

    let mut transport = SmtpTransport::builder_dangerous(settings.host.clone())
        .port(settings.port)
        .credentials(Credentials::new(
            credential.username.clone(),
            credential.password.clone(),
        ));
    if settings.security != Security::None {
        let parameters = tls_parameters(&settings.host).map_err(|message| SendError {
            message,
            retryable: false,
        })?;
        transport = transport.tls(match settings.security {
            Security::Starttls => Tls::Required(parameters),
            Security::Tls => Tls::Wrapper(parameters),
            Security::None => unreachable!(),
        });
    }
    transport.build().send(&email).map(|_| ()).map_err(|error| {
        let retryable = !(error.is_permanent() || error.is_client() || error.is_tls());
        let status = error
            .status()
            .map(|code| format!(" (SMTP {})", code))
            .unwrap_or_default();
        SendError {
            message: format!("SMTP delivery failed{status}: {error}"),
            retryable,
        }
    })
}

fn tls_parameters(host: &str) -> Result<TlsParameters, String> {
    let mut builder = TlsParameters::builder(host.to_string());
    for path in ca_bundle_paths() {
        let Ok(pem) = std::fs::read(&path) else {
            continue;
        };
        for block in certificate_blocks(&pem) {
            if let Ok(certificate) = Certificate::from_pem(&block) {
                builder = builder.add_root_certificate(certificate);
            }
        }
    }
    builder
        .build()
        .map_err(|error| format!("could not build SMTP TLS parameters: {error}"))
}

fn ca_bundle_paths() -> Vec<std::path::PathBuf> {
    let mut seen = std::collections::BTreeSet::new();
    ["SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", "NODE_EXTRA_CA_CERTS"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && seen.insert(value.clone()))
        .map(std::path::PathBuf::from)
        .collect()
}

fn certificate_blocks(pem: &[u8]) -> Vec<Vec<u8>> {
    const BEGIN: &[u8] = b"-----BEGIN CERTIFICATE-----";
    const END: &[u8] = b"-----END CERTIFICATE-----";
    let mut blocks = Vec::new();
    let mut cursor = 0;
    while let Some(start) = find(&pem[cursor..], BEGIN) {
        let start = cursor + start;
        let Some(end) = find(&pem[start..], END) else {
            break;
        };
        let end = start + end + END.len();
        blocks.push(pem[start..end].to_vec());
        cursor = end;
    }
    blocks
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Validate an SMTP config at save time without connecting.
pub fn validate_config(config: &serde_json::Value) -> Result<(), String> {
    Settings::parse(config).map(|_| ())
}

/// Validate a write-only SMTP credential at save time.
pub fn validate_credential(secret: &str) -> Result<(), String> {
    Credential::parse(secret).map(|_| ())
}

#[allow(dead_code)]
fn _path_marker(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    fn settings(port: u16) -> Settings {
        Settings::parse(&serde_json::json!({
            "host": "127.0.0.1",
            "port": port,
            "security": "none",
            "from": "orbit@example.test",
            "to": ["ops@example.test"],
            "cc": ["lead@example.test"],
            "subject": "Run finished"
        }))
        .unwrap()
    }

    #[test]
    fn config_requires_recipients_and_valid_mailboxes() {
        let missing = Settings::parse(&serde_json::json!({
            "host": "mail.example.test",
            "from": "orbit@example.test",
            "to": []
        }));
        assert!(missing.unwrap_err().contains("recipient"));
        let bad = Settings::parse(&serde_json::json!({
            "host": "mail.example.test",
            "from": "not-an-address",
            "to": ["ops@example.test"]
        }));
        assert!(bad.unwrap_err().contains("from"));
        let ok = Settings::parse(&serde_json::json!({
            "host": "mail.example.test",
            "from": "orbit@example.test",
            "to": "ops@example.test",
            "security": "starttls"
        }))
        .unwrap();
        assert_eq!(ok.port, 587);
        assert_eq!(ok.to.len(), 1);
    }

    #[test]
    fn credential_is_json_and_requires_both_fields() {
        assert!(Credential::parse(r#"{"username":"u","password":"p"}"#).is_ok());
        assert!(Credential::parse(r#"{"username":"","password":"p"}"#).is_err());
        assert!(Credential::parse("u:p").is_err());
    }

    #[test]
    fn sends_one_plain_text_message() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let received = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let received_for_thread = received.clone();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            stream.write_all(b"220 test ESMTP\r\n").unwrap();
            let mut body = String::new();
            let mut in_data = false;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                if in_data {
                    if line == ".\r\n" {
                        in_data = false;
                        *received_for_thread.lock().unwrap() = body.clone();
                        stream.write_all(b"250 queued\r\n").unwrap();
                    } else {
                        body.push_str(&line);
                    }
                    continue;
                }
                let command = line.trim_end().to_ascii_uppercase();
                if command.starts_with("EHLO") || command.starts_with("HELO") {
                    stream
                        .write_all(b"250-test\r\n250-AUTH PLAIN LOGIN\r\n250 OK\r\n")
                        .unwrap();
                } else if command.starts_with("AUTH") {
                    stream
                        .write_all(b"235 authentication successful\r\n")
                        .unwrap();
                } else if command.starts_with("MAIL FROM") || command.starts_with("RCPT TO") {
                    stream.write_all(b"250 OK\r\n").unwrap();
                } else if command.starts_with("DATA") {
                    in_data = true;
                    stream.write_all(b"354 send data\r\n").unwrap();
                } else if command.starts_with("QUIT") {
                    stream.write_all(b"221 bye\r\n").unwrap();
                    break;
                } else {
                    stream.write_all(b"250 OK\r\n").unwrap();
                }
            }
            *received_for_thread.lock().unwrap() = body;
        });

        let credential = Credential {
            username: "orbit".into(),
            password: "secret".into(),
        };
        send(&settings(port), &credential, "result text").unwrap();
        let message = received.lock().unwrap().clone();
        assert!(message.contains("Subject: Run finished"));
        assert!(message.contains("result text"));
        assert!(message.contains("ops@example.test"));
        assert!(message.contains("lead@example.test"));
    }
}
