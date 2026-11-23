//! Observation hook for execution lifecycle transitions.
//!
//! Executions change state in many places: API handlers, the worker protocol and the
//! SQL-only reapers. The store is the one place every transition passes through, so it
//! reports them to an optional observer. `orbit-api` uses this to keep its metrics
//! complete without instrumenting each caller.
//!
//! Observers are called after the row is written (after commit, where a transaction is
//! used) and must be cheap and must not block: they run on the request path.

use std::sync::Arc;

use crate::models::ExecutionRecord;

/// Why an execution reached a terminal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishCause {
    /// A worker (or the control plane on its behalf) reported the result.
    Reported,
    /// A queued execution was cancelled before any worker took it.
    CancelledWhileQueued,
    /// Reaped: the worker's heartbeat expired.
    WorkerLost,
    /// Reaped: a worker took the execution but never started it.
    NeverStarted,
    /// Reaped: the execution ran past its deadline.
    TimedOut,
    /// Reaped: the execution's lease expired while its worker was still alive.
    HarnessUnresponsive,
}

impl FinishCause {
    /// Stable, low-cardinality label for reaped executions; `None` when not reaped.
    pub fn reaped_reason(self) -> Option<&'static str> {
        match self {
            FinishCause::WorkerLost => Some("worker_lost"),
            FinishCause::NeverStarted => Some("never_started"),
            FinishCause::TimedOut => Some("timed_out"),
            FinishCause::HarnessUnresponsive => Some("harness_unresponsive"),
            FinishCause::Reported | FinishCause::CancelledWhileQueued => None,
        }
    }
}

/// Receives execution lifecycle transitions. All methods default to no-ops.
pub trait ExecutionObserver: Send + Sync {
    /// A new execution row was inserted (not called for an idempotent replay).
    fn created(&self, _execution: &ExecutionRecord) {}
    /// The harness reported `running` for the first time (not called when a run resumes
    /// after an approval).
    fn started(&self, _execution: &ExecutionRecord) {}
    /// The execution reached a terminal state.
    fn finished(&self, _execution: &ExecutionRecord, _cause: FinishCause) {}
}

pub(crate) type SharedObserver = Option<Arc<dyn ExecutionObserver>>;
