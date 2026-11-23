//! Read-only Kubernetes connector for the tool broker.
//!
//! Supports a ServiceAccount token or a static kubeconfig (token/client cert only). Auth
//! plugins (`exec`) and insecure TLS are rejected; credentials never leave the worker.

use base64::Engine;
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct KubernetesClientConfig {
    api_server: String,
    token: Option<String>,
    ca_pem: Option<Vec<u8>>,
    identity_pem: Option<Vec<u8>>,
    namespaces: Vec<String>,
    default_namespace: Option<String>,
    allowed_resources: Vec<String>,
    max_log_bytes: usize,
    timeout: std::time::Duration,
}

impl KubernetesClientConfig {
    pub fn from_connection(config: &Value, credential: Option<String>) -> Result<Self, String> {
        let auth_mode = config
            .get("authMode")
            .and_then(Value::as_str)
            .unwrap_or("service_account");
        let mut api_server = config
            .get("apiServer")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim_end_matches('/')
            .to_string();
        let mut token = None;
        let mut identity_pem = None;
        let mut ca_pem = config
            .get("caCert")
            .and_then(Value::as_str)
            .map(|value| value.as_bytes().to_vec());

        match auth_mode {
            "service_account" => {
                token = credential;
                if api_server.is_empty() {
                    api_server = "https://kubernetes.default.svc".into();
                }
            }
            "kubeconfig" => {
                let raw = credential.ok_or("kubeconfig credential is required")?;
                let doc: Value = serde_yaml::from_str(&raw)
                    .map_err(|error| format!("invalid kubeconfig YAML: {error}"))?;
                let context_name = config
                    .get("context")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        doc.get("current-context")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .ok_or("kubeconfig has no current-context; set config.context")?;
                let context = find_named(doc.get("contexts"), &context_name)
                    .and_then(|item| item.get("context"))
                    .ok_or_else(|| format!("kubeconfig context '{context_name}' was not found"))?;
                let cluster_name = context
                    .get("cluster")
                    .and_then(Value::as_str)
                    .ok_or("kubeconfig context has no cluster")?;
                let user_name = context.get("user").and_then(Value::as_str);
                let cluster = find_named(doc.get("clusters"), cluster_name)
                    .and_then(|item| item.get("cluster"))
                    .ok_or_else(|| format!("kubeconfig cluster '{cluster_name}' was not found"))?;
                if cluster
                    .get("insecure-skip-tls-verify")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    return Err("kubeconfig insecure-skip-tls-verify is not allowed".into());
                }
                api_server = cluster
                    .get("server")
                    .and_then(Value::as_str)
                    .ok_or("kubeconfig cluster has no server")?
                    .trim_end_matches('/')
                    .to_string();
                if let Some(encoded) = cluster
                    .get("certificate-authority-data")
                    .and_then(Value::as_str)
                {
                    ca_pem = Some(
                        base64::engine::general_purpose::STANDARD
                            .decode(encoded)
                            .map_err(|error| {
                                format!("invalid certificate-authority-data: {error}")
                            })?,
                    );
                }
                if let Some(user_name) = user_name {
                    let user = find_named(doc.get("users"), user_name)
                        .and_then(|item| item.get("user"))
                        .ok_or_else(|| format!("kubeconfig user '{user_name}' was not found"))?;
                    if user.get("exec").is_some() {
                        return Err("kubeconfig exec authentication is not supported".into());
                    }
                    if let Some(value) = user.get("token").and_then(Value::as_str) {
                        token = Some(value.to_string());
                    } else if let (Some(cert), Some(key)) = (
                        user.get("client-certificate-data").and_then(Value::as_str),
                        user.get("client-key-data").and_then(Value::as_str),
                    ) {
                        let cert = base64::engine::general_purpose::STANDARD
                            .decode(cert)
                            .map_err(|error| format!("invalid client-certificate-data: {error}"))?;
                        let key = base64::engine::general_purpose::STANDARD
                            .decode(key)
                            .map_err(|error| format!("invalid client-key-data: {error}"))?;
                        let mut pem = cert;
                        if !pem.ends_with(b"\n") {
                            pem.push(b'\n');
                        }
                        pem.extend(key);
                        identity_pem = Some(pem);
                    } else {
                        return Err("kubeconfig user has no static token".into());
                    }
                }
            }
            other => return Err(format!("unknown Kubernetes authMode '{other}'")),
        }
        if api_server.is_empty() || !api_server.starts_with("https://") {
            return Err("apiServer must be an https URL".into());
        }

