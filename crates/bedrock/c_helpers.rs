// SPDX-License-Identifier: GPL-2.0

//! Bindings to local C helper functions in helpers.c

use kernel::bindings::{page, phys_addr_t, smp_call_func_t};

/// Opaque type representing a Linux perf_event structure.
///
/// We only interact with perf_events through our C helper functions,
/// so we don't need to know its internal layout.
#[repr(C)]
pub(crate) struct PerfEvent {
    _private: [u8; 0],
}

/// XXH64 streaming state structure.
///
/// Matches the kernel's struct xxh64_state from <linux/xxhash.h>.
#[repr(C)]
pub struct Xxh64State {
    pub total_len: u64,
    pub v1: u64,
    pub v2: u64,
    pub v3: u64,
    pub v4: u64,
    pub mem64: [u64; 4],
    pub memsize: u32,
}

/// Info struct passed to callbacks by bedrock_for_each_cpu.
/// Matches the C struct bedrock_cpu_call_info.
#[repr(C)]
pub(crate) struct BedrockCpuCallInfo {
    pub(crate) info: *mut core::ffi::c_void,
    pub(crate) error: i32,
}

/// VMX capabilities structure.
/// Matches the C struct bedrock_vmx_caps layout.
#[repr(C)]
pub(crate) struct BedrockVmxCaps {
    pub(crate) pin_based_exec_ctrl: u32,
    pub(crate) cpu_based_exec_ctrl: u32,
    pub(crate) cpu_based_exec_ctrl2: u32,
    pub(crate) vmexit_ctrl: u32,
    pub(crate) vmentry_ctrl: u32,
    pub(crate) cr0_fixed0: u64,
    pub(crate) cr0_fixed1: u64,
    pub(crate) cr4_fixed0: u64,
    pub(crate) cr4_fixed1: u64,
    pub(crate) has_ept: bool,
    pub(crate) has_vpid: bool,
}

