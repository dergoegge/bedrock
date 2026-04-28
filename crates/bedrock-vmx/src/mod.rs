// SPDX-License-Identifier: GPL-2.0

//! Re-export for use as a submodule in kernel builds.

// In kernel builds, pub items in private modules trigger warnings.
// These items are pub for the cargo build's public API.
// Some items are only used in tests or by external crates, not the kernel.
#![allow(unreachable_pub, dead_code, unused_imports, unused_assignments)]

mod compat;
mod cow;
mod decoder;
mod devices;
mod exits;
mod fields;
mod handler;
mod host;
mod hypercalls;
mod logging;
mod prelude;
pub mod registers;
mod timing;
pub mod traits;
pub mod vm;
pub mod vm_state;

pub use fields::{VmcsField16, VmcsField32, VmcsField64, VmcsFieldNatural};
pub use handler::{BedrockHandler, VmEntry, VmRef};
pub use traits::{
    InveptError, InvvpidError, VirtualMachineControlStructure, VmEntryError, VmRunner,
    VmcsReadError, VmcsReadResult, VmcsWriteError, VmcsWriteResult, Vmx, VmxCapabilities,
    VmxContext, VmxInitError, VmxoffError, VmxonError,
};

// VM implementation
pub use cow::CowPageMap;
pub use vm::{ForkableVm, ForkedVm, ParentVm, RootVm};
pub use vm_state::{LogMode, VmState, MAX_FEEDBACK_BUFFERS, PERIODIC_EXIT_MARGIN};

// Exit reasons
pub use exits::ExitReason;

// Device emulation types
pub use devices::RdrandMode;