        let namespaces = string_list(config.get("namespaces"));
        let allowed_resources = string_list(config.get("allowedResources"));
        let timeout = config
            .get("timeoutSeconds")
            .and_then(Value::as_u64)
            .unwrap_or(20)
            .clamp(1, 120);
        let max_log_bytes = config
            .get("maxLogBytes")
            .and_then(Value::as_u64)
            .unwrap_or(200_000)
            .clamp(1_000, 2_000_000) as usize;
        Ok(Self {
            api_server,
            token,
            ca_pem,
            identity_pem,
            namespaces,
            default_namespace: config
                .get("defaultNamespace")
                .and_then(Value::as_str)
                .map(str::to_string),
            allowed_resources,
            max_log_bytes,
            timeout: std::time::Duration::from_secs(timeout),
        })
    }

    /// The namespaces tools may use. An empty `namespaces` allowlist allows only
    /// `defaultNamespace`; with neither set, every namespaced tool is refused. `*` lets
    /// the kubeconfig/RBAC decide which namespaces are reachable.
    fn allowed_namespaces(&self) -> Vec<&str> {
        if self.namespaces.is_empty() {
            self.default_namespace.as_deref().into_iter().collect()
        } else {
            self.namespaces.iter().map(String::as_str).collect()
        }
    }

    fn all_namespaces(&self) -> bool {
        self.namespaces.iter().any(|namespace| namespace == "*")
    }

    fn namespace(&self, args: &Value) -> Result<String, String> {
        let namespace = args
            .get("namespace")
            .and_then(Value::as_str)
            .or(self.default_namespace.as_deref())
            .ok_or("namespace is required (or set config.defaultNamespace)")?;
        validate_segment(namespace, "namespace")?;
        if !self.all_namespaces() && !self.allowed_namespaces().contains(&namespace) {
            return Err(format!(
                "namespace '{namespace}' is not allowed on this connection; add it to \
                 config.namespaces, use '*' to follow the kubeconfig, or set config.defaultNamespace"
            ));
        }
        Ok(namespace.to_string())
    }

    /// An empty `allowedResources` list denies every tool; `*` allows them all.
    fn resource_allowed(&self, resource: &str) -> bool {
        self.allowed_resources
            .iter()
            .any(|item| item == resource || item == "*")
    }

    /// A client bound to the platform's outbound policy: loopback, link-local and
    /// cloud-metadata addresses (and private ranges when `ORBIT_OUTBOUND_ALLOW_PRIVATE` is
    /// false) are refused at DNS resolution, and redirects to another host are not
    /// followed (#321).
    fn client(&self) -> Result<reqwest::Client, String> {
        let mut builder = orbit_security::guard_client_builder(
            reqwest::Client::builder().timeout(self.timeout),
            orbit_security::OutboundPolicy::from_env(),
        );
        if let Some(pem) = &self.ca_pem {
            let certificate = reqwest::Certificate::from_pem(pem)
                .map_err(|error| format!("invalid caCert: {error}"))?;
            builder = builder.add_root_certificate(certificate);
        }
        if let Some(pem) = &self.identity_pem {
            let identity = reqwest::Identity::from_pem(pem)
                .map_err(|error| format!("invalid client certificate/key: {error}"))?;
            builder = builder.identity(identity);
        }
        builder.build().map_err(|error| error.to_string())
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
        content_type: Option<&str>,
    ) -> Result<reqwest::Response, String> {
        // The resolver guard does not see IP-literal hosts; check the address up front.
        orbit_security::check_outbound_url(
            &self.api_server,
            &orbit_security::OutboundPolicy::from_env(),
        )
        .await
        .map_err(|error| format!("request refused ({})", error.category()))?;
        let mut request = self
            .client()?
            .request(method, format!("{}{path}", self.api_server));
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if !query.is_empty() {
            request = request.query(query);
        }
        if let Some(content_type) = content_type {
            request = request.header(reqwest::header::CONTENT_TYPE, content_type);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        request
            .send()
            .await
            .map_err(|error| orbit_security::describe_request_error(&error))
    }

    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<reqwest::Response, String> {
        self.request(reqwest::Method::GET, path, query, None, None)
            .await
    }

    /// Parse and connect, then list the namespaces visible to this credential. Used by the
    /// connection Test button; RBAC errors surface as a readable message. The test checks
    /// the credential itself, so it does not apply the connection's tool allowlists.
    pub async fn test_connection(&self) -> Result<Value, String> {
        let response = self.get("/api/v1/namespaces", &[]).await?;
        let status = response.status();
        let data: Value = response
            .json()
            .await
            .map_err(|error| format!("invalid Kubernetes response: {error}"))?;
        if !status.is_success() {
            // A kubeconfig is often scoped to one namespace and cannot list namespaces.
            // Fall back to a bounded pod list in an allowed namespace so the connection
            // can still be validated without cluster-wide RBAC.
            if status == reqwest::StatusCode::FORBIDDEN {
                let probe_namespace = self.default_namespace.as_deref().or_else(|| {
                    self.allowed_namespaces()
                        .into_iter()
                        .find(|namespace| *namespace != "*")
                });
                if let Some(namespace) = probe_namespace {
                    let probe = self
                        .get(
                            &format!("/api/v1/namespaces/{namespace}/pods"),
                            &[("limit", "1".to_string())],
                        )
                        .await?;
                    let probe_status = probe.status();
                    if probe_status.is_success() {
                        return Ok(json!({
                            "namespaces": self.allowed_namespaces(),
                            "count": self.allowed_namespaces().len(),
                            "message": format!(
                                "Kubernetes reachable; namespace listing is not permitted, but namespace {namespace} is readable"
                            ),
                        }));
                    }
                    let probe_body: Value = probe.json().await.unwrap_or(Value::Null);
                    return Err(format!(
                        "Kubernetes namespace listing returned {status} and the configured namespace probe returned {probe_status}: {probe_body}"
                    ));
                }
            }
            return Err(format!("Kubernetes returned {status}: {data}"));
        }
        let names: Vec<&str> = data
            .get("items")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.pointer("/metadata/name").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        let shown = names
            .iter()
            .take(10)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        Ok(json!({
            "namespaces": names,
            "count": names.len(),
            "message": if shown.is_empty() {
                "Kubernetes reachable; no namespaces visible".to_string()
            } else {
                format!("Kubernetes reachable; {} namespace(s): {shown}", names.len())
            },
        }))
    }

    pub async fn invoke(&self, capability: &str, args: &Value) -> Result<Value, String> {
        let resource = match capability {
            "namespaces.list" => "namespaces",
            "pods.list" | "pods.get" | "pods.delete" => "pods",
            "pods.logs" => "pods/log",
            "deployments.list" | "deployments.get" | "deployments.restart" => "deployments",
            "events.list" => "events",
            other => return Err(format!("unsupported Kubernetes capability '{other}'")),
        };
        if !self.resource_allowed(resource) {
            return Err(format!(
                "resource '{resource}' is not allowed on this connection; add it to \
                 config.allowedResources (an empty list denies every tool)"
            ));
        }
        let response = match capability {
            "namespaces.list" => self.get("/api/v1/namespaces", &[]).await?,
            "pods.list" => {
                let namespace = self.namespace(args)?;
                let mut query = Vec::new();
                for (arg, param) in [
                    ("labelSelector", "labelSelector"),
                    ("fieldSelector", "fieldSelector"),
                ] {
                    if let Some(value) = args.get(arg).and_then(Value::as_str) {
                        query.push((param, value.to_string()));
                    }
                }
                self.get(&format!("/api/v1/namespaces/{namespace}/pods"), &query)
                    .await?
            }
            "pods.get" => {
                let namespace = self.namespace(args)?;
                let name = required_name(args)?;
                self.get(&format!("/api/v1/namespaces/{namespace}/pods/{name}"), &[])
                    .await?
            }
            "pods.logs" => {
                let namespace = self.namespace(args)?;
                let name = required_name(args)?;
                let mut query = Vec::new();
                let tail = args
                    .get("tailLines")
                    .and_then(Value::as_u64)
                    .unwrap_or(200)
                    .clamp(1, 5000)
                    .to_string();
                query.push(("tailLines", tail));
                if args
                    .get("previous")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    query.push(("previous", "true".into()));
                }
                let response = self
                    .get(
                        &format!("/api/v1/namespaces/{namespace}/pods/{name}/log"),
                        &query,
                    )
                    .await?;
                let status = response.status();
                if !status.is_success() {
                    let body = response.json::<Value>().await.unwrap_or(Value::Null);
                    return Err(error_message(status, &body));
                }
                let (text, truncated) = read_tail(response, self.max_log_bytes).await?;
                return Ok(json!({ "logs": text, "truncated": truncated }));
            }
            "deployments.list" => {
                let namespace = self.namespace(args)?;
                self.get(
                    &format!("/apis/apps/v1/namespaces/{namespace}/deployments"),
                    &[],
                )
                .await?
            }
            "deployments.get" => {
                let namespace = self.namespace(args)?;
                let name = required_name(args)?;
                self.get(
                    &format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}"),
                    &[],
                )
                .await?
            }
            "deployments.restart" => {
                let namespace = self.namespace(args)?;
                let name = required_name(args)?;
                let restarted_at = chrono::Utc::now().to_rfc3339();
                self.request(
                    reqwest::Method::PATCH,
                    &format!("/apis/apps/v1/namespaces/{namespace}/deployments/{name}"),
                    &[],
                    Some(&json!({
                        "spec": {
                            "template": {
                                "metadata": {
                                    "annotations": {
                                        "kubectl.kubernetes.io/restartedAt": restarted_at
                                    }
                                }
                            }
                        }
                    })),
                    Some("application/merge-patch+json"),
                )
                .await?
            }
            "pods.delete" => {
                let namespace = self.namespace(args)?;
                let name = required_name(args)?;
                let grace = args
                    .get("gracePeriodSeconds")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .min(300);
                self.request(
                    reqwest::Method::DELETE,
                    &format!("/api/v1/namespaces/{namespace}/pods/{name}"),
                    &[],
                    Some(&json!({ "gracePeriodSeconds": grace })),
                    None,
                )
                .await?
            }
            "events.list" => {
                let namespace = self.namespace(args)?;
                let mut query = Vec::new();
                if let Some(value) = args.get("fieldSelector").and_then(Value::as_str) {
                    query.push(("fieldSelector", value.to_string()));
                }
                self.get(&format!("/api/v1/namespaces/{namespace}/events"), &query)
                    .await?
            }
            _ => unreachable!(),
        };
        let status = response.status();
        let mut data: Value = response
            .json()
            .await
            .map_err(|error| format!("invalid Kubernetes response: {error}"))?;
        if !status.is_success() {
            return Err(error_message(status, &data));
        }
        match capability {
            "namespaces.list" => {
                if !self.all_namespaces() {
                    let allowed = self.allowed_namespaces();
                    if let Some(items) = data.get_mut("items").and_then(Value::as_array_mut) {
                        items.retain(|item| {
                            item.pointer("/metadata/name")
                                .and_then(Value::as_str)
                                .is_some_and(|name| allowed.contains(&name))
                        });
                    }
                }
            }
            "pods.get" => redact_object(&mut data, "/spec"),
            "pods.list" => redact_items(&mut data, "/spec"),
            "deployments.get" | "deployments.restart" => {
                redact_object(&mut data, "/spec/template/spec")
            }
            "deployments.list" => redact_items(&mut data, "/spec/template/spec"),
            _ => {}
        }
        Ok(data)
    }
}

