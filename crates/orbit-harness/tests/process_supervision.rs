//! Adapter-level behaviour against shell-script stub harnesses (Unix only).
//!
//! These cover what the unit tests cannot: that both adapters report the right outcome
//! for a misbehaving harness, never leave the harness' process group running, and never
//! write through symlinks shipped by a cloned repository. They do not replace the
//! `#[ignore]`d tests against the real CLIs.
#![cfg(unix)]

use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;
use std::time::Duration;

use orbit_domain::ExecutionStatus;
use orbit_harness::testing::execution_request;
use orbit_harness::{ExecutionOutcome, HarnessAdapter, OpenCodeHarnessAdapter, PiHarnessAdapter};

/// Writing an executable while another test forks can fail the exec with ETXTBSY, so the
/// stub-based tests run one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn write_stub(dir: &Path, body: &str) -> String {
    let path = dir.join("stub-harness.sh");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo stub-1.0; exit 0; fi\n{body}\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.to_string_lossy().into_owned()
}

async fn run_opencode(stub_body: &str, workspace: &Path, stub_dir: &Path) -> ExecutionOutcome {
    let adapter = OpenCodeHarnessAdapter;
    let mut request = execution_request(&adapter, "do the thing");
    request.workspace.root = workspace.to_string_lossy().into_owned();
    request.harness_config = serde_json::json!({ "binary": write_stub(stub_dir, stub_body) });
    adapter.execute(request).await.unwrap()
}

#[tokio::test]
async fn opencode_error_event_with_exit_zero_fails_the_run() {
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let outcome = run_opencode(
        r#"echo '{"type":"error","error":{"message":"invalid api key"}}'"#,
        ws.path(),
        dir.path(),
    )
    .await;
    assert_eq!(outcome.status, ExecutionStatus::Failed);
    assert!(outcome.output.is_none());
    assert!(outcome.error.unwrap().message.contains("invalid api key"));
}

#[tokio::test]
async fn opencode_error_followed_by_an_answer_is_a_recovered_retry() {
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let outcome = run_opencode(
        r#"echo '{"type":"error","error":{"message":"rate limited, retrying"}}'
echo '{"type":"text","sessionID":"ses_1","part":{"text":"the answer"}}'"#,
        ws.path(),
        dir.path(),
    )
    .await;
    assert_eq!(outcome.status, ExecutionStatus::Succeeded);
    assert_eq!(outcome.output.unwrap()["text"], "the answer");
    assert_eq!(outcome.session_ref.as_deref(), Some("ses_1"));
}

