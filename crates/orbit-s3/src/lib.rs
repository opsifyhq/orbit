//! Minimal S3 client for Amazon S3 and RustFS.
//!
//! ORBIT uses only single-request operations: PutObject, GetObject, DeleteObject,
//! CopyObject, ListBuckets and ListObjectsV2. Requests are SigV4-signed here so the API
//! and worker share one dependency-light client.

use std::collections::BTreeMap;

use chrono::Utc;
use hmac::{Hmac, Mac};
use quick_xml::events::Event;
use sha2::{Digest, Sha256};

const MAX_ERROR_BODY: usize = 4_000;
const DEFAULT_MAX_OBJECT_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

impl S3Credentials {
    /// The connection credential is a JSON object, never a comma-joined secret.
    pub fn from_secret(raw: &str) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(raw)
            .map_err(|_| "S3 credential must be a JSON object".to_string())?;
        let access_key_id = value
            .get("accessKeyId")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or("S3 credential is missing accessKeyId")?;
        let secret_access_key = value
            .get("secretAccessKey")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or("S3 credential is missing secretAccessKey")?;
        Ok(Self {
            access_key_id: access_key_id.trim().to_string(),
            secret_access_key: secret_access_key.trim().to_string(),
            session_token: value
                .get("sessionToken")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
        })
    }
}

#[derive(Debug, Clone)]
pub struct S3Config {
    pub endpoint: String,
    pub region: String,
    pub path_style: bool,
    pub allowed_buckets: Vec<String>,
    pub default_bucket: Option<String>,
    pub key_prefix: String,
    pub max_object_bytes: usize,
}

