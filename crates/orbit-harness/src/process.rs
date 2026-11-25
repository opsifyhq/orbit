//! Shared supervision of a harness subprocess.
//!
//! Both CLI adapters (OpenCode, Pi) spawn the harness in its own process group and feed
//! its stdout lines to a harness-specific parser. Everything else lives here so the two
//! cannot diverge again: group-wide SIGTERM -> grace -> SIGKILL on cancel and timeout, a
//! group SIGKILL whenever the run ends (descendants such as bash tool children or an MCP
//! bridge must not outlive the execution), a bounded wait after stdout EOF, bounded
//! stderr capture and a drop guard for the case where the caller's future is dropped.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::process::ExitStatus;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{Stream, StreamExt};
use orbit_contracts::{HarnessError, HarnessErrorKind, HarnessEvent};
use serde_json::{json, Value};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

use crate::stream;

/// SIGTERM grace before the group is SIGKILLed.
const TERM_GRACE: Duration = Duration::from_secs(3);
/// How long a harness may take to exit after closing stdout.
const EXIT_AFTER_EOF: Duration = Duration::from_secs(10);
/// How long buffered output is drained after the leader exited.
const DRAIN: Duration = Duration::from_secs(2);
const STDERR_BUFFER_BYTES: usize = 16_000;

pub(crate) type EventSender = Option<tokio::sync::mpsc::Sender<HarnessEvent>>;

pub(crate) async fn emit(sender: &EventSender, event: HarnessEvent) {
    if let Some(sender) = sender {
        let _ = sender.send(event).await;
    }
}

pub(crate) fn truncate(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        value.to_string()
    } else {
        let mut end = limit;
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}… [truncated]", &value[..end])
    }
}

pub(crate) fn truncate_json(value: &Value, limit: usize) -> Value {
    let Ok(serialized) = serde_json::to_string(value) else {
        return json!("[unserializable JSON]");
    };
    if serialized.len() <= limit {
        value.clone()
    } else {
        json!(truncate(&serialized, limit))
    }
}

/// What the next step of a supervised run produced.
pub(crate) enum Next {
    Line(Result<String, io::Error>),
    /// stdout closed (or the drain after the leader's exit finished).
    Eof,
    /// Cancellation was observed; the process group has already been stopped.
    Cancelled,
    /// The active-time deadline passed; the process group has already been stopped.
    TimedOut,
}

pub(crate) enum Ending {
    Exited(ExitStatus),
    Cancelled,
    TimedOut,
}

pub(crate) struct Finished {
    pub ending: Ending,
    pub stderr: String,
}

type LineStream = Pin<Box<dyn Stream<Item = Result<String, io::Error>> + Send>>;

pub(crate) struct SupervisedChild {
    child: Child,
    /// Process group id (the leader's pid); `None` once the group has been cleaned up.
    group: Option<i32>,
    lines: LineStream,
    stderr_task: tokio::task::JoinHandle<()>,
    stderr: Arc<Mutex<String>>,
    cancel: CancellationToken,
    deadline: Pin<Box<dyn Future<Output = ()> + Send>>,
    exit: Option<io::Result<ExitStatus>>,
    ended: Option<Ending>,
    label: &'static str,
}

fn signal_group(group: i32, signal: i32) {
    #[cfg(unix)]
    // SAFETY: plain kill(2) on a process group this module created.
    unsafe {
        libc::kill(-group, signal);
    }
    #[cfg(not(unix))]
    let _ = (group, signal);
}

impl SupervisedChild {
    /// Spawn `command` in its own process group with piped stdout/stderr.
    ///
    /// `stderr_event` maps each stderr line to an optional timeline event (debug logs);
    /// stderr is always captured, bounded, for failure messages.
    pub(crate) fn spawn(
        mut command: Command,
        label: &'static str,
        cancel: CancellationToken,
        deadline: impl Future<Output = ()> + Send + 'static,
        events: EventSender,
        stderr_event: impl Fn(&str) -> Option<HarnessEvent> + Send + 'static,
    ) -> Result<Self, HarnessError> {
        command.stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        command.kill_on_drop(true);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.as_std_mut().process_group(0);
        }