#[tokio::test]
async fn opencode_does_not_write_or_read_through_workspace_symlinks() {
    let _serial = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let victim = outside.path().join("victim");
    std::fs::write(&victim, "keep me").unwrap();
    let config_victim = outside.path().join("config.json");
    std::fs::write(&config_victim, r#"{"leak":"yes"}"#).unwrap();
    symlink(&victim, ws.path().join("AGENTS.md")).unwrap();
    symlink(&config_victim, ws.path().join("opencode.json")).unwrap();
    // A symlinked directory is refused outright.
    symlink(outside.path(), ws.path().join(".opencode")).unwrap();

    let adapter = OpenCodeHarnessAdapter;
    let mut request = execution_request(&adapter, "go");
    request.workspace.root = ws.path().to_string_lossy().into_owned();
    request.harness_config = serde_json::json!({
        "binary": write_stub(dir.path(), "echo '{\"type\":\"text\",\"part\":{\"text\":\"ok\"}}'"),
        "orbitMcp": { "command": ["orbit-worker", "mcp-broker"] },
    });
    let result = adapter.execute(request).await;

    assert!(result.is_err(), "symlinked .opencode must be refused");
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
    assert_eq!(
        std::fs::read_to_string(&config_victim).unwrap(),
        r#"{"leak":"yes"}"#
    );
    assert_eq!(
        std::fs::read_dir(outside.path()).unwrap().count(),
        2,
        "nothing may be created outside the workspace"
    );
    let agents = std::fs::read_to_string(ws.path().join("AGENTS.md")).unwrap();
    assert!(!agents.contains("keep me"));
    let config = std::fs::read_to_string(ws.path().join("opencode.json")).unwrap();
    assert!(!config.contains("leak"));
}

/// Wait for the stub's background pid file.
async fn background_pid(dir: &Path) -> i32 {
    for _ in 0..100 {
        if let Ok(text) = std::fs::read_to_string(dir.join("pid")) {
            if let Ok(pid) = text.trim().parse() {
                return pid;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("stub never wrote its background pid");
}

async fn assert_gone(pid: i32) {
    for _ in 0..60 {
        // SAFETY: signal 0 only probes for existence.
        if unsafe { libc::kill(pid, 0) } != 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    unsafe { libc::kill(pid, libc::SIGKILL) };
    panic!("process {pid} survived the end of the execution");
}

const FORKING_BODY: &str =
    "sleep 600 &\necho $! > pid\necho '{\"type\":\"text\",\"sessionID\":\"ses_9\",\"part\":{\"text\":\"hi\"}}'\nwait";

async fn cancel_case(adapter: &dyn HarnessAdapter, ws: &Path, stub_dir: &Path, name: &str) {
    let mut request = execution_request(adapter, "go");
    request.workspace.root = ws.to_string_lossy().into_owned();
    request.harness_config = serde_json::json!({ "binary": write_stub(stub_dir, FORKING_BODY) });
    let cancel = request.cancel.clone();
    let task = tokio::spawn({
        let ws = ws.to_path_buf();
        async move {
            // The stub runs with the workspace as its cwd, so the pid file lands there.
            let pid = background_pid(&ws).await;
            cancel.cancel();
            pid
        }
    });
    let outcome = adapter.execute(request).await.unwrap();
    let pid = task.await.unwrap();
    assert_eq!(outcome.status, ExecutionStatus::Cancelled, "{name}");
    assert_gone(pid).await;
}

#[tokio::test]
async fn cancel_leaves_no_process_from_the_harness_group() {
    let _serial = SERIAL.lock().await;
    let stubs = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    cancel_case(&OpenCodeHarnessAdapter, ws.path(), stubs.path(), "opencode").await;
    let ws = tempfile::tempdir().unwrap();
    cancel_case(&PiHarnessAdapter, ws.path(), stubs.path(), "pi").await;
}

#[tokio::test]
async fn timeout_leaves_no_process_from_the_harness_group() {
    let _serial = SERIAL.lock().await;
    for adapter in [
        Box::new(OpenCodeHarnessAdapter) as Box<dyn HarnessAdapter>,
        Box::new(PiHarnessAdapter),
    ] {
        let stubs = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let mut request = execution_request(adapter.as_ref(), "go");
        request.workspace.root = ws.path().to_string_lossy().into_owned();
        request.resources.timeout_seconds = 1;
        request.harness_config =
            serde_json::json!({ "binary": write_stub(stubs.path(), FORKING_BODY) });
        let outcome = adapter.execute(request).await.unwrap();
        assert_eq!(outcome.status, ExecutionStatus::TimedOut);
        assert_gone(background_pid(ws.path()).await).await;
    }
}

#[tokio::test]
async fn pi_error_with_exit_zero_still_fails_the_run() {
    let _serial = SERIAL.lock().await;
    let stubs = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let adapter = PiHarnessAdapter;
    let mut request = execution_request(&adapter, "go");
    request.workspace.root = ws.path().to_string_lossy().into_owned();
    request.harness_config = serde_json::json!({ "binary": write_stub(
        stubs.path(),
        r#"echo '{"type":"message_end","message":{"role":"assistant","stopReason":"error","errorMessage":"no credits"}}'"#,
    ) });
    let outcome = adapter.execute(request).await.unwrap();
    assert_eq!(outcome.status, ExecutionStatus::Failed);
    assert!(outcome.error.unwrap().message.contains("no credits"));
}