impl S3Config {
    pub fn from_connection(
        config: &serde_json::Value,
        credential: Option<&str>,
    ) -> Result<(Self, S3Credentials), String> {
        let credentials =
            S3Credentials::from_secret(credential.ok_or("S3 credential is required")?)?;
        let provider = config
            .get("provider")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("amazon");
        let region = config
            .get("region")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("us-east-1")
            .to_string();
        let endpoint = config
            .get("endpoint")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.trim_end_matches('/').to_string())
            .unwrap_or_else(|| match provider {
                "rustfs" => String::new(),
                _ => format!("https://s3.{region}.amazonaws.com"),
            });
        if endpoint.is_empty() {
            return Err("S3 endpoint is required for RustFS".into());
        }
        if !endpoint.starts_with("https://") && !endpoint.starts_with("http://") {
            return Err("S3 endpoint must be an http(s) URL".into());
        }
        let allowed_buckets = string_list(config.get("allowedBuckets"));
        let default_bucket = config
            .get("defaultBucket")
            .or_else(|| config.get("bucket"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let key_prefix = config
            .get("keyPrefix")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim_start_matches('/')
            .to_string();
        if !key_prefix.is_empty() && !key_prefix.ends_with('/') {
            return Err("S3 keyPrefix must end with '/'".into());
        }
        let max_object_bytes = config
            .get("maxObjectBytes")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(DEFAULT_MAX_OBJECT_BYTES as u64)
            .clamp(1_024, 100 * 1024 * 1024) as usize;
        Ok((
            Self {
                endpoint,
                region,
                path_style: config
                    .get("pathStyle")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(provider == "rustfs"),
                allowed_buckets,
                default_bucket,
                key_prefix,
                max_object_bytes,
            },
            credentials,
        ))
    }

    fn bucket(&self, bucket: Option<&str>) -> Result<String, String> {
        let bucket = bucket
            .or(self.default_bucket.as_deref())
            .ok_or("bucket is required (or set config.defaultBucket)")?
            .trim();
        validate_bucket(bucket)?;
        if self.allowed_buckets.is_empty()
            || !self.allowed_buckets.iter().any(|item| item == bucket)
        {
            return Err(format!(
                "bucket '{bucket}' is not in the connection's allowedBuckets (an empty list denies every operation)"
            ));
        }
        Ok(bucket.to_string())
    }

    fn object_key(&self, key: &str) -> Result<String, String> {
        let key = key.trim_start_matches('/');
        if key.is_empty()
            || key.contains('\0')
            || key
                .split('/')
                .any(|segment| segment == ".." || segment.is_empty())
        {
            return Err("object key is empty or contains an invalid path segment".into());
        }
        if !key.starts_with(&self.key_prefix) {
            return Err(format!(
                "object key must start with the configured keyPrefix '{}'",
                self.key_prefix
            ));
        }
        Ok(key.to_string())
    }
}

fn string_list(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn validate_bucket(value: &str) -> Result<(), String> {
    if value.len() < 3
        || value.len() > 63
        || value.starts_with('.')
        || value.ends_with('.')
        || value
            .chars()
            .any(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.')))
    {
        return Err("invalid S3 bucket name".into());
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectInfo {
    pub key: String,
    pub size: u64,
    pub last_modified: Option<String>,
    pub etag: Option<String>,
}

pub struct S3Client {
    config: S3Config,
    credentials: S3Credentials,
    http: reqwest::Client,
}

impl S3Client {
    pub fn new(config: S3Config, credentials: S3Credentials) -> Result<Self, String> {
        let http = orbit_security::guarded_client_with_policy(
            std::time::Duration::from_secs(60),
            orbit_security::OutboundPolicy::from_env(),
        );
        Ok(Self {
            config,
            credentials,
            http,
        })
    }

    pub fn config(&self) -> &S3Config {
        &self.config
    }

    pub async fn put_object(
        &self,
        bucket: Option<&str>,
        key: &str,
        body: &[u8],
        content_type: &str,
    ) -> Result<(), String> {
        if body.len() > self.config.max_object_bytes {
            return Err(format!(
                "object is {} bytes; the connection limit is {} bytes",
                body.len(),
                self.config.max_object_bytes
            ));
        }
        let bucket = self.config.bucket(bucket)?;
        let key = self.config.object_key(key)?;
        let response = self
            .send(
                reqwest::Method::PUT,
                Some(&bucket),
                Some(&key),
                &[],
                Some(body),
                Some(content_type),
                None,
            )
            .await?;
        ensure_success(response, "PutObject").await.map(|_| ())
    }

    pub async fn get_object(
        &self,
        bucket: Option<&str>,
        key: &str,
    ) -> Result<(Vec<u8>, bool), String> {
        let bucket = self.config.bucket(bucket)?;
        let key = self.config.object_key(key)?;
        let response = self
            .send(
                reqwest::Method::GET,
                Some(&bucket),
                Some(&key),
                &[],
                None,
                None,
                None,
            )
            .await?;
        let response = check_success(response, "GetObject").await?;
        let (body, truncated) = read_limited(response, self.config.max_object_bytes).await?;
        Ok((body, truncated))
    }

    pub async fn delete_object(&self, bucket: Option<&str>, key: &str) -> Result<(), String> {
        let bucket = self.config.bucket(bucket)?;
        let key = self.config.object_key(key)?;
        let response = self
            .send(
                reqwest::Method::DELETE,
                Some(&bucket),
                Some(&key),
                &[],
                None,
                None,
                None,
            )
            .await?;
        ensure_success(response, "DeleteObject").await.map(|_| ())
    }

    pub async fn copy_object(
        &self,
        from_bucket: Option<&str>,
        from_key: &str,
        to_bucket: Option<&str>,
        to_key: &str,
    ) -> Result<(), String> {
        let from_bucket = self.config.bucket(from_bucket)?;
        let from_key = self.config.object_key(from_key)?;
        let to_bucket = self.config.bucket(to_bucket)?;
        let to_key = self.config.object_key(to_key)?;
        let copy_source = format!("/{from_bucket}/{}", encode_path(&from_key));
        let response = self
            .send(
                reqwest::Method::PUT,
                Some(&to_bucket),
                Some(&to_key),
                &[],
                None,
                None,
                Some(("x-amz-copy-source", copy_source)),
            )
            .await?;
        ensure_success(response, "CopyObject").await.map(|_| ())
    }

    pub async fn list_buckets(&self) -> Result<Vec<String>, String> {
        let response = self
            .send(reqwest::Method::GET, None, None, &[], None, None, None)
            .await?;
        let body = ensure_success(response, "ListBuckets").await?;
        parse_list(&body, "Bucket").map(|items| {
            items
                .into_iter()
                .filter_map(|item| item.get("Name").cloned())
                .collect()
        })
    }

    pub async fn list_objects(
        &self,
        bucket: Option<&str>,
        prefix: Option<&str>,
    ) -> Result<Vec<ObjectInfo>, String> {
        let bucket = self.config.bucket(bucket)?;
        let prefix = prefix
            .unwrap_or(&self.config.key_prefix)
            .trim_start_matches('/');
        let query = vec![
            ("list-type".to_string(), "2".to_string()),
            ("prefix".to_string(), prefix.to_string()),
            ("max-keys".to_string(), "100".to_string()),
        ];
        let response = self
            .send(
                reqwest::Method::GET,
                Some(&bucket),
                None,
                &query,
                None,
                None,
                None,
            )
            .await?;
        let body = ensure_success(response, "ListObjectsV2").await?;
        parse_list(&body, "Contents").map(|items| {
            items
                .into_iter()
                .map(|item| ObjectInfo {
                    key: item.get("Key").cloned().unwrap_or_default(),
                    size: item
                        .get("Size")
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(0),
                    last_modified: item.get("LastModified").cloned(),
                    etag: item.get("ETag").cloned(),
                })
                .collect()
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn send(
        &self,
        method: reqwest::Method,
        bucket: Option<&str>,
        key: Option<&str>,
        query: &[(String, String)],
        body: Option<&[u8]>,
        content_type: Option<&str>,
        extra_header: Option<(&str, String)>,
    ) -> Result<reqwest::Response, String> {
        let url = build_url(&self.config, bucket, key, query)?;
        orbit_security::check_outbound_url(
            url.as_str(),
            &orbit_security::OutboundPolicy::from_env(),
        )
        .await
        .map_err(|error| format!("S3 endpoint refused ({})", error.category()))?;
        let payload_hash = hex::encode(Sha256::digest(body.unwrap_or_default()));
        let now = Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date = now.format("%Y%m%d").to_string();
        let host = url.host_str().ok_or("S3 endpoint has no host")?;
        let host = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        };
        let mut headers = BTreeMap::new();
        headers.insert("host".to_string(), host.clone());
        headers.insert("x-amz-content-sha256".to_string(), payload_hash.clone());
        headers.insert("x-amz-date".to_string(), amz_date.clone());
        if let Some(token) = &self.credentials.session_token {
            headers.insert("x-amz-security-token".to_string(), token.clone());
        }
        if let Some((name, value)) = &extra_header {
            headers.insert(name.to_ascii_lowercase(), value.clone());
        }
        let signed_headers = headers.keys().cloned().collect::<Vec<_>>().join(";");
        let canonical_headers = headers
            .iter()
            .map(|(key, value)| format!("{key}:{}\n", value.trim()))
            .collect::<String>();
        let canonical_query = canonical_query(query);
        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method.as_str(),
            url.path(),
            canonical_query,
            canonical_headers,
            signed_headers,
            payload_hash
        );
        let scope = format!("{date}/{}/s3/aws4_request", self.config.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );
        let signature = sign(
            &self.credentials,
            &date,
            &self.config.region,
            &string_to_sign,
        )?;
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            self.credentials.access_key_id
        );
        let mut request = self
            .http
            .request(method, url)
            .header("authorization", authorization);
        for (key, value) in &headers {
            if key != "host" {
                request = request.header(key.as_str(), value.as_str());
            }
        }
        if let Some(content_type) = content_type {
            request = request.header("content-type", content_type);
        }
        if let Some(body) = body {
            request = request.body(body.to_vec());
        }
        request
            .send()
            .await
            .map_err(|error| orbit_security::describe_request_error(&error))
    }
}

async fn check_success(
    response: reqwest::Response,
    operation: &str,
) -> Result<reqwest::Response, String> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.bytes().await.unwrap_or_default();
    let text = String::from_utf8_lossy(&body[..body.len().min(MAX_ERROR_BODY)]).into_owned();
    let code = extract_tag(&text, "Code").unwrap_or_else(|| status.to_string());
    let message = extract_tag(&text, "Message").unwrap_or_else(|| "S3 request failed".into());
    Err(format!("{operation} failed ({status}, {code}): {message}"))
}

async fn ensure_success(response: reqwest::Response, operation: &str) -> Result<Vec<u8>, String> {
    let response = check_success(response, operation).await?;
    Ok(response.bytes().await.unwrap_or_default().to_vec())
}

fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    let start = format!("<{tag}>");
    let end = format!("</{tag}>");
    let from = xml.find(&start)? + start.len();
    let to = xml[from..].find(&end)? + from;
    Some(
        quick_xml::escape::unescape(&xml[from..to])
            .ok()?
            .into_owned(),
    )
}

/// Parse `<Contents>`/`<Bucket>` entries into flat maps of their child tags.
fn parse_list(xml: &[u8], item_tag: &str) -> Result<Vec<BTreeMap<String, String>>, String> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut items = Vec::new();
    let mut current: Option<BTreeMap<String, String>> = None;
    let mut field: Option<String> = None;
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => {
                let name = event.name().as_ref().to_string();
                if name == item_tag {
                    current = Some(BTreeMap::new());
                } else if current.is_some() {
                    field = Some(name);
                }
            }
            Ok(Event::Text(event)) => {
                if let (Some(item), Some(field)) = (current.as_mut(), field.as_ref()) {
                    let text = event.as_ref();
                    let text =
                        quick_xml::escape::unescape(text).map_err(|error| error.to_string())?;
                    item.insert(field.clone(), text.into_owned());
                }
            }
            Ok(Event::End(event)) => {
                let name = event.name().as_ref().to_string();
                if name == item_tag {
                    if let Some(item) = current.take() {
                        items.push(item);
                    }
                }
                field = None;
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("invalid S3 XML response: {error}")),
            _ => {}
        }
        buffer.clear();
    }
    Ok(items)
}