        let mut child = command.spawn().map_err(|e| {
            HarnessError::new(
                HarnessErrorKind::Unavailable,
                format!(
                    "could not start {label} binary {:?}: {e}. The worker image must include the {label} CLI.",
                    command.as_std().get_program()
                ),
            )
        })?;
        let unavailable = |what: &str| {
            HarnessError::new(
                HarnessErrorKind::Internal,
                format!("{label} {what} unavailable"),
            )
        };
        let group = child.id().map(|pid| pid as i32);
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            if let Some(group) = group {
                signal_group(group, libc::SIGKILL);
            }
            return Err(unavailable("stdout/stderr"));
        };

        let buffer = Arc::new(Mutex::new(String::new()));
        let stderr_task = {
            let buffer = buffer.clone();
            tokio::spawn(async move {
                let lines = stream::lines(stderr);
                tokio::pin!(lines);
                while let Some(line) = lines.next().await {
                    let line = line.unwrap_or_else(|e| format!("[skipped oversized line: {e}]"));
                    {
                        let mut buffer = buffer.lock().expect("stderr lock");
                        if buffer.len() < STDERR_BUFFER_BYTES {
                            buffer.push_str(&line);
                            buffer.push('\n');
                        }
                    }
                    if let Some(event) = stderr_event(&line) {
                        emit(&events, event).await;
                    }
                }
            })
        };

        Ok(Self {
            child,
            group,
            lines: Box::pin(stream::lines(stdout)),
            stderr_task,
            stderr: buffer,
            cancel,
            deadline: Box::pin(deadline),
            exit: None,
            ended: None,
            label,
        })
    }

    /// Next stdout line, or the reason the run is over. Cancellation and the deadline
    /// win over pending output; the group is stopped before they are reported.
    pub(crate) async fn next(&mut self) -> Next {
        loop {
            if self.exit.is_some() {
                // The leader is gone and its group was killed: drain what is buffered.
                return match tokio::time::timeout(DRAIN, self.lines.next()).await {
                    Ok(Some(line)) => Next::Line(line),
                    _ => Next::Eof,
                };
            }
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => {
                    self.stop().await;
                    self.ended = Some(Ending::Cancelled);
                    return Next::Cancelled;
                }
                _ = &mut self.deadline => {
                    self.stop().await;
                    self.ended = Some(Ending::TimedOut);
                    return Next::TimedOut;
                }
                line = self.lines.next() => {
                    return match line {
                        Some(line) => Next::Line(line),
                        None => Next::Eof,
                    };
                }
                status = self.child.wait() => {
                    // Leader exited while stdout may still be held open by descendants.
                    self.exit = Some(status);
                    self.kill_group();
                }
            }
        }
    }

    /// SIGTERM the whole group, wait `TERM_GRACE` for the leader, then SIGKILL the group.
    async fn stop(&mut self) {
        if let Some(group) = self.group {
            signal_group(group, libc::SIGTERM);
        }
        if tokio::time::timeout(TERM_GRACE, self.child.wait())
            .await
            .is_err()
        {
            self.kill_group();
            let _ = tokio::time::timeout(TERM_GRACE, self.child.wait()).await;
        }
        self.kill_group();
    }

    /// SIGKILL every process left in the group. Idempotent.
    fn kill_group(&mut self) {
        if let Some(group) = self.group {
            signal_group(group, libc::SIGKILL);
        }
        let _ = self.child.start_kill();
    }

    /// End the run: bounded wait for the leader, kill whatever is left in its group and
    /// collect stderr. Always leaves no descendant of the harness running.
    pub(crate) async fn finish(mut self) -> Result<Finished, HarnessError> {
        let ending = match self.ended.take() {
            Some(ending) => ending,
            None => {
                if self.exit.is_none() {
                    let waited = tokio::time::timeout(EXIT_AFTER_EOF, self.child.wait()).await;
                    self.exit = Some(waited.unwrap_or_else(|_| {
                        Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!(
                                "did not exit within {}s after closing stdout",
                                EXIT_AFTER_EOF.as_secs()
                            ),
                        ))
                    }));
                }
                match self.exit.take() {
                    Some(Ok(status)) => Ending::Exited(status),
                    Some(Err(error)) => {
                        self.kill_group();
                        let _ = self.child.wait().await;
                        self.group = None;
                        return Err(HarnessError::new(
                            HarnessErrorKind::Internal,
                            format!("{} wait failed: {error}", self.label),
                        ));
                    }
                    None => unreachable!("exit recorded above"),
                }
            }
        };
        self.kill_group();
        self.group = None;
        let _ = tokio::time::timeout(DRAIN, &mut self.stderr_task).await;
        self.stderr_task.abort();
        let stderr = self.stderr.lock().expect("stderr lock").clone();
        Ok(Finished { ending, stderr })
    }
}

