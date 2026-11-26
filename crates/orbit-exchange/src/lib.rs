//! On-prem Exchange EWS read client for ORBIT connectors.
//!
//! Supports the common on-prem credential shape: EWS URL plus domain, username and
//! password over HTTPS. Basic authentication is implemented first; NTLM/Kerberos need a
//! dedicated handshake and are reported as unsupported rather than attempted insecurely.

use base64::Engine;
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Deserialize;
use serde_json::{json, Value};

const TYPES_NS: &str = "http://schemas.microsoft.com/exchange/services/2006/types";
const SOAP_NS: &str = "http://schemas.xmlsoap.org/soap/envelope/";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub ews_url: String,
    pub mailbox: String,
    #[serde(default)]
    pub allowed_folders: Vec<String>,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default = "default_max_attachment_bytes")]
    pub max_attachment_bytes: usize,
    /// Exact addresses, `*@domain` and `*` (explicitly any recipient). Empty denies sends.
    #[serde(default)]
    pub allowed_recipients: Vec<String>,
}

fn default_max_results() -> usize {
    25
}
fn default_max_attachment_bytes() -> usize {
    10 * 1024 * 1024
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub domain: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExchangeError {
    #[error("{0}")]
    Config(String),
    #[error("{0}")]
    Auth(String),
    #[error("{0}")]
    Transport(String),
    #[error("{0}")]
    Ews(String),
}

#[derive(Clone)]
pub struct ExchangeClient {
    config: Config,
    base64_auth: String,
    http: reqwest::Client,
}

impl ExchangeClient {
    pub fn from_connection(config: &Value, credential: Option<String>) -> Result<Self, String> {
        Self::from_connection_with_policy(
            config,
            credential,
            orbit_security::OutboundPolicy::from_env(),
        )
    }

    pub fn from_connection_with_policy(
        config: &Value,
        credential: Option<String>,
        policy: orbit_security::OutboundPolicy,
    ) -> Result<Self, String> {
        let config: Config = serde_json::from_value(config.clone())
            .map_err(|error| format!("invalid Exchange config: {error}"))?;
        let parsed_url = reqwest::Url::parse(config.ews_url.trim())
            .map_err(|_| "ewsUrl must be a valid HTTPS URL".to_string())?;
        if parsed_url.scheme() != "https" {
            return Err("ewsUrl must use HTTPS".into());
        }
        if config.mailbox.trim().is_empty() {
            return Err("mailbox is required".into());
        }
        let credential = credential.ok_or("Exchange username/password credential is missing")?;
        let credentials: Credentials = serde_json::from_str(&credential).map_err(|_| {
            "Exchange credential must be JSON with username and password".to_string()
        })?;
        if credentials.username.trim().is_empty() || credentials.password.is_empty() {
            return Err("Exchange username and password must not be empty".into());
        }
        let user = match credentials
            .domain
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
        {
            Some(domain)
                if !credentials.username.contains('\\') && !credentials.username.contains('@') =>
            {
                format!("{domain}\\{}", credentials.username)
            }
            _ => credentials.username,
        };
        let base64_auth = base64::engine::general_purpose::STANDARD
            .encode(format!("{user}:{}", credentials.password));
        Ok(Self {
            config,
            base64_auth,
            http: orbit_security::guarded_client_with_policy(
                std::time::Duration::from_secs(30),
                policy,
            ),
        })
    }

    pub fn allowed_folders(&self) -> Vec<String> {
        let folders: Vec<String> = self
            .config
            .allowed_folders
            .iter()
            .map(|folder| folder.trim().to_ascii_lowercase())
            .filter(|folder| !folder.is_empty())
            .collect();
        if folders.is_empty() {
            vec!["inbox".into()]
        } else {
            folders
        }
    }

    /// Whether a recipient is inside the configured send allowlist. `*` means every
    /// recipient is allowed; an empty list denies every send.
    pub fn recipient_allowed(&self, address: &str) -> bool {
        let address = address.trim().to_ascii_lowercase();
        if address.is_empty() {
            return false;
        }
        let Some((local, domain)) = address.rsplit_once('@') else {
            return false;
        };
        if local.is_empty() || domain.is_empty() {
            return false;
        }
        self.config.allowed_recipients.iter().any(|rule| {
            let rule = rule.trim().to_ascii_lowercase();
            if rule == "*" {
                return true;
            }
            if let Some(domain_rule) = rule.strip_prefix("*@") {
                return domain == domain_rule;
            }
            rule == address
        })
    }

    pub async fn test(&self) -> Result<(), ExchangeError> {
        self.call(&find_item_body("inbox", None, 1, &self.config.mailbox))
            .await?;
        Ok(())
    }

    pub async fn invoke(&self, capability: &str, args: &Value) -> Result<Value, String> {
        match capability {
            "mail.search" => self.search(args).await.map_err(|e| e.to_string()),
            "mail.get" => self.get(args).await.map_err(|e| e.to_string()),
            "mail.attachments.get" => self.attachment(args).await.map_err(|e| e.to_string()),
            "mail.send" => self.send(args).await.map_err(|e| e.to_string()),
            other => Err(format!("unsupported Exchange capability '{other}'")),
        }
    }

    async fn send(&self, args: &Value) -> Result<Value, ExchangeError> {
        let to = recipients(args, "to")?;
        let cc = recipients_optional(args, "cc")?;
        let bcc = recipients_optional(args, "bcc")?;
        for address in to.iter().chain(cc.iter()).chain(bcc.iter()) {
            if !self.recipient_allowed(address) {
                return Err(ExchangeError::Config(format!(
                    "recipient '{address}' is not in allowedRecipients"
                )));
            }
        }
        let subject = args
            .get("subject")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ExchangeError::Config("subject is required".into()))?;
        let body = args
            .get("body")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ExchangeError::Config("body is required".into()))?;
        let xml = self
            .call(&create_message_body(&to, &cc, &bcc, subject, body))
            .await?;
        let id = parse_sent_item_id(&xml).ok_or_else(|| {
            let detail =
                parse_ews_error(&xml).unwrap_or_else(|| "EWS did not confirm the send".into());
            ExchangeError::Ews(detail)
        })?;
        Ok(json!({
            "id": id,
            "to": to,
            "cc": cc,
            "bcc": bcc,
            "subject": subject,
        }))
    }

    async fn search(&self, args: &Value) -> Result<Value, ExchangeError> {
        let folder = args
            .get("folder")
            .and_then(Value::as_str)
            .unwrap_or("inbox")
            .to_ascii_lowercase();
        if !self.allowed_folders().contains(&folder) {
            return Err(ExchangeError::Config(format!(
                "folder '{folder}' is not in allowedFolders"
            )));
        }
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|q| !q.is_empty());
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(25)
            .min(self.config.max_results.max(1));
        let xml = self
            .call(&find_item_body(&folder, query, limit, &self.config.mailbox))
            .await?;
        Ok(parse_find_items(&xml, limit))
    }

    async fn get(&self, args: &Value) -> Result<Value, ExchangeError> {
        let item_id = required(args, "itemId")?;
        let xml = self.call(&get_item_body(item_id)).await?;
        parse_get_item(&xml)
    }

    async fn attachment(&self, args: &Value) -> Result<Value, ExchangeError> {
        let attachment_id = required(args, "attachmentId")?;
        let xml = self.call(&get_attachment_body(attachment_id)).await?;
        let attachment = parse_attachment(&xml)?;
        let size = attachment.content.len();
        if size > self.config.max_attachment_bytes {
            return Err(ExchangeError::Config(format!(
                "attachment is {size} bytes; maxAttachmentBytes is {}",
                self.config.max_attachment_bytes
            )));
        }
        Ok(json!({
            "name": attachment.name,
            "contentType": attachment.content_type,
            "size": size,
            "contentBase64": base64::engine::general_purpose::STANDARD.encode(attachment.content),
        }))
    }

    async fn call(&self, body: &str) -> Result<String, ExchangeError> {
        orbit_security::check_outbound_url(
            &self.config.ews_url,
            &orbit_security::OutboundPolicy::from_env(),
        )
        .await
        .map_err(|error| ExchangeError::Config(format!("EWS URL refused: {error}")))?;
        let envelope = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="{SOAP_NS}"><s:Header><t:RequestServerVersion xmlns:t="{TYPES_NS}" Version="Exchange2013"/></s:Header><s:Body>{body}</s:Body></s:Envelope>"#
        );
        let response = self
            .http
            .post(&self.config.ews_url)
            .header("Authorization", format!("Basic {}", self.base64_auth))
            .header("Content-Type", "text/xml; charset=utf-8")
            .header("Accept", "text/xml")
            .body(envelope)
            .send()
            .await
            .map_err(|error| {
                ExchangeError::Transport(orbit_security::describe_request_error(&error))
            })?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ExchangeError::Auth(
                "EWS authentication failed; check the domain, username and password (Basic auth must be enabled)"
                    .into(),
            ));
        }
        if !status.is_success() {
            return Err(ExchangeError::Transport(format!(
                "EWS returned HTTP {status}"
            )));
        }
        Ok(text)
    }
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str, ExchangeError> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ExchangeError::Config(format!("{key} is required")))
}

