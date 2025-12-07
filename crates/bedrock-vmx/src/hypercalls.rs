// SPDX-License-Identifier: GPL-2.0

//! Hypercall numbers for the bedrock hypervisor.
//!
//! Guest code invokes hypercalls via the VMCALL instruction with the
//! hypercall number in RAX.

/// Shutdown the VM cleanly.
pub const HYPERCALL_SHUTDOWN: u64 = 0;

/// Trigger a snapshot.
/// Exits to userspace and logs VM state if logging is enabled.
pub const HYPERCALL_SNAPSHOT: u64 = 1;

/// Register a feedback buffer for fuzzing.
///
/// Inputs:
/// - RBX: Guest virtual address of buffer
/// - RCX: Size of buffer in bytes
/// - RDX: Buffer index (0-15)
///
/// Outputs:
/// - RAX: 0 on success, -1 (0xFFFFFFFFFFFFFFFF) on failure
///
/// The buffer's GVA is translated to GPAs and stored in VmState
/// at the specified index for later mapping by host userspace.
/// Up to 16 feedback buffers can be registered per VM.
pub const HYPERCALL_REGISTER_FEEDBACK_BUFFER: u64 = 2;
