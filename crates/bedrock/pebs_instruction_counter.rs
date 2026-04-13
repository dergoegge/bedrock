// SPDX-License-Identifier: GPL-2.0

//! PEBS+PDist direct PMU instruction counter implementation.
//!
//! This module provides `PebsInstructionCounter`, which directly programs
//! IA32_FIXED_CTR0 (INST_RETIRED.ANY) with PEBS + PDist for zero-skid
//! PMI-triggered VM exits at precise instruction counts.
//!
//! When the counter overflows, PEBS + PDist ensures the PMI fires at exactly
//! the overflowing instruction (zero skid). The PMI is delivered as an NMI,
//! which causes a VM exit that the hypervisor handles deterministically.
//!
//! # DS area placement
//!
//! PEBS requires a DS save area. `IA32_DS_AREA` is a **linear** address that
//! hardware translates through the currently active CR3. During guest
//! execution that means the guest's page tables. For PEBS to work without
//! corrupting guest memory, the DS area must be at a linear address the guest
//! kernel already maps to a guest-physical address that is backed by an EPT
//! mapping we control.
//!
//! We achieve this by reserving the last page of guest memory as the DS area:
//!
//! * Userspace allocates guest memory = user-requested RAM + 1 page.
//! * The guest's e820 lists the full range as RAM, so Linux's direct-map
//!   covers the DS area page at `0xffff888000000000 + GPA`.
//! * A `setup_data` header at that page causes Linux to `memblock_reserve`
//!   the page, preventing the buddy allocator from using it.
//! * The EPT identity-maps guest memory, so PEBS writes land in our page.
//!
//! This requires `nokaslr` in the guest cmdline (so the direct-map base is
//! fixed at `0xffff888000000000`) and an Ice Lake or later CPU with
//! "EPT-friendly PEBS" support (`IA32_PERF_CAPABILITIES` bit 14).

use crate::c_helpers::{
    bedrock_clear_guest_state, bedrock_detect_pebs_pdist, bedrock_set_guest_state,
};
use crate::vmx::traits::InstructionCounter;

/// 48-bit counter mask (IA32_FIXED_CTR0 is 48 bits wide).
const COUNTER_WIDTH: u32 = 48;
const COUNTER_MASK: u64 = (1u64 << COUNTER_WIDTH) - 1;

/// IA32_FIXED_CTR_CTRL bits for counter 0:
/// - Bits [1:0] = 0b11: Enable at all CPLs (OS + USR)
/// - Bit 3 = 1: Enable PMI on overflow
const FIXED_CTR0_ENABLE_ALL_PMI: u64 = 0b1011;

/// IA32_DEBUGCTL bit 12: FREEZE_PERFMON_ON_PMI
const DEBUGCTL_FREEZE_PERFMON_ON_PMI: u64 = 1 << 12;

/// IA32_PEBS_ENABLE bit 32: Enable PEBS on IA32_FIXED_CTR0
const PEBS_ENABLE_FIXED_CTR0: u64 = 1 << 32;

/// IA32_PERF_GLOBAL_STATUS bit 32: IA32_FIXED_CTR0 overflow
const GLOBAL_STATUS_FIXED_CTR0_OVF: u64 = 1 << 32;

/// IA32_PERF_GLOBAL_STATUS bit 59: CTR_FRZ (counters frozen)
const GLOBAL_STATUS_CTR_FRZ: u64 = 1 << 59;

/// IA32_PERF_GLOBAL_CTRL bit 32: Enable IA32_FIXED_CTR0
const GLOBAL_CTRL_FIXED_CTR0: u64 = 1 << 32;

/// DS management area offsets (see Intel SDM Vol 3B, Section 21.9).
mod ds_offset {
    pub(super) const PEBS_BUFFER_BASE: usize = 0x20;
    pub(super) const PEBS_INDEX: usize = 0x28;
    pub(super) const PEBS_ABSOLUTE_MAX: usize = 0x30;
    pub(super) const PEBS_INTERRUPT_THRESHOLD: usize = 0x38;
    pub(super) const PEBS_FIXED_CTR0_RESET: usize = 0x80;
}

