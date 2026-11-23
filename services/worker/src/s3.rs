//! S3 connector tools. The client and SigV4 implementation live in `orbit-s3`.

use base64::Engine;
use serde_json::{json, Value};

pub struct S3Connector {
    client: orbit_s3::S3Client,
}

impl S3Connector {
    pub fn from_connection(config: &Value, credential: Option<String>) -> Result<Self, String> {
        let (config, credentials) =
            orbit_s3::S3Config::from_connection(config, credential.as_deref())?;
        Ok(Self {
            client: orbit_s3::S3Client::new(config, credentials)?,
        })
    }

    pub async fn invoke(&self, capability: &str, args: &Value) -> Result<Value, String> {
        match capability {
            "buckets.list" => {
                let buckets = self.client.list_buckets().await?;
                Ok(json!({ "count": buckets.len(), "buckets": buckets }))
            }
            "objects.list" => {
                let bucket = args.get("bucket").and_then(Value::as_str);
                let prefix = args.get("prefix").and_then(Value::as_str);
                let objects = self.client.list_objects(bucket, prefix).await?;
                Ok(json!({ "count": objects.len(), "objects": objects }))
            }
            "objects.get" => {
                let bucket = args.get("bucket").and_then(Value::as_str);
                let key = required(args, "key")?;
                let (body, truncated) = self.client.get_object(bucket, key).await?;
                Ok(json!({
                    "key": key,
                    "size": body.len(),
                    "truncated": truncated,
                    "contentBase64": base64::engine::general_purpose::STANDARD.encode(body),
                }))
            }
            "objects.put" => {
                let bucket = args.get("bucket").and_then(Value::as_str);
                let key = required(args, "key")?;
                let content = args
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or("content is required")?;
                self.client
                    .put_object(bucket, key, content.as_bytes(), "text/plain; charset=utf-8")
                    .await?;
                Ok(json!({ "status": "written", "key": key, "size": content.len() }))
            }
            "objects.delete" => {
                let bucket = args.get("bucket").and_then(Value::as_str);
                let key = required(args, "key")?;
                self.client.delete_object(bucket, key).await?;
                Ok(json!({ "status": "deleted", "key": key }))
            }
            "objects.copy" => {
                let from_bucket = args.get("fromBucket").and_then(Value::as_str);
                let from_key = required(args, "fromKey")?;
                let to_bucket = args.get("toBucket").and_then(Value::as_str);
                let to_key = required(args, "toKey")?;
                self.client
                    .copy_object(from_bucket, from_key, to_bucket, to_key)
                    .await?;
                Ok(json!({ "status": "copied", "fromKey": from_key, "toKey": to_key }))
            }
            other => Err(format!("unsupported S3 capability: {other}")),
        }
    }
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{key} is required"))
}