fn recipients(args: &Value, key: &str) -> Result<Vec<String>, ExchangeError> {
    let values = recipient_values(args, key)?;
    if values.is_empty() {
        return Err(ExchangeError::Config(format!(
            "{key} requires at least one recipient"
        )));
    }
    Ok(values)
}

fn recipients_optional(args: &Value, key: &str) -> Result<Vec<String>, ExchangeError> {
    recipient_values(args, key)
}

fn recipient_values(args: &Value, key: &str) -> Result<Vec<String>, ExchangeError> {
    let Some(value) = args.get(key) else {
        return Ok(Vec::new());
    };
    let values: Vec<String> = match value {
        Value::String(value) => vec![value.clone()],
        Value::Array(values) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| ExchangeError::Config(format!("{key} must contain strings")))
            })
            .collect::<Result<_, _>>()?,
        _ => {
            return Err(ExchangeError::Config(format!(
                "{key} must be a string or array"
            )))
        }
    };
    Ok(values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect())
}

fn create_message_body(
    to: &[String],
    cc: &[String],
    bcc: &[String],
    subject: &str,
    body: &str,
) -> String {
    let recipients = |values: &[String]| {
        values
            .iter()
            .map(|address| {
                format!(
                    r#"<t:Mailbox><t:EmailAddress>{}</t:EmailAddress></t:Mailbox>"#,
                    xml_escape(address)
                )
            })
            .collect::<String>()
    };
    let cc_xml = if cc.is_empty() {
        String::new()
    } else {
        format!("<t:CcRecipients>{}</t:CcRecipients>", recipients(cc))
    };
    let bcc_xml = if bcc.is_empty() {
        String::new()
    } else {
        format!("<t:BccRecipients>{}</t:BccRecipients>", recipients(bcc))
    };
    format!(
        r#"<m:CreateItem MessageDisposition="SendAndSaveCopy"><m:SavedItemFolderId><t:DistinguishedFolderId Id="sentitems"/></m:SavedItemFolderId><m:Items><t:Message><t:Subject>{}</t:Subject><t:Body BodyType="Text">{}</t:Body><t:ToRecipients>{}</t:ToRecipients>{cc_xml}{bcc_xml}</t:Message></m:Items></m:CreateItem>"#,
        xml_escape(subject),
        xml_escape(body),
        recipients(to)
    )
}