extern "C" {
    /// Convert a struct page pointer to its physical address.
    pub(crate) fn bedrock_page_to_phys(page: *mut page) -> phys_addr_t;

    /// Get the kernel virtual address for a page.
    pub(crate) fn bedrock_page_address(page: *mut page) -> *mut core::ffi::c_void;

    /// Execute a function on each online CPU sequentially with per-CPU error handling.
    /// Returns 0 on success, or the first error encountered.
    /// If an error occurs, failed_cpu will be set to the CPU that failed.
    pub(crate) fn bedrock_for_each_cpu(
        func: smp_call_func_t,
        info: *mut core::ffi::c_void,
        failed_cpu: *mut i32,
    ) -> i32;

    /// Allocate zeroed memory that can be mapped to userspace.
    pub(crate) fn bedrock_vmalloc_user(size: core::ffi::c_ulong) -> *mut core::ffi::c_void;

    /// Free memory allocated with bedrock_vmalloc_user.
    pub(crate) fn bedrock_vfree(addr: *mut core::ffi::c_void);

    /// Get the physical address of a page within vmalloc memory.
    /// Returns 0 if the address is not valid vmalloc memory.
    pub(crate) fn bedrock_vmalloc_to_phys(addr: *mut core::ffi::c_void) -> phys_addr_t;

    /// Convert any kernel virtual address to its physical address.
    /// Handles both vmalloc and direct-mapped (kmalloc/alloc_page) addresses.
    /// Returns 0 if the address is invalid.
    pub(crate) fn bedrock_kva_to_phys(addr: *mut core::ffi::c_void) -> phys_addr_t;

    /// Convert a physical address to a kernel virtual address.
    pub(crate) fn bedrock_phys_to_virt(phys: phys_addr_t) -> *mut core::ffi::c_void;

    /// Create an anonymous inode and return a file descriptor for it.
    ///
    /// This creates a new file descriptor pointing to an anonymous inode
    /// with the given file operations. The priv pointer is stored in
    /// file->private_data.
    ///
    /// Returns: file descriptor on success, negative error code on failure.
    pub(crate) fn bedrock_anon_inode_getfd(
        name: *const core::ffi::c_char,
        fops: *const kernel::bindings::file_operations,
        priv_: *mut core::ffi::c_void,
        flags: core::ffi::c_int,
    ) -> core::ffi::c_int;

    /// Copy data from userspace to kernel space.
    ///
    /// Returns: Number of bytes that could NOT be copied (0 on success).
    pub(crate) fn bedrock_copy_from_user(
        to: *mut core::ffi::c_void,
        from: *const core::ffi::c_void,
        n: core::ffi::c_ulong,
    ) -> core::ffi::c_ulong;

    /// Copy data from kernel space to userspace.
    ///
    /// Returns: Number of bytes that could NOT be copied (0 on success).
    pub(crate) fn bedrock_copy_to_user(
        to: *mut core::ffi::c_void,
        from: *const core::ffi::c_void,
        n: core::ffi::c_ulong,
    ) -> core::ffi::c_ulong;

    /// Map vmalloc memory into a userspace VMA.
    ///
    /// The vmalloc memory must have been allocated with vmalloc_user().
    ///
    /// Returns: 0 on success, negative error code on failure.
    pub(crate) fn bedrock_remap_vmalloc_range(
        vma: *mut kernel::bindings::vm_area_struct,
        addr: *mut core::ffi::c_void,
        pgoff: core::ffi::c_ulong,
    ) -> core::ffi::c_int;

    /// Map a single page into a userspace VMA.
    /// Uses remap_pfn_range internally.
    ///
    /// Returns: 0 on success, negative error code on failure.
    pub(crate) fn bedrock_remap_page(
        vma: *mut kernel::bindings::vm_area_struct,
        page: *mut kernel::bindings::page,
    ) -> core::ffi::c_int;

    /// Map multiple (potentially non-contiguous) physical pages into a userspace VMA.
    ///
    /// The hpas array contains page-aligned host physical addresses.
    /// The VMA size must equal num_pages * PAGE_SIZE.
    ///
    /// Returns: 0 on success, negative error code on failure.
    pub(crate) fn bedrock_remap_pages(
        vma: *mut kernel::bindings::vm_area_struct,
        hpas: *const u64,
        num_pages: core::ffi::c_int,
    ) -> core::ffi::c_int;

    /// Get VMA start address.
    pub(crate) fn bedrock_vma_start(vma: *mut kernel::bindings::vm_area_struct) -> core::ffi::c_ulong;

    /// Get VMA end address.
    pub(crate) fn bedrock_vma_end(vma: *mut kernel::bindings::vm_area_struct) -> core::ffi::c_ulong;

    /// Get VMA page offset.
    pub(crate) fn bedrock_vma_pgoff(vma: *mut kernel::bindings::vm_area_struct) -> core::ffi::c_ulong;

    /// Disable preemption on the current CPU.
    pub(crate) fn bedrock_preempt_disable();

    /// Enable preemption on the current CPU.
    pub(crate) fn bedrock_preempt_enable();

    /// Enable local interrupts (sets IF flag in RFLAGS).
    pub(crate) fn bedrock_local_irq_enable();

    /// Disable local interrupts (clears IF flag in RFLAGS).
    pub(crate) fn bedrock_local_irq_disable();

    /// Check if the current task needs to be rescheduled.
    ///
    /// This is a thin wrapper around the kernel's need_resched() check.
    /// Returns non-zero if TIF_NEED_RESCHED is set.
    pub(crate) fn bedrock_need_resched() -> core::ffi::c_int;

    /// Get the current real time as a Unix timestamp (seconds since 1970).
    pub(crate) fn bedrock_get_real_time_secs() -> u64;

    /// Register bedrock's perf guest callbacks.
    /// Call this during module initialization.
    pub(crate) fn bedrock_register_perf_callbacks();

    /// Unregister bedrock's perf guest callbacks.
    /// Call this during module cleanup.
    pub(crate) fn bedrock_unregister_perf_callbacks();

    /// Mark that we're entering guest mode on the current CPU.
    /// Call this just before VM entry (with preemption disabled).
    pub(crate) fn bedrock_set_guest_state(user_mode: bool, rip: core::ffi::c_ulong);

    /// Mark that we've exited guest mode on the current CPU.
    /// Call this just after VM exit (with preemption still disabled).
    pub(crate) fn bedrock_clear_guest_state();

    /// Mark current task as running a virtual CPU (sets PF_VCPU flag).
    ///
    /// When PF_VCPU is set, the kernel's time accounting automatically treats
    /// CPU time as guest time. This makes htop show guest time in orange.
    /// Call this before VM entry.
    pub(crate) fn bedrock_enter_guest();

    /// Clear VCPU flag when leaving guest mode.
    /// Call this after VM exit.
    pub(crate) fn bedrock_leave_guest();

    /// Create an instruction counter for guest instruction counting.
    ///
    /// Uses exclude_kernel=1 and exclude_host=1 to only count guest instructions.
    /// Creates counter on current CPU - userspace must pin thread to desired CPU
    /// before calling this (use sched_setaffinity for P-core pinning on hybrid CPUs).
    ///
    /// Returns: perf_event pointer, or ERR_PTR on failure
    pub(crate) fn bedrock_create_instruction_counter() -> *mut PerfEvent;

    /// Destroy an instruction counter created with bedrock_create_instruction_counter.
    pub(crate) fn bedrock_destroy_instruction_counter(event: *mut PerfEvent);

    /// Enable instruction counting (call before VM entry).
    pub(crate) fn bedrock_perf_event_enable(event: *mut PerfEvent);

    /// Disable instruction counting (call after VM exit).
    pub(crate) fn bedrock_perf_event_disable(event: *mut PerfEvent);

    /// Read the current instruction count (exact value, no skid).
    pub(crate) fn bedrock_perf_event_read(event: *mut PerfEvent) -> u64;

    /// Get the PERF_GLOBAL_CTRL MSR values for hardware-assisted switching.
    ///
    /// Returns true if values were found, false otherwise.
    /// When true, guest_val and host_val contain the values to write to
    /// GUEST_IA32_PERF_GLOBAL_CTRL and HOST_IA32_PERF_GLOBAL_CTRL VMCS fields.
    pub(crate) fn bedrock_get_perf_global_ctrl(guest_val: *mut u64, host_val: *mut u64) -> bool;

    /// One-shot XXH64 hash.
    pub fn bedrock_xxh64(
        input: *const core::ffi::c_void,
        length: usize,
        seed: u64,
    ) -> u64;

    /// Reset XXH64 state for streaming hashing.
    pub fn bedrock_xxh64_reset(state: *mut Xxh64State, seed: u64);

    /// Update XXH64 state with more data.
    pub fn bedrock_xxh64_update(
        state: *mut Xxh64State,
        input: *const core::ffi::c_void,
        length: usize,
    );

    /// Finalize and return the XXH64 hash.
    pub fn bedrock_xxh64_digest(state: *const Xxh64State) -> u64;

    /// Check if VMX is enabled on the current CPU.
    /// Must be called with preemption disabled.
    pub(crate) fn bedrock_vcpu_is_vmxon() -> bool;

    /// Set VMX enabled state on the current CPU.
    /// Must be called with preemption disabled.
    pub(crate) fn bedrock_vcpu_set_vmxon(enabled: bool);

    /// Get VMX capabilities for the current CPU.
    /// Returns a pointer that is valid while preemption is disabled.
    pub(crate) fn bedrock_vcpu_get_capabilities() -> *const BedrockVmxCaps;

    /// Set VMX capabilities for the current CPU.
    /// Must be called with preemption disabled.
    pub(crate) fn bedrock_vcpu_set_capabilities(
        pin_based: u32,
        cpu_based: u32,
        cpu_based2: u32,
        vmexit: u32,
        vmentry: u32,
        cr0_fixed0: u64,
        cr0_fixed1: u64,
        cr4_fixed0: u64,
        cr4_fixed1: u64,
        has_ept: bool,
        has_vpid: bool,
    );

    /// Set VMXON region for the current CPU.
    /// Must be called with preemption disabled.
    pub(crate) fn bedrock_vcpu_set_vmxon_region(phys: u64, virt: u64);

    /// Set CR4.VMXE using cr4_set_bits() to properly update the kernel's CR4 shadow.
    pub(crate) fn bedrock_cr4_set_vmxe();

    /// Clear CR4.VMXE using cr4_clear_bits() to properly update the kernel's CR4 shadow.
    /// Must only be called after VMXOFF (outside VMX operation).
    pub(crate) fn bedrock_cr4_clear_vmxe();
}

