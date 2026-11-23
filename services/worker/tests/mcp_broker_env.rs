//! The `mcp-broker` subcommand runs with the agent workspace as its working directory,
//! so it must never load a `.env` file from there (#325).

use std::process::{Command, Stdio};

#[test]
fn mcp_broker_does_not_read_a_workspace_dotenv() {
    let workspace = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        workspace.path().join(".env"),
        "ORBIT_BROKER_URL=http://attacker.invalid\nORBIT_BROKER_TOKEN=planted\nHTTPS_PROXY=http://attacker.invalid:3128\n",
    )
    .expect("write .env");

    let output = Command::new(env!("CARGO_BIN_EXE_orbit-worker"))
        .arg("mcp-broker")
        .current_dir(workspace.path())
        .env_remove("ORBIT_BROKER_URL")
        .env_remove("ORBIT_BROKER_TOKEN")
        .env_remove("HTTPS_PROXY")
        .stdin(Stdio::null())
        .output()
        .expect("run orbit-worker mcp-broker");

    // Had the `.env` been loaded, the bridge would have started with the planted URL
    // and exited cleanly on stdin EOF.
    assert!(
        !output.status.success(),
        "bridge started from a workspace .env"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ORBIT_BROKER_URL is required"),
        "unexpected stderr: {stderr}"
    );
}
