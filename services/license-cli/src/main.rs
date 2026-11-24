//! Vendor-side offline license tool.
//!
//! `keygen` creates an Ed25519 signing key; keep the private key offline. `sign` turns a
//! claims JSON document into a compact JWS license. The runtime only needs the public key
//! (embedded by `kid` or supplied via `ORBIT_LICENSE_PUBLIC_KEYS`).

use std::io::Write;
use std::path::PathBuf;

use anyhow::{bail, Context};
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct KeyFile {
    kid: String,
    /// base64url raw 32-byte private key. Keep this file offline and never commit it.
    private_key: String,
    /// base64url raw 32-byte public key, for `ORBIT_LICENSE_PUBLIC_KEYS` / embedding.
    public_key: String,
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keygen") => keygen(&args[1..]),
        Some("public-key") => public_key(&args[1..]),
        Some("sign") => sign(&args[1..]),
        _ => {
            eprintln!(
                "usage:\n  orbit-license-cli keygen --out key.json [--kid orbit-2026-01]\n  orbit-license-cli public-key --key key.json\n  orbit-license-cli sign --key key.json --claims claims.json [--out license.jwt]"
            );
            std::process::exit(2);
        }
    }
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|window| window[0] == name)
        .map(|window| window[1].clone())
}

fn keygen(args: &[String]) -> anyhow::Result<()> {
    let out = arg(args, "--out").context("--out is required")?;
    let kid = arg(args, "--kid").unwrap_or_else(|| "orbit-2026-01".into());
    let signing = SigningKey::generate(&mut OsRng);
    let key = KeyFile {
        kid,
        private_key: encode(&signing.to_bytes()),
        public_key: encode(&signing.verifying_key().to_bytes()),
    };
    let mut file = std::fs::File::create(&out).context("create key file")?;
    file.write_all(serde_json::to_string_pretty(&key)?.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o600))?;
    }
    println!("wrote {out}");
    println!("kid: {}", key.kid);
    println!("public key (base64url): {}", key.public_key);
    println!("Keep the private key offline; only embed/share the public key.");
    Ok(())
}

fn public_key(args: &[String]) -> anyhow::Result<()> {
    let key = read_key(args)?;
    println!("{}", serde_json::json!({ key.kid.clone(): key.public_key }));
    Ok(())
}

fn sign(args: &[String]) -> anyhow::Result<()> {
    let key = read_key(args)?;
    let claims_path = arg(args, "--claims").context("--claims is required")?;
    let claims: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&claims_path).context("read claims")?)
            .context("claims must be valid JSON")?;
    if claims.get("v").and_then(|value| value.as_u64()) != Some(1) {
        bail!("claims must contain \"v\": 1");
    }
    for field in ["issued_at", "expires_at"] {
        if claims.get(field).and_then(|value| value.as_str()).is_none() {
            bail!("claims must contain {field} (RFC3339)");
        }
    }

    let private = decode(&key.private_key)?;
    let private: [u8; 32] = private
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid private key"))?;
    let signing = SigningKey::from_bytes(&private);
    let header = serde_json::json!({ "alg": "EdDSA", "kid": key.kid });
    let header = encode(&serde_json::to_vec(&header)?);
    let payload = encode(&serde_json::to_vec(&claims)?);
    let signing_input = format!("{header}.{payload}");
    let signature = signing.sign(signing_input.as_bytes());
    let token = format!("{signing_input}.{}", encode(&signature.to_bytes()));

    match arg(args, "--out") {
        Some(out) => {
            std::fs::write(&out, format!("{token}\n")).context("write license")?;
            println!("wrote {out}");
        }
        None => println!("{token}"),
    }
    Ok(())
}

fn read_key(args: &[String]) -> anyhow::Result<KeyFile> {
    let path: PathBuf = arg(args, "--key").context("--key is required")?.into();
    serde_json::from_str(&std::fs::read_to_string(&path).context("read key file")?)
        .context("key file is invalid")
}

fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn decode(value: &str) -> anyhow::Result<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value.trim())
        .context("invalid base64url value")
}