/// Within the DS area page, the PEBS record buffer starts at this offset.
/// Earlier bytes are consumed by the setup_data header (first 16 bytes) and
/// the DS management area (first ~128 bytes).
const DS_PEBS_BUFFER_OFFSET: usize = 0x100;
/// Room for one PEBS record (records are <= 256 bytes in common formats).
const DS_PEBS_RECORD_SIZE: usize = 0x100;

/// MSR addresses (matching msr_defs.rs but needed as raw u32 for wrmsr/rdmsr).
mod msr_addr {
    pub(super) const IA32_DEBUGCTL: u32 = 0x1D9;
    pub(super) const IA32_FIXED_CTR0: u32 = 0x309;
    pub(super) const IA32_FIXED_CTR_CTRL: u32 = 0x38D;
    pub(super) const IA32_PERF_GLOBAL_STATUS: u32 = 0x38E;
    pub(super) const IA32_PERF_GLOBAL_CTRL: u32 = 0x38F;
    pub(super) const IA32_PERF_GLOBAL_STATUS_RESET: u32 = 0x390;
    pub(super) const IA32_PEBS_ENABLE: u32 = 0x3F1;
    pub(super) const IA32_DS_AREA: u32 = 0x600;
}

/// Direct PMU instruction counter using PEBS + PDist for zero-skid overflow.
pub(crate) struct PebsInstructionCounter {
    /// Total instructions retired across all VM runs (software accumulator).
    total_count: u64,
    /// Value loaded into IA32_FIXED_CTR0 before last VM entry (48-bit).
    loaded_count: u64,
    /// Target total instruction count for next PMI-triggered VM exit.
    overflow_target: Option<u64>,
    /// Host-writable pointer to the DS area page (within guest memory).
    /// Used to initialize the DS management area fields.
    ds_area_host_ptr: *mut u8,
    /// Guest linear address (Linux direct-map VA) of the DS area.
    /// Written to IA32_DS_AREA so hardware can reach the page during guest
    /// execution via the guest's CR3 + EPT translation.
    ds_area_guest_virt: u64,
    /// Saved host IA32_DEBUGCTL value.
    saved_debugctl: u64,
    /// Saved host IA32_FIXED_CTR_CTRL value.
    saved_fixed_ctr_ctrl: u64,
    /// Saved host IA32_PEBS_ENABLE value.
    saved_pebs_enable: u64,
    /// Saved host IA32_DS_AREA value.
    saved_ds_area: u64,
    /// Saved host IA32_PERF_GLOBAL_CTRL value.
    saved_perf_global_ctrl: u64,
    /// Whether PMU MSRs are currently configured for guest use.
    pmu_configured: bool,
}

// SAFETY: PebsInstructionCounter is used within the VM run loop where
// preemption is disabled. It will not migrate CPUs.
unsafe impl Send for PebsInstructionCounter {}

/// # Safety
/// Caller must be in kernel context with appropriate privileges to read the MSR.
#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    // SAFETY: Caller ensures kernel context with MSR read privileges.
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags),
        );
    }
    u64::from(high) << 32 | u64::from(low)
}

/// # Safety
/// Caller must be in kernel context with appropriate privileges to write the MSR.
#[inline]
unsafe fn wrmsr(msr: u32, val: u64) {
    let low = val as u32;
    let high = (val >> 32) as u32;
    // SAFETY: Caller ensures kernel context with MSR write privileges.
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") low,
            in("edx") high,
            options(nomem, nostack, preserves_flags),
        );
    }
}

/// Write a u64 at byte offset `offset` within the page pointed to by `base`.
/// # Safety
/// `base..base+offset+8` must be a valid writable region (within our DS page).
#[inline]
unsafe fn write_ds_u64(base: *mut u8, offset: usize, value: u64) {
    // SAFETY: Caller ensures the offset is within the DS area page.
    unsafe {
        core::ptr::write_volatile(base.add(offset).cast::<u64>(), value);
    }
}