impl Drop for SupervisedChild {
    /// The caller's future was dropped mid-run: do not leave the group behind.
    fn drop(&mut self) {
        if self.group.is_some() {
            self.kill_group();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::*;

    /// Keeps the stub tests from overlapping each other's process groups and timings.
    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// The stub runs as `/bin/sh <script>` rather than exec-ing the script itself: exec-ing a
    /// file just written fails with ETXTBSY when any other test in the binary forks while
    /// the write descriptor is open, and `SERIAL` only covers this module.
    fn stub(dir: &Path, body: &str) -> Command {
        let path = dir.join("stub.sh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut command = Command::new("/bin/sh");
        command.arg(&path);
        command.current_dir(dir);
        command
    }

    fn spawn(command: Command, cancel: CancellationToken, deadline: Duration) -> SupervisedChild {
        SupervisedChild::spawn(
            command,
            "stub",
            cancel,
            tokio::time::sleep(deadline),
            None,
            |_| None,
        )
        .unwrap()
    }

    /// Wait for the stub's background `sleep` pid file, then return the pid.
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
            let alive = unsafe { libc::kill(pid, 0) } == 0;
            if !alive {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // SAFETY: do not leak the sleeper if the assertion fails.
        unsafe { libc::kill(pid, libc::SIGKILL) };
        panic!("process {pid} from the harness group survived");
    }

    const FORKING_STUB: &str = "sleep 600 &\necho $! > pid\necho started\nwait";

    #[tokio::test]
    async fn cancel_kills_the_whole_group_and_reports_cancelled() {
        let _serial = SERIAL.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let mut run = spawn(
            stub(dir.path(), FORKING_STUB),
            cancel.clone(),
            Duration::from_secs(60),
        );
        assert!(matches!(run.next().await, Next::Line(Ok(line)) if line == "started"));
        let pid = background_pid(dir.path()).await;
        cancel.cancel();
        assert!(matches!(run.next().await, Next::Cancelled));
        let finished = run.finish().await.unwrap();
        assert!(matches!(finished.ending, Ending::Cancelled));
        assert_gone(pid).await;
    }

    #[tokio::test]
    async fn timeout_kills_the_whole_group_even_when_term_is_ignored() {
        let _serial = SERIAL.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let mut run = spawn(
            stub(dir.path(), &format!("trap '' TERM\n{FORKING_STUB}")),
            CancellationToken::new(),
            Duration::from_millis(500),
        );
        assert!(matches!(run.next().await, Next::Line(Ok(_))));
        let pid = background_pid(dir.path()).await;
        assert!(matches!(run.next().await, Next::TimedOut));
        let finished = run.finish().await.unwrap();
        assert!(matches!(finished.ending, Ending::TimedOut));
        assert_gone(pid).await;
    }

    #[tokio::test]
    async fn normal_exit_kills_background_children_holding_stdout_open() {
        let _serial = SERIAL.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let mut run = spawn(
            stub(dir.path(), "sleep 600 &\necho $! > pid\necho done\nexit 0"),
            CancellationToken::new(),
            Duration::from_secs(60),
        );
        let mut lines = Vec::new();
        loop {
            match run.next().await {
                Next::Line(Ok(line)) => lines.push(line),
                Next::Eof => break,
                _ => panic!("unexpected ending"),
            }
        }
        assert_eq!(lines, ["done"]);
        let pid = background_pid(dir.path()).await;
        let finished = run.finish().await.unwrap();
        assert!(matches!(finished.ending, Ending::Exited(s) if s.success()));
        assert_gone(pid).await;
    }

    #[tokio::test]
    async fn dropping_the_run_kills_the_group() {
        let _serial = SERIAL.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let mut run = spawn(
            stub(dir.path(), FORKING_STUB),
            CancellationToken::new(),
            Duration::from_secs(60),
        );
        assert!(matches!(run.next().await, Next::Line(Ok(_))));
        let pid = background_pid(dir.path()).await;
        drop(run);
        assert_gone(pid).await;
    }

    #[tokio::test]
    async fn stderr_is_captured_for_failures() {
        let _serial = SERIAL.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let mut run = spawn(
            stub(dir.path(), "echo boom >&2\nexit 3"),
            CancellationToken::new(),
            Duration::from_secs(60),
        );
        while !matches!(run.next().await, Next::Eof) {}
        let finished = run.finish().await.unwrap();
        assert!(matches!(finished.ending, Ending::Exited(s) if s.code() == Some(3)));
        assert_eq!(finished.stderr.trim(), "boom");
    }
}
