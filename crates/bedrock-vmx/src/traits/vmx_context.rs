// SPDX-License-Identifier: GPL-2.0

//! VMX context structure for guest/host register switching.
//!
//! This structure is shared between Rust and assembly code, so its layout
//! must match the assembly code exactly.

/// VMX context for guest/host register switching.
///
/// This structure holds the guest and host general-purpose registers
/// that are saved/restored during VM entry and exit. The layout must
/// match the assembly code in vmx_support.S.
///
/// Note: RSP is not included because:
/// - Guest RSP is in the VMCS (loaded/saved automatically)
/// - Host RSP points to this structure during VM operation
#[repr(C)]
pub struct VmxContext {
    // Guest GPRs (offsets 0-112)
    pub guest_rax: u64,
    pub guest_rbx: u64,
    pub guest_rcx: u64,
    pub guest_rdx: u64,
    pub guest_rsi: u64,
    pub guest_rdi: u64,
    pub guest_rbp: u64,
    pub guest_r8: u64,
    pub guest_r9: u64,
    pub guest_r10: u64,
    pub guest_r11: u64,
    pub guest_r12: u64,
    pub guest_r13: u64,
    pub guest_r14: u64,
    pub guest_r15: u64,

    // Host GPRs (offsets 120-232)
    pub host_rax: u64,
    pub host_rbx: u64,
    pub host_rcx: u64,
    pub host_rdx: u64,
    pub host_rsi: u64,
    pub host_rdi: u64,
    pub host_rbp: u64,
    pub host_r8: u64,
    pub host_r9: u64,
    pub host_r10: u64,
    pub host_r11: u64,
    pub host_r12: u64,
    pub host_r13: u64,
    pub host_r14: u64,
    pub host_r15: u64,

    // Launch state: 0 = use VMLAUNCH, 1 = use VMRESUME (offset 240)
    pub launched: u32,

    // Padding for alignment (offset 244)
    pub _pad: u32,

    // XSAVE state pointers (offset 248, 256)
    // These point to 64-byte aligned XsaveArea structures for extended state
    // (FPU/SSE/AVX) save/restore during VM entry/exit.
    // Set to null (0) to skip XSAVE operations.
    pub guest_xsave_ptr: u64,
    pub host_xsave_ptr: u64,

    // XCR0 mask for XSAVE/XRSTOR (offset 264)
    // Specifies which state components to save/restore.
    // Common values: 0x7 (X87|SSE|AVX), 0xE7 (with AVX-512)
    // This is also the value that will be set in the hardware XCR0 register
    // during guest execution (so XGETBV returns this value).
    pub xcr0_mask: u64,

    // Host XCR0 value (offset 272)
    // Saved on VM entry, restored on VM exit.
    // This allows us to set a different XCR0 for the guest.
    pub host_xcr0: u64,

    // Guest CR2 value (offset 280)
    // CR2 is not part of the VMCS, so we must manually save/restore it.
    // This holds the guest's page-fault linear address.
    pub guest_cr2: u64,
}

impl Default for VmxContext {
    fn default() -> Self {
        Self::new()
    }
}

impl VmxContext {
    /// Create a new VmxContext with all registers zeroed.
    pub const fn new() -> Self {
        Self {
            guest_rax: 0,
            guest_rbx: 0,
            guest_rcx: 0,
            guest_rdx: 0,
            guest_rsi: 0,
            guest_rdi: 0,
            guest_rbp: 0,
            guest_r8: 0,
            guest_r9: 0,
            guest_r10: 0,
            guest_r11: 0,
            guest_r12: 0,
            guest_r13: 0,
            guest_r14: 0,
            guest_r15: 0,
            host_rax: 0,
            host_rbx: 0,
            host_rcx: 0,
            host_rdx: 0,
            host_rsi: 0,
            host_rdi: 0,
            host_rbp: 0,
            host_r8: 0,
            host_r9: 0,
            host_r10: 0,
            host_r11: 0,
            host_r12: 0,
            host_r13: 0,
            host_r14: 0,
            host_r15: 0,
            launched: 0,
            _pad: 0,
            guest_xsave_ptr: 0,
            host_xsave_ptr: 0,
            xcr0_mask: 0,
            host_xcr0: 0,
            guest_cr2: 0,
        }
    }
}