fn parse_sent_item_id(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut in_message = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = local(&event);
                if name == "Message" {
                    in_message = true;
                } else if in_message && name == "ItemId" {
                    if let Some(id) = attribute(&event, "Id") {
                        return Some(id);
                    }
                }
            }
            Ok(Event::Empty(event)) => {
                if in_message && local(&event) == "ItemId" {
                    if let Some(id) = attribute(&event, "Id") {
                        return Some(id);
                    }
                }
            }
            Ok(Event::End(event)) => {
                if local_end(&event) == "Message" {
                    in_message = false;
                }
            }
            Ok(Event::Eof) => return None,
            _ => {}
        }
    }
}

fn parse_ews_error(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut in_message_text = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                in_message_text = local(&event) == "MessageText";
            }
            Ok(Event::Text(text)) if in_message_text => {
                let value = text.xml10_content().into_owned();
                if !value.is_empty() {
                    return Some(value);
                }
            }
            Ok(Event::End(_)) => in_message_text = false,
            Ok(Event::Eof) => return None,
            _ => {}
        }
    }
}

fn find_item_body(folder: &str, query: Option<&str>, limit: usize, mailbox: &str) -> String {
    let folder_id = match folder {
        "inbox" | "sentitems" | "drafts" | "deleteditems" | "archive" | "outbox" => {
            format!(
                r#"<t:DistinguishedFolderId Id="{folder}"><t:Mailbox><t:EmailAddress>{}</t:EmailAddress></t:Mailbox></t:DistinguishedFolderId>"#,
                xml_escape(mailbox)
            )
        }
        _ => format!(r#"<t:FolderId Id="{folder}"/>"#),
    };
    // The mailbox is filled by the caller's config; this function is called with a body
    // template and replaced below to keep XML escaping in one place.
    let restriction = query
        .map(|query| {
            format!(
                r#"<m:Restriction><t:Contains ContainmentMode="Substring" ContainmentComparison="IgnoreCase"><t:FieldURI FieldURI="item:Subject"/><t:Constant Value="{}"/></t:Contains></m:Restriction>"#,
                xml_escape(query)
            )
        })
        .unwrap_or_default();
    format!(
        r#"<m:FindItem Traversal="Shallow"><m:ItemShape><t:BaseShape>IdOnly</t:BaseShape><t:AdditionalProperties><t:FieldURI FieldURI="item:Subject"/><t:FieldURI FieldURI="item:DateTimeReceived"/><t:FieldURI FieldURI="message:From"/><t:FieldURI FieldURI="item:Preview"/></t:AdditionalProperties></m:ItemShape><m:IndexedPageItemView MaxEntriesReturned="{limit}" Offset="0" BasePoint="Beginning"/><m:ParentFolderIds>{folder_id}</m:ParentFolderIds>{restriction}</m:FindItem>"#
    )
}

fn get_item_body(item_id: &str) -> String {
    format!(
        r#"<m:GetItem><m:ItemShape><t:BaseShape>AllProperties</t:BaseShape><t:BodyType>Text</t:BodyType></m:ItemShape><m:ItemIds><t:ItemId Id="{}"/></m:ItemIds></m:GetItem>"#,
        xml_escape(item_id)
    )
}

fn get_attachment_body(attachment_id: &str) -> String {
    format!(
        r#"<m:GetAttachment><m:AttachmentIds><t:AttachmentId Id="{}"/></m:AttachmentIds></m:GetAttachment>"#,
        xml_escape(attachment_id)
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[derive(Debug, Default)]
struct Message {
    id: String,
    subject: String,
    received: String,
    from_name: String,
    from_address: String,
    preview: String,
    body: String,
    to: Vec<String>,
    attachments: Vec<AttachmentSummary>,
}

#[derive(Debug, Default, Clone)]
struct AttachmentSummary {
    id: String,
    name: String,
    content_type: String,
    size: i64,
}

#[derive(Debug, Default)]
struct ParsedAttachment {
    name: String,
    content_type: String,
    content: Vec<u8>,
}

fn parse_find_items(xml: &str, limit: usize) -> Value {
    let messages = parse_messages(xml, false);
    let items: Vec<Value> = messages
        .into_iter()
        .take(limit)
        .map(|message| {
            json!({
                "id": message.id,
                "subject": message.subject,
                "receivedAt": message.received,
                "from": { "name": message.from_name, "address": message.from_address },
                "preview": message.preview,
            })
        })
        .collect();
    json!({ "items": items, "count": items.len() })
}

fn parse_get_item(xml: &str) -> Result<Value, ExchangeError> {
    let message = parse_messages(xml, true)
        .into_iter()
        .next()
        .ok_or_else(|| ExchangeError::Ews("EWS returned no message".into()))?;
    Ok(json!({
        "id": message.id,
        "subject": message.subject,
        "receivedAt": message.received,
        "from": { "name": message.from_name, "address": message.from_address },
        "to": message.to,
        "body": message.body,
        "attachments": message.attachments.iter().map(|a| json!({
            "id": a.id, "name": a.name, "contentType": a.content_type, "size": a.size
        })).collect::<Vec<_>>(),
    }))
}

fn parse_attachment(xml: &str) -> Result<ParsedAttachment, ExchangeError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut current = String::new();
    let mut attachment = ParsedAttachment::default();
    let mut content = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => current = local(&event),
            Ok(Event::Text(text)) => {
                let value = text.xml10_content().into_owned();
                match current.as_str() {
                    "Name" => attachment.name = value,
                    "ContentType" => attachment.content_type = value,
                    "Content" => content.push_str(&value),
                    _ => {}
                }
            }
            Ok(Event::End(_)) => current.clear(),
            Ok(Event::Eof) => break,
            Err(error) => return Err(ExchangeError::Ews(format!("invalid EWS XML: {error}"))),
            _ => {}
        }
    }
    attachment.content = base64::engine::general_purpose::STANDARD
        .decode(content.trim())
        .map_err(|_| ExchangeError::Ews("EWS attachment content is not valid base64".into()))?;
    Ok(attachment)
}

