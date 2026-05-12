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

/// Register a single 4KB page as the PEBS scratch page for precise VM exits.
///
/// The page must be:
/// - Writable in the guest's page tables (so PEBS writes don't take a guest #PF).
/// - Quiescent — the guest must agree not to read or write to it. Typical use:
///   `mmap` an anonymous page in userspace and `mlock` it; the kernel direct-map
///   alias of that page is then used by the hypervisor as both DS Management
///   Area and PEBS Buffer.
///
/// Inputs:
/// - RBX: Guest virtual address of the scratch page (must be 4KB-aligned).
///
/// Outputs:
/// - RAX: 0 on success, -1 on failure (translation failed, unaligned address,
///   capability missing on host CPU, or already registered).
///
/// On success, the hypervisor:
/// 1. Walks guest page tables to translate RBX to its guest physical address.
/// 2. Populates the DS Management Area at the start of that page (via the
///    host's EPT-mapped view).
/// 3. Remaps the page in EPT as R+E (no W). The next time the PEBS engine
///    attempts to write a record, an EPT violation fires and the precise-exit
///    handler runs.
/// 4. Stores `PebsState` in `VmState` so the APIC-timer precise-exit path
///    knows where to direct PEBS.
pub const HYPERCALL_REGISTER_PEBS_PAGE: u64 = 3;
