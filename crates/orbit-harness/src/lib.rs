//! Harness adapters and the harness registry.
//!
//! See `docs/architecture/runtime-contract.md`. Adapters never touch PostgreSQL, HTTP or
//! the frontend. The control plane only stores their descriptors and capabilities; the
//! worker executes them.

pub mod mock;
pub mod opencode;
pub mod paths;
pub mod pi;
pub mod planned;
mod process;
mod process_env;
pub mod registry;
mod stream;
pub mod testing;

pub use mock::MockHarnessAdapter;
pub use opencode::OpenCodeHarnessAdapter;
pub use pi::PiHarnessAdapter;
pub use planned::DescriptorOnlyAdapter;
pub use registry::HarnessRegistry;

pub use orbit_contracts::{
    ExecutionOutcome, ExecutionRequest, HarnessAdapter, HarnessCapabilities, HarnessConfig,
    HarnessDescriptor, HarnessError, HarnessErrorKind, HarnessEvent, HarnessEventKind,
    HarnessHealth,
};