impl PebsInstructionCounter {
    /// Create a new PEBS+PDist instruction counter.
    ///
    /// * `ds_area_host_ptr`: host-writable pointer to the DS area page (this
    ///   is within the VM's guest-memory allocation).
    /// * `ds_area_guest_virt`: guest linear address (Linux direct-map VA) of
    ///   the same page. Written to `IA32_DS_AREA` during guest execution.
    ///
    /// Returns `None` if the CPU doesn't support PEBS+PDist.
    pub(crate) fn new(ds_area_host_ptr: *mut u8, ds_area_guest_virt: u64) -> Option<Self> {
        // SAFETY: Detection reads CPUID and MSRs, safe from kernel context.
        let supported = unsafe { bedrock_detect_pebs_pdist() };
        if !supported {
            return None;
        }

        // Initialize the DS management area. PEBS fields point into the
        // same page (via guest linear addresses). The BTS fields at offsets
        // 0x00-0x18 are left as-is (the setup_data header occupies
        // 0x00-0x0F); BTS is disabled via IA32_DEBUGCTL so those bytes are
        // never read by the CPU.
        let pebs_buffer_base = ds_area_guest_virt + DS_PEBS_BUFFER_OFFSET as u64;
        // SAFETY: ds_area_host_ptr points to a valid 4 KiB page.
        unsafe {
            write_ds_u64(ds_area_host_ptr, ds_offset::PEBS_BUFFER_BASE, pebs_buffer_base);
            write_ds_u64(ds_area_host_ptr, ds_offset::PEBS_INDEX, pebs_buffer_base);
            write_ds_u64(
                ds_area_host_ptr,
                ds_offset::PEBS_ABSOLUTE_MAX,
                pebs_buffer_base + DS_PEBS_RECORD_SIZE as u64,
            );
            // Threshold = base so PMI fires on the first record.
            write_ds_u64(ds_area_host_ptr, ds_offset::PEBS_INTERRUPT_THRESHOLD, pebs_buffer_base);
            // PEBS Fixed Counter 0 Reset value is updated each VM entry by
            // prepare_for_entry(). Initialize to zero.
            write_ds_u64(ds_area_host_ptr, ds_offset::PEBS_FIXED_CTR0_RESET, 0);
        }

        Some(Self {
            total_count: 0,
            loaded_count: 0,
            overflow_target: None,
            ds_area_host_ptr,
            ds_area_guest_virt,
            saved_debugctl: 0,
            saved_fixed_ctr_ctrl: 0,
            saved_pebs_enable: 0,
            saved_ds_area: 0,
            saved_perf_global_ctrl: 0,
            pmu_configured: false,
        })
    }

    /// Save host PMU MSR values and configure them for guest instruction counting.
    fn configure_pmu(&mut self) {
        if self.pmu_configured {
            return;
        }

        // SAFETY: Reading/writing MSRs from kernel context with preemption disabled.
        unsafe {
            // Save host values
            self.saved_debugctl = rdmsr(msr_addr::IA32_DEBUGCTL);
            self.saved_fixed_ctr_ctrl = rdmsr(msr_addr::IA32_FIXED_CTR_CTRL);
            self.saved_pebs_enable = rdmsr(msr_addr::IA32_PEBS_ENABLE);
            self.saved_ds_area = rdmsr(msr_addr::IA32_DS_AREA);
            self.saved_perf_global_ctrl = rdmsr(msr_addr::IA32_PERF_GLOBAL_CTRL);

            // Disable all counters while reconfiguring (SDM requirement for PEBS)
            wrmsr(msr_addr::IA32_PERF_GLOBAL_CTRL, 0);

            // Configure IA32_FIXED_CTR_CTRL: enable counter 0 at all CPLs + PMI
            // Preserve bits for other fixed counters (bits 4+)
            let ctrl = (self.saved_fixed_ctr_ctrl & !0xF) | FIXED_CTR0_ENABLE_ALL_PMI;
            wrmsr(msr_addr::IA32_FIXED_CTR_CTRL, ctrl);

            // Configure IA32_DEBUGCTL: set FREEZE_PERFMON_ON_PMI
            let debugctl = self.saved_debugctl | DEBUGCTL_FREEZE_PERFMON_ON_PMI;
            wrmsr(msr_addr::IA32_DEBUGCTL, debugctl);

            // Configure IA32_PEBS_ENABLE: enable PEBS on fixed counter 0
            let pebs = self.saved_pebs_enable | PEBS_ENABLE_FIXED_CTR0;
            wrmsr(msr_addr::IA32_PEBS_ENABLE, pebs);

            // Set IA32_DS_AREA to our guest-visible DS area linear address.
            // Hardware translates this through the guest's CR3 (Linux direct-map)
            // to the DS area GPA, then through EPT to our page.
            wrmsr(msr_addr::IA32_DS_AREA, self.ds_area_guest_virt);

            // Clear any stale overflow status
            wrmsr(
                msr_addr::IA32_PERF_GLOBAL_STATUS_RESET,
                GLOBAL_STATUS_FIXED_CTR0_OVF | GLOBAL_STATUS_CTR_FRZ,
            );
        }

        self.pmu_configured = true;
    }