fn parse_messages(xml: &str, full: bool) -> Vec<Message> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut messages = Vec::new();
    let mut message: Option<Message> = None;
    let mut current = String::new();
    let mut in_attachment = false;
    let mut attachment: Option<AttachmentSummary> = None;
    let mut in_from = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = local(&event);
                current = name.clone();
                if name == "Message" {
                    message = Some(Message::default());
                } else if name == "FileAttachment" && message.is_some() {
                    in_attachment = true;
                    attachment = Some(AttachmentSummary::default());
                } else if name == "From" && message.is_some() {
                    in_from = true;
                }
                if name == "ItemId" || name == "AttachmentId" {
                    if let Some(value) = attribute(&event, "Id") {
                        if let Some(message) = message.as_mut() {
                            if let Some(attachment) = attachment.as_mut() {
                                attachment.id = value;
                            } else if message.id.is_empty() {
                                message.id = value;
                            }
                        }
                    }
                }
            }
            Ok(Event::Empty(event)) => {
                let name = local(&event);
                if name == "ItemId" || name == "AttachmentId" {
                    if let Some(value) = attribute(&event, "Id") {
                        if let Some(message) = message.as_mut() {
                            if let Some(attachment) = attachment.as_mut() {
                                attachment.id = value;
                            } else if message.id.is_empty() {
                                message.id = value;
                            }
                        }
                    }
                }
            }
            Ok(Event::Text(text)) => {
                let value = text.xml10_content().into_owned();
                if let Some(message) = message.as_mut() {
                    if let Some(attachment) = attachment.as_mut() {
                        match current.as_str() {
                            "AttachmentId" => attachment.id = value,
                            "Name" => attachment.name = value,
                            "ContentType" => attachment.content_type = value,
                            "Size" => attachment.size = value.parse().unwrap_or(0),
                            _ => {}
                        }
                    } else {
                        match current.as_str() {
                            "ItemId" => {
                                if message.id.is_empty() {
                                    message.id = value;
                                }
                            }
                            "Subject" => message.subject = value,
                            "DateTimeReceived" => message.received = value,
                            "Preview" => message.preview = value,
                            "EmailAddress" if in_from => message.from_address = value,
                            "Name" if in_from => message.from_name = value,
                            "Body" => message.body = value,
                            "EmailAddress" => message.to.push(value),
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::End(event)) => {
                let name = local_end(&event);
                if name == "From" {
                    in_from = false;
                }
                if name == "FileAttachment" && in_attachment {
                    if let (Some(message), Some(attachment)) = (message.as_mut(), attachment.take())
                    {
                        message.attachments.push(attachment);
                    }
                    in_attachment = false;
                }
                if name == "Message" {
                    if let Some(message) = message.take() {
                        if !full || !message.body.is_empty() {
                            messages.push(message);
                        }
                    }
                }
                current.clear();
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
    }
    messages
}

fn attribute(event: &quick_xml::events::BytesStart<'_>, key: &str) -> Option<String> {
    event
        .attributes()
        .flatten()
        .find_map(|attr| (attr.key.as_ref() == key).then(|| attr.value.as_ref().to_string()))
}

fn local(event: &quick_xml::events::BytesStart<'_>) -> String {
    event.local_name().as_ref().to_string()
}

fn local_end(event: &quick_xml::events::BytesEnd<'_>) -> String {
    event.local_name().as_ref().to_string()
}

pub fn capability_input_schema(capability: &str) -> Value {
    match capability {
        "mail.search" => json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Subject substring; omit for the newest messages" },
                "folder": { "type": "string", "description": "Allowed folder, default inbox" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
            }
        }),
        "mail.get" => json!({
            "type": "object",
            "properties": { "itemId": { "type": "string" } },
            "required": ["itemId"]
        }),
        "mail.attachments.get" => json!({
            "type": "object",
            "properties": { "attachmentId": { "type": "string" } },
            "required": ["attachmentId"]
        }),
        "mail.send" => json!({
            "type": "object",
            "properties": {
                "to": { "oneOf": [{ "type": "string" }, { "type": "array", "items": { "type": "string" } }] },
                "cc": { "oneOf": [{ "type": "string" }, { "type": "array", "items": { "type": "string" } }] },
                "bcc": { "oneOf": [{ "type": "string" }, { "type": "array", "items": { "type": "string" } }] },
                "subject": { "type": "string", "minLength": 1 },
                "body": { "type": "string", "minLength": 1 }
            },
            "required": ["to", "subject", "body"]
        }),
        _ => json!({ "type": "object" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_find_item_ids_and_previews() {
        let xml = r#"<s:Envelope xmlns:s="x" xmlns:t="t" xmlns:m="m"><s:Body><m:FindItemResponse><m:ResponseMessages><m:FindItemResponseMessage ResponseClass="Success"><m:RootFolder><t:Items><t:Message><t:ItemId Id="AAMk="/><t:Subject>Hello</t:Subject><t:DateTimeReceived>2026-01-01T10:00:00Z</t:DateTimeReceived><t:From><t:Mailbox><t:Name>Sender</t:Name><t:EmailAddress>s@example.com</t:EmailAddress></t:Mailbox></t:From><t:Preview>Preview text</t:Preview></t:Message></t:Items></m:RootFolder></m:FindItemResponseMessage></m:ResponseMessages></m:FindItemResponse></s:Body></s:Envelope>"#;
        let result = parse_find_items(xml, 25);
        assert_eq!(result["items"][0]["id"], "AAMk=");
        assert_eq!(result["items"][0]["subject"], "Hello");
        assert_eq!(result["items"][0]["from"]["address"], "s@example.com");
    }

    #[test]
    fn parses_message_body_and_attachments() {
        let xml = r#"<s:Envelope xmlns:s="x" xmlns:t="t" xmlns:m="m"><s:Body><m:GetItemResponse><m:ResponseMessages><m:GetItemResponseMessage ResponseClass="Success"><m:Items><t:Message><t:ItemId Id="ID"/><t:Subject>S</t:Subject><t:Body BodyType="Text">Body text</t:Body><t:ToRecipients><t:Mailbox><t:EmailAddress>to@example.com</t:EmailAddress></t:Mailbox></t:ToRecipients><t:Attachments><t:FileAttachment><t:AttachmentId Id="ATT"/><t:Name>a.txt</t:Name><t:ContentType>text/plain</t:ContentType><t:Size>3</t:Size></t:FileAttachment></t:Attachments></t:Message></m:Items></m:GetItemResponseMessage></m:ResponseMessages></m:GetItemResponse></s:Body></s:Envelope>"#;
        let result = parse_get_item(xml).unwrap();
        assert_eq!(result["body"], "Body text");
        assert_eq!(result["attachments"][0]["id"], "ATT");
        assert_eq!(result["to"][0], "to@example.com");
    }

    #[test]
    fn parses_attachment_content() {
        let xml = r#"<s:Envelope xmlns:s="x" xmlns:t="t" xmlns:m="m"><s:Body><m:GetAttachmentResponse><m:ResponseMessages><m:GetAttachmentResponseMessage ResponseClass="Success"><m:Attachments><t:FileAttachment><t:Name>a.txt</t:Name><t:ContentType>text/plain</t:ContentType><t:Content>aGVsbG8=</t:Content></t:FileAttachment></m:Attachments></m:GetAttachmentResponseMessage></m:ResponseMessages></m:GetAttachmentResponse></s:Body></s:Envelope>"#;
        let result = parse_attachment(xml).unwrap();
        assert_eq!(result.name, "a.txt");
        assert_eq!(result.content, b"hello");
    }

    #[test]
    fn recipient_allowlist_supports_exact_domain_and_everyone() {
        let client = ExchangeClient::from_connection(
            &json!({
                "ewsUrl": "https://exchange/EWS/Exchange.asmx",
                "mailbox": "a@b",
                "allowedRecipients": ["ops@example.com", "*@customer.test"]
            }),
            Some(r#"{"username":"u","password":"p"}"#.into()),
        )
        .unwrap();
        assert!(client.recipient_allowed("ops@example.com"));
        assert!(client.recipient_allowed("OPS@EXAMPLE.COM"));
        assert!(client.recipient_allowed("anyone@customer.test"));
        assert!(!client.recipient_allowed("other@example.com"));
        assert!(!client.recipient_allowed(""));

        let everyone = ExchangeClient::from_connection(
            &json!({
                "ewsUrl": "https://exchange/EWS/Exchange.asmx",
                "mailbox": "a@b",
                "allowedRecipients": ["*"]
            }),
            Some(r#"{"username":"u","password":"p"}"#.into()),
        )
        .unwrap();
        assert!(everyone.recipient_allowed("anyone@anywhere.test"));
    }

    #[test]
    fn parses_sent_item_id_and_ews_error() {
        let sent = r#"<s:Envelope xmlns:s="x" xmlns:t="t" xmlns:m="m"><s:Body><m:CreateItemResponse><m:ResponseMessages><m:CreateItemResponseMessage ResponseClass="Success"><m:Items><t:Message><t:ItemId Id="SENT"/></t:Message></m:Items></m:CreateItemResponseMessage></m:ResponseMessages></m:CreateItemResponse></s:Body></s:Envelope>"#;
        assert_eq!(parse_sent_item_id(sent).as_deref(), Some("SENT"));
        let error = r#"<s:Envelope xmlns:s="x" xmlns:t="t" xmlns:m="m"><s:Body><m:CreateItemResponse><m:ResponseMessages><m:CreateItemResponseMessage ResponseClass="Error"><m:MessageText>Recipient rejected</m:MessageText></m:CreateItemResponseMessage></m:ResponseMessages></m:CreateItemResponse></s:Body></s:Envelope>"#;
        assert_eq!(
            parse_ews_error(error).as_deref(),
            Some("Recipient rejected")
        );
        assert!(parse_sent_item_id(error).is_none());
    }

    #[test]
    fn config_requires_https_and_credential() {
        let config = json!({ "ewsUrl": "http://exchange/EWS/Exchange.asmx", "mailbox": "a@b" });
        assert!(ExchangeClient::from_connection(
            &config,
            Some(r#"{"username":"u","password":"p"}"#.into())
        )
        .is_err());
        let config = json!({ "ewsUrl": "https://exchange/EWS/Exchange.asmx", "mailbox": "a@b" });
        assert!(ExchangeClient::from_connection(&config, None).is_err());
    }
}