/// Placeholder for values removed from tool output.
const REDACTED: &str = "[redacted]";

/// Annotations that carry a full copy of the object, including environment values.
const MANIFEST_ANNOTATIONS: &[&str] = &["kubectl.kubernetes.io/last-applied-configuration"];

fn redact_items(list: &mut Value, pod_spec: &str) {
    if let Some(items) = list.get_mut("items").and_then(Value::as_array_mut) {
        for item in items {
            redact_object(item, pod_spec);
        }
    }
}

/// Removes plaintext environment values from a pod or workload before an agent sees it.
/// `valueFrom` references (Secret and ConfigMap names and keys) are kept: they name where a
/// value lives without revealing it.
fn redact_object(object: &mut Value, pod_spec: &str) {
    if let Some(spec) = object.pointer_mut(pod_spec) {
        for list in ["containers", "initContainers", "ephemeralContainers"] {
            let Some(containers) = spec.get_mut(list).and_then(Value::as_array_mut) else {
                continue;
            };
            for container in containers {
                let Some(env) = container.get_mut("env").and_then(Value::as_array_mut) else {
                    continue;
                };
                for variable in env {
                    if let Some(value) = variable.get_mut("value") {
                        *value = json!(REDACTED);
                    }
                }
            }
        }
    }
    if let Some(annotations) = object
        .pointer_mut("/metadata/annotations")
        .and_then(Value::as_object_mut)
    {
        for key in MANIFEST_ANNOTATIONS {
            if let Some(value) = annotations.get_mut(*key) {
                *value = json!(REDACTED);
            }
        }
    }
}