    /// Restore host PMU MSR values.
    fn restore_pmu(&mut self) {
        if !self.pmu_configured {
            return;
        }

        // SAFETY: Restoring MSRs from kernel context with preemption disabled.
        unsafe {
            // Disable all counters before reconfiguring
            wrmsr(msr_addr::IA32_PERF_GLOBAL_CTRL, 0);

            // Restore host values
            wrmsr(msr_addr::IA32_FIXED_CTR_CTRL, self.saved_fixed_ctr_ctrl);
            wrmsr(msr_addr::IA32_DEBUGCTL, self.saved_debugctl);
            wrmsr(msr_addr::IA32_PEBS_ENABLE, self.saved_pebs_enable);
            wrmsr(msr_addr::IA32_DS_AREA, self.saved_ds_area);

            // Clear any overflow status we may have caused
            wrmsr(
                msr_addr::IA32_PERF_GLOBAL_STATUS_RESET,
                GLOBAL_STATUS_FIXED_CTR0_OVF | GLOBAL_STATUS_CTR_FRZ,
            );

            // Restore host global control last (re-enables host counters)
            wrmsr(msr_addr::IA32_PERF_GLOBAL_CTRL, self.saved_perf_global_ctrl);
        }

        self.pmu_configured = false;
    }
}

impl Drop for PebsInstructionCounter {
    fn drop(&mut self) {
        // Restore host PMU state if still configured. The DS area lives in
        // guest memory (owned by the VM) and is freed with the VM.
        if self.pmu_configured {
            self.restore_pmu();
        }
    }
}

impl InstructionCounter for PebsInstructionCounter {
    fn set_guest_state(&mut self, user_mode: bool, rip: u64) {
        // SAFETY: Sets per-CPU state for perf_guest_cbs.
        unsafe {
            bedrock_set_guest_state(user_mode, rip as core::ffi::c_ulong);
        }
    }

    fn clear_guest_state(&mut self) {
        // SAFETY: Clears per-CPU state for perf_guest_cbs.
        unsafe {
            bedrock_clear_guest_state();
        }
    }

    fn enable(&mut self) {
        self.configure_pmu();
    }

    fn disable(&mut self) {
        self.restore_pmu();
    }

    fn read(&self) -> u64 {
        self.total_count
    }

    fn is_configured(&self) -> bool {
        true
    }

    fn perf_global_ctrl_values(&self) -> Option<(u64, u64)> {
        // Guest: enable FIXED_CTR0 (counts guest instructions during VMX non-root).
        // Host: preserve whatever the host was doing, but force FIXED_CTR0 OFF so
        // our counter is frozen between VM exit and accumulate_after_exit(), and
        // so writes to IA32_FIXED_CTR0 in prepare_for_entry() are not disturbed by
        // VMM execution before the next VM entry.
        let host_val = self.saved_perf_global_ctrl & !GLOBAL_CTRL_FIXED_CTR0;
        Some((GLOBAL_CTRL_FIXED_CTR0, host_val))
    }