/// RAII guard that disables preemption while held.
///
/// Preemption is disabled when the guard is created and re-enabled when dropped.
/// This ensures the current thread stays on the same CPU for the duration.
pub(crate) struct PreemptionGuard {
    // Zero-sized marker to prevent Send/Sync and construction outside this module
    _marker: core::marker::PhantomData<*mut ()>,
}

impl PreemptionGuard {
    /// Disable preemption and return a guard.
    ///
    /// Preemption will be re-enabled when the guard is dropped.
    #[inline]
    pub(crate) fn new() -> Self {
        // SAFETY: This is a valid kernel call that disables preemption.
        unsafe { bedrock_preempt_disable() };
        Self {
            _marker: core::marker::PhantomData,
        }
    }
}

impl Drop for PreemptionGuard {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: Preemption was disabled in new(), so it's safe to re-enable.
        unsafe { bedrock_preempt_enable() };
    }
}

/// Enable local interrupts.
#[inline]
pub(crate) fn local_irq_enable() {
    // SAFETY: Enabling interrupts is always safe.
    unsafe { bedrock_local_irq_enable() };
}

/// Disable local interrupts.
#[inline]
pub(crate) fn local_irq_disable() {
    // SAFETY: Disabling interrupts is always safe.
    unsafe { bedrock_local_irq_disable() };
}