fn find_named<'a>(items: Option<&'a Value>, name: &str) -> Option<&'a Value> {
    items?
        .as_array()?
        .iter()
        .find(|item| item.get("name").and_then(Value::as_str) == Some(name))
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn required_name(args: &Value) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or("name is required")?;
    validate_segment(name, "name")?;
    Ok(name.to_string())
}

fn validate_segment(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 253
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_'))
    {
        return Err(format!("{field} contains invalid characters"));
    }
    Ok(())
}

/// The message of a failed request. Only a Kubernetes `Status` object's message is
/// passed on (RBAC and not-found errors stay readable); anything else is described by
/// its status code alone, so a misdirected `apiServer` cannot be used to read arbitrary
/// responses back through the connection test (#321).
fn error_message(status: reqwest::StatusCode, body: &Value) -> String {
    match (
        body.get("kind").and_then(Value::as_str),
        body.get("message").and_then(Value::as_str),
    ) {
        (Some("Status"), Some(message)) => format!("Kubernetes returned {status}: {message}"),
        _ => format!("{status} from the API server, which is not a Kubernetes API response"),
    }
}

/// Read at most the last `max_bytes` of a response without holding the whole body, and
/// cut at a character boundary (#307). A plain byte slice of a large log could split a
/// multi-byte character and panic.
async fn read_tail(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<(String, bool), String> {
    let mut buffer: Vec<u8> = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("could not read pod logs: {error}"))?
    {
        buffer.extend_from_slice(&chunk);
        if buffer.len() > max_bytes.saturating_mul(2).max(64 * 1024) {
            let excess = buffer.len() - max_bytes;
            buffer.drain(..excess);
            truncated = true;
        }
    }
    if buffer.len() > max_bytes {
        let excess = buffer.len() - max_bytes;
        buffer.drain(..excess);
        truncated = true;
    }
    Ok((tail_text(&buffer), truncated))
}