fn build_url(
    config: &S3Config,
    bucket: Option<&str>,
    key: Option<&str>,
    query: &[(String, String)],
) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(&config.endpoint)
        .map_err(|error| format!("invalid S3 endpoint: {error}"))?;
    let mut path = String::new();
    if let Some(bucket) = bucket {
        if config.path_style {
            path.push('/');
            path.push_str(bucket);
        } else {
            let host = url.host_str().ok_or("S3 endpoint has no host")?;
            url.set_host(Some(&format!("{bucket}.{host}")))
                .map_err(|error| error.to_string())?;
        }
    }
    if let Some(key) = key {
        path.push('/');
        path.push_str(&encode_path(key));
    }
    if path.is_empty() {
        path.push('/');
    }
    url.set_path(&path);
    if !query.is_empty() {
        url.query_pairs_mut()
            .clear()
            .extend_pairs(query.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }
    Ok(url)
}

fn canonical_query(query: &[(String, String)]) -> String {
    let mut values = query
        .iter()
        .map(|(key, value)| (encode_component(key), encode_component(value)))
        .collect::<Vec<_>>();
    values.sort();
    values
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn encode_path(path: &str) -> String {
    path.split('/')
        .map(encode_component)
        .collect::<Vec<_>>()
        .join("/")
}

fn encode_component(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn sign(
    credentials: &S3Credentials,
    date: &str,
    region: &str,
    string_to_sign: &str,
) -> Result<String, String> {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac =
        HmacSha256::new_from_slice(format!("AWS4{}", credentials.secret_access_key).as_bytes())
            .map_err(|error| error.to_string())?;
    mac.update(date.as_bytes());
    let date_key = mac.finalize().into_bytes();
    let mut mac = HmacSha256::new_from_slice(&date_key).map_err(|error| error.to_string())?;
    mac.update(region.as_bytes());
    let region_key = mac.finalize().into_bytes();
    let mut mac = HmacSha256::new_from_slice(&region_key).map_err(|error| error.to_string())?;
    mac.update(b"s3");
    let service_key = mac.finalize().into_bytes();
    let mut mac = HmacSha256::new_from_slice(&service_key).map_err(|error| error.to_string())?;
    mac.update(b"aws4_request");
    let signing_key = mac.finalize().into_bytes();
    let mut mac = HmacSha256::new_from_slice(&signing_key).map_err(|error| error.to_string())?;
    mac.update(string_to_sign.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

async fn read_limited(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<(Vec<u8>, bool), String> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("could not read S3 object: {error}"))?
    {
        if body.len() + chunk.len() > limit {
            let remaining = limit.saturating_sub(body.len());
            body.extend_from_slice(&chunk[..remaining]);
            return Ok((body, true));
        }
        body.extend_from_slice(&chunk);
    }
    Ok((body, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> (S3Config, S3Credentials) {
        S3Config::from_connection(
            &serde_json::json!({
                "provider": "rustfs",
                "endpoint": "https://s3.example",
                "region": "us-east-1",
                "pathStyle": true,
                "allowedBuckets": ["orbit"],
                "defaultBucket": "orbit",
                "keyPrefix": "orbit/"
            }),
            Some(r#"{ "accessKeyId": "AK", "secretAccessKey": "SK" }"#),
        )
        .unwrap()
    }

    #[test]
    fn keys_cannot_escape_the_prefix() {
        let (config, credentials) = connection();
        let client = S3Client::new(config, credentials).unwrap();
        assert!(client.config.object_key("orbit/runs/a.txt").is_ok());
        assert!(client.config.object_key("other/a.txt").is_err());
        assert!(client.config.object_key("orbit/../a.txt").is_err());
        assert!(client.config.object_key("/orbit//a.txt").is_err());
    }

    #[test]
    fn bucket_allowlist_is_required() {
        let (config, credentials) = connection();
        let client = S3Client::new(config, credentials).unwrap();
        assert!(client.config.bucket(Some("orbit")).is_ok());
        assert!(client.config.bucket(Some("other")).is_err());
    }

    #[test]
    fn path_style_urls_include_the_bucket() {
        let (config, credentials) = connection();
        let client = S3Client::new(config, credentials).unwrap();
        let url = build_url(
            &client.config,
            Some("orbit"),
            Some("orbit/runs/a b.txt"),
            &[("list-type".into(), "2".into())],
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://s3.example/orbit/orbit/runs/a%20b.txt?list-type=2"
        );
    }

    #[tokio::test]
    async fn put_object_signs_a_path_style_request() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        std::env::set_var("ORBIT_OUTBOUND_ALLOWED_HOSTS", "127.0.0.1");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 4096];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut chunk).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
            }
            let _ = tx.send(String::from_utf8_lossy(&request).into_owned());
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
        });
        let (config, credentials) = S3Config::from_connection(
            &serde_json::json!({
                "provider": "rustfs",
                "endpoint": format!("http://{address}"),
                "region": "us-east-1",
                "pathStyle": true,
                "allowedBuckets": ["orbit"],
                "defaultBucket": "orbit",
                "keyPrefix": "orbit/"
            }),
            Some(r#"{ "accessKeyId": "AK", "secretAccessKey": "SK" }"#),
        )
        .unwrap();
        let client = S3Client::new(config, credentials).unwrap();
        client
            .put_object(None, "orbit/runs/one.txt", b"hello", "text/plain")
            .await
            .unwrap();
        let request = rx.await.unwrap();
        assert!(
            request.starts_with("PUT /orbit/orbit/runs/one.txt HTTP/1.1"),
            "{request}"
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: aws4-hmac-sha256"),
            "{request}"
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("x-amz-content-sha256:"),
            "{request}"
        );
    }

    #[test]
    fn credential_must_be_json() {
        assert!(S3Credentials::from_secret("AK:SK").is_err());
    }
}