    fn supports_overflow(&self) -> bool {
        true
    }

    fn set_overflow_target(&mut self, target: u64) {
        self.overflow_target = Some(target);
    }

    fn clear_overflow_target(&mut self) {
        self.overflow_target = None;
    }

    fn prepare_for_entry(&mut self) {
        let remaining = match self.overflow_target {
            Some(target) if target > self.total_count => target - self.total_count,
            Some(_) => {
                // Target already reached or passed — set to overflow immediately.
                1
            }
            None => {
                // No target — load the minimum non-zero value so the counter
                // can count for ~2^48 − 1 instructions before wrapping.
                COUNTER_MASK
            }
        };

        // Load counter with -(remaining) in 48-bit two's complement.
        // When the counter reaches 0 (overflows), exactly `remaining` instructions
        // have been executed.
        let load_value = remaining.wrapping_neg() & COUNTER_MASK;
        self.loaded_count = load_value;

        // Update the PEBS Fixed Counter 0 Reset value so that after a PEBS
        // assist the counter is reloaded to the same load_value.
        // SAFETY: ds_area_host_ptr points to a valid page we initialized.
        unsafe {
            write_ds_u64(
                self.ds_area_host_ptr,
                ds_offset::PEBS_FIXED_CTR0_RESET,
                load_value,
            );
            // Reset the PEBS Index so the next record writes to the buffer
            // start (we only care about the PMI, not the record contents).
            let pebs_buffer_base = self.ds_area_guest_virt + DS_PEBS_BUFFER_OFFSET as u64;
            write_ds_u64(
                self.ds_area_host_ptr,
                ds_offset::PEBS_INDEX,
                pebs_buffer_base,
            );
        }

        // Write the counter value.
        // SAFETY: Writing MSR with preemption disabled in run loop.
        unsafe {
            wrmsr(msr_addr::IA32_FIXED_CTR0, load_value);
        }
    }

    fn accumulate_after_exit(&mut self) {
        // SAFETY: Reading MSRs with preemption disabled after VM exit.
        let status = unsafe { rdmsr(msr_addr::IA32_PERF_GLOBAL_STATUS) };
        // SAFETY: Reading MSRs with preemption disabled after VM exit.
        let counter_now = unsafe { rdmsr(msr_addr::IA32_FIXED_CTR0) } & COUNTER_MASK;

        // Raw 48-bit delta from the value we loaded to the current value.
        let raw_delta = counter_now.wrapping_sub(self.loaded_count) & COUNTER_MASK;

        if status & GLOBAL_STATUS_FIXED_CTR0_OVF != 0 {
            // Counter overflowed. PEBS reloaded the counter from our reset value
            // (which is also loaded_count), so counter_now reads near loaded_count
            // and raw_delta is a small residual. Actual instructions executed =
            // distance from loaded_count to overflow (2^48) + any residual after
            // reload. Distance = -loaded_count mod 2^48.
            let distance = self.loaded_count.wrapping_neg() & COUNTER_MASK;
            self.total_count += distance.wrapping_add(raw_delta);
        } else {
            self.total_count += raw_delta;
        }
    }

    fn check_and_clear_pmi(&mut self) -> bool {
        // SAFETY: Reading/writing MSR in NMI exit handler context.
        let status = unsafe { rdmsr(msr_addr::IA32_PERF_GLOBAL_STATUS) };

        if status & GLOBAL_STATUS_FIXED_CTR0_OVF != 0 {
            // Our counter overflowed — this NMI is our PMI.
            // Clear the overflow bit and CTR_FRZ to unfreeze counters.
            // SAFETY: Writing MSR to clear overflow status in NMI exit handler context.
            unsafe {
                wrmsr(
                    msr_addr::IA32_PERF_GLOBAL_STATUS_RESET,
                    GLOBAL_STATUS_FIXED_CTR0_OVF | GLOBAL_STATUS_CTR_FRZ,
                );
            }
            return true;
        }

        false
    }
}