/// Text of a byte tail that may start in the middle of a UTF-8 character: skip the
/// partial character instead of producing a replacement character or panicking.
fn tail_text(bytes: &[u8]) -> String {
    // A UTF-8 character has at most three continuation bytes after its first byte.
    let start = bytes
        .iter()
        .take(3)
        .take_while(|byte| is_continuation(**byte))
        .count();
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

fn is_continuation(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_kubernetes_status_messages_are_passed_on() {
        let status = reqwest::StatusCode::FORBIDDEN;
        let rbac = json!({ "kind": "Status", "message": "namespaces is forbidden" });
        assert_eq!(
            error_message(status, &rbac),
            "Kubernetes returned 403 Forbidden: namespaces is forbidden"
        );
        let foreign = json!({ "secret": "value from some other service" });
        let message = error_message(status, &foreign);
        assert!(!message.contains("secret"), "{message}");
        assert!(message.contains("not a Kubernetes API response"));
    }

    #[tokio::test]
    async fn loopback_api_server_is_refused_before_connecting() {
        let config = json!({ "authMode": "service_account", "apiServer": "https://127.0.0.1:1" });
        let client =
            KubernetesClientConfig::from_connection(&config, Some("token".into())).unwrap();
        let error = client.test_connection().await.unwrap_err();
        assert!(error.starts_with("request refused"), "{error}");
    }

    #[test]
    fn a_log_tail_never_splits_a_character() {
        // "é" is two bytes; cutting after its first byte must not panic or garble.
        let bytes = "aé€b".as_bytes();
        for start in 0..bytes.len() {
            let text = tail_text(&bytes[start..]);
            assert!(!text.contains('\u{FFFD}'), "{start}: {text:?}");
        }
        assert_eq!(tail_text("€b".as_bytes()), "€b");
        assert_eq!(tail_text(&"€b".as_bytes()[1..]), "b");
    }

    #[test]
    fn service_account_requires_https_server() {
        let config = json!({ "authMode": "service_account", "apiServer": "http://cluster" });
        assert!(KubernetesClientConfig::from_connection(&config, Some("token".into())).is_err());
    }

    #[test]
    fn kubeconfig_rejects_exec_and_insecure_tls() {
        let exec = r#"
current-context: c
contexts:
  - name: c
    context: { cluster: k, user: u }
clusters:
  - name: k
    cluster: { server: https://cluster }
users:
  - name: u
    user: { exec: { command: aws } }
"#;
        assert!(KubernetesClientConfig::from_connection(
            &json!({ "authMode": "kubeconfig" }),
            Some(exec.into())
        )
        .is_err());

        let insecure = exec
            .replace("exec: { command: aws }", "token: abc")
            .replace(
                "cluster: { server: https://cluster }",
                "cluster: { server: https://cluster, insecure-skip-tls-verify: true }",
            );
        assert!(KubernetesClientConfig::from_connection(
            &json!({ "authMode": "kubeconfig" }),
            Some(insecure)
        )
        .is_err());
    }

    #[test]
    fn kubeconfig_accepts_client_certificate_data() {
        use base64::Engine;
        let b64 = |value: &[u8]| base64::engine::general_purpose::STANDARD.encode(value);
        // The parser only validates the PEM structure when building the HTTP client, so a
        // dummy cert/key is enough to prove the auth mode is accepted.
        let kubeconfig = format!(
            r#"
current-context: c
contexts:
  - name: c
    context: {{ cluster: k, user: u }}
clusters:
  - name: k
    cluster: {{ server: https://cluster }}
users:
  - name: u
    user:
      client-certificate-data: {}
      client-key-data: {}
"#,
            b64(b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n"),
            b64(b"-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n"),
        );
        let parsed = KubernetesClientConfig::from_connection(
            &json!({ "authMode": "kubeconfig" }),
            Some(kubeconfig),
        )
        .expect("client-cert kubeconfig is accepted");
        assert!(parsed.identity_pem.is_some());
    }

    #[test]
    fn namespace_and_resource_allowlists_are_enforced() {
        let config = json!({
            "authMode": "service_account",
            "apiServer": "https://cluster",
            "namespaces": ["payments"],
            "defaultNamespace": "payments",
            "allowedResources": ["pods"]
        });
        let parsed =
            KubernetesClientConfig::from_connection(&config, Some("token".into())).unwrap();
        assert!(parsed.namespace(&json!({})).is_ok());
        assert!(parsed.namespace(&json!({ "namespace": "other" })).is_err());
        assert!(parsed.resource_allowed("pods"));
        assert!(!parsed.resource_allowed("events"));
    }

    #[tokio::test]
    async fn write_capabilities_need_their_resource_allowlist() {
        let parsed = connection(json!({
            "namespaces": ["payments"],
            "allowedResources": ["pods"]
        }));
        let error = parsed
            .invoke(
                "deployments.restart",
                &json!({ "namespace": "payments", "name": "api" }),
            )
            .await
            .unwrap_err();
        assert!(error.contains("resource 'deployments'"), "{error}");
    }

    #[test]
    fn wildcard_namespaces_follow_the_kubeconfig() {
        let parsed = connection(json!({ "namespaces": ["*"] }));
        assert!(parsed
            .namespace(&json!({ "namespace": "kube-system" }))
            .is_ok());
        assert!(parsed
            .namespace(&json!({ "namespace": "payments" }))
            .is_ok());
        assert!(parsed.all_namespaces());
    }

    fn connection(extra: Value) -> KubernetesClientConfig {
        let mut config = json!({ "authMode": "service_account", "apiServer": "https://cluster" });
        config
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        KubernetesClientConfig::from_connection(&config, Some("token".into())).unwrap()
    }

    #[test]
    fn empty_allowlists_deny() {
        let open = connection(json!({}));
        assert!(open
            .namespace(&json!({ "namespace": "kube-system" }))
            .is_err());
        assert!(open.allowed_namespaces().is_empty());
        for resource in ["namespaces", "pods", "pods/log", "deployments", "events"] {
            assert!(!open.resource_allowed(resource), "{resource}");
        }

        let default_only = connection(json!({ "defaultNamespace": "payments" }));
        assert!(default_only.namespace(&json!({})).is_ok());
        assert!(default_only
            .namespace(&json!({ "namespace": "payments" }))
            .is_ok());
        assert!(default_only
            .namespace(&json!({ "namespace": "kube-system" }))
            .is_err());
        assert_eq!(default_only.allowed_namespaces(), vec!["payments"]);

        let wildcard = connection(json!({ "allowedResources": ["*"] }));
        assert!(wildcard.resource_allowed("events"));
    }

    #[tokio::test]
    async fn empty_allowlists_refuse_before_any_request() {
        // Nothing listens on this address; a refusal must come from the allowlist.
        let client = connection(json!({ "apiServer": "https://127.0.0.1:9" }));
        let error = client
            .invoke("pods.list", &json!({ "namespace": "default" }))
            .await
            .unwrap_err();
        assert!(error.contains("allowedResources"), "{error}");

        let client = connection(json!({
            "apiServer": "https://127.0.0.1:9",
            "allowedResources": ["pods"]
        }));
        let error = client
            .invoke(
                "pods.get",
                &json!({ "namespace": "default", "name": "web" }),
            )
            .await
            .unwrap_err();
        assert!(error.contains("not allowed on this connection"), "{error}");
    }

    #[test]
    fn pod_and_deployment_env_values_are_redacted() {
        let container = json!({
            "name": "app",
            "env": [
                { "name": "DATABASE_PASSWORD", "value": "hunter2" },
                { "name": "TOKEN", "valueFrom": { "secretKeyRef": { "name": "app", "key": "token" } } }
            ]
        });
        let pod_spec = json!({
            "containers": [container.clone()],
            "initContainers": [container.clone()],
            "ephemeralContainers": [container]
        });
        let annotations = json!({
            "kubectl.kubernetes.io/last-applied-configuration": "{\"env\":\"hunter2\"}",
            "team": "payments"
        });
        let mut pod = json!({
            "metadata": { "name": "web", "annotations": annotations.clone() },
            "spec": pod_spec.clone()
        });
        redact_object(&mut pod, "/spec");
        let mut deployments = json!({ "items": [{
            "metadata": { "name": "web", "annotations": annotations },
            "spec": { "template": { "spec": pod_spec } }
        }] });
        redact_items(&mut deployments, "/spec/template/spec");

        for (object, spec) in [
            (&pod, "/spec"),
            (&deployments["items"][0], "/spec/template/spec"),
        ] {
            let text = object.to_string();
            assert!(!text.contains("hunter2"), "{text}");
            for list in ["containers", "initContainers", "ephemeralContainers"] {
                let env = object.pointer(&format!("{spec}/{list}/0/env")).unwrap();
                assert_eq!(env[0]["name"], "DATABASE_PASSWORD");
                assert_eq!(env[0]["value"], REDACTED);
                assert_eq!(env[1]["valueFrom"]["secretKeyRef"]["key"], "token");
            }
            assert_eq!(object["metadata"]["annotations"]["team"], "payments");
        }
    }
}
