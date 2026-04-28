// SPDX-License-Identifier: GPL-2.0

//! Linux perf_event-based instruction counter implementation.
//!
//! This module provides `LinuxInstructionCounter`, which uses the Linux kernel's
//! perf_event subsystem to count guest instructions executed during VM runs.

use crate::c_helpers::{
    bedrock_clear_guest_state, bedrock_create_instruction_counter,
    bedrock_destroy_instruction_counter, bedrock_get_perf_global_ctrl, bedrock_perf_event_disable,
    bedrock_perf_event_enable, bedrock_perf_event_read, bedrock_perf_event_realign,
    bedrock_set_guest_state, PerfEvent,
};
use crate::vmx::traits::InstructionCounter;

/// Linux perf_event-based instruction counter implementation.
///
/// Uses the kernel's perf_event subsystem with `exclude_host=1` to only count
/// guest instructions. Two underlying perf_events:
///
/// - `count_event`: free-running (no sample_period), used by `read()` to
///   provide a deterministic cumulative retired-instruction count. Driven by
///   the PMU's instructions-retired counter without PMI overhead, so the value
///   read at any natural VM-exit is exact and reproducible across runs.
/// - `sample_event`: sampling counter with a non-zero sample_period. Its
///   PMU overflow generates a PMI which (with external-interrupt exiting
///   set) causes a VM-exit at the overflow point — used by the hypervisor
///   to land MTF on a precise retired-instruction target (APIC timer
///   deadline, stop-at-tsc threshold, etc.). Created at construction with
///   a seed period; `realign_sampling()` re-arms it to fire at the chosen
///   target whenever `update_mtf_state` installs a new one. Never read.
///
/// The counters are created on the current CPU — userspace must pin the
/// thread to the desired CPU before creating the VM. On hybrid CPUs, this
/// should be a P-core for reliable instruction counting.
pub(crate) struct LinuxInstructionCounter {
    /// Free-running counter, source of truth for `read()`.
    count_event: *mut PerfEvent,
    /// Sampling counter that drives PMI-based periodic VM-exits. NULL when
    /// periodic exits are disabled.
    sample_event: *mut PerfEvent,
    /// Whether counting is currently enabled.
    _enabled: bool,
}

// SAFETY: LinuxInstructionCounter is tied to a specific CPU via its perf_event.
// We ensure it's only used within the VM run loop where preemption is disabled,
// so it won't migrate CPUs. The pointer is only accessed via our helper functions.
unsafe impl Send for LinuxInstructionCounter {}

impl LinuxInstructionCounter {
    /// Create a new instruction counter on the current CPU.
    ///
    /// Userspace must pin the thread to the desired CPU before calling this.
    /// On hybrid CPUs, this should be a P-core for reliable instruction counting.
    ///
    /// Always creates a free-running counting event (used for `read()`).
    /// When `sample_period > 0`, additionally creates a sampling event whose
    /// PMU overflow generates PMIs → external-interrupt VM-exits at retired
    /// guest-instruction boundaries (skid expected; periodic exits are
    /// classified as non-deterministic).
    ///
    /// Returns `None` if creation of the counting event fails.
    pub(crate) fn new(sample_period: u64) -> Option<Self> {
        // SAFETY: helper creates a perf_event on the current CPU; pointer is
        // owned by us and freed in Drop.
        let count_event = unsafe { bedrock_create_instruction_counter(0) };
        if is_err_ptr(count_event) {
            return None;
        }

        let sample_event = if sample_period != 0 {
            // SAFETY: same as above for the sampling event.
            let ev = unsafe { bedrock_create_instruction_counter(sample_period) };
            if is_err_ptr(ev) {
                // Tear down the counting event we just created.
                // SAFETY: helper handles NULL and ERR_PTR safely.
                unsafe { bedrock_destroy_instruction_counter(count_event) };
                return None;
            }
            ev
        } else {
            core::ptr::null_mut()
        };

        Some(Self {
            count_event,
            sample_event,
            _enabled: false,
        })
    }

    /// Create a null instruction counter that doesn't actually count.
    ///
    /// This is used when instruction counting is not requested.
    pub(crate) fn null() -> Self {
        Self {
            count_event: core::ptr::null_mut(),
            sample_event: core::ptr::null_mut(),
            _enabled: false,
        }
    }

    /// Check if this counter has a valid counting event.
    pub(crate) fn is_valid(&self) -> bool {
        !self.count_event.is_null()
    }
}

impl Drop for LinuxInstructionCounter {
    fn drop(&mut self) {
        // SAFETY: The helper handles NULL and ERR_PTR safely.
        unsafe {
            bedrock_destroy_instruction_counter(self.count_event);
            bedrock_destroy_instruction_counter(self.sample_event);
        }
    }
}

impl InstructionCounter for LinuxInstructionCounter {
    fn set_guest_state(&mut self, user_mode: bool, rip: u64) {
        // SAFETY: This sets per-CPU state for perf_guest_cbs.
        // We're in the VM run loop with preemption disabled, so this is safe.
        unsafe {
            bedrock_set_guest_state(user_mode, rip as core::ffi::c_ulong);
        }
    }

    fn clear_guest_state(&mut self) {
        // SAFETY: This clears per-CPU state for perf_guest_cbs.
        // We're in the VM run loop with preemption disabled, so this is safe.
        unsafe {
            bedrock_clear_guest_state();
        }
    }

    fn enable(&mut self) {
        if !self._enabled && self.is_valid() {
            // SAFETY: events are valid perf_event pointers (or NULL, which
            // the helper tolerates).
            unsafe {
                bedrock_perf_event_enable(self.count_event);
                bedrock_perf_event_enable(self.sample_event);
            }
            self._enabled = true;
        }
    }

    fn disable(&mut self) {
        if self._enabled && self.is_valid() {
            // SAFETY: events are valid perf_event pointers (or NULL).
            unsafe {
                bedrock_perf_event_disable(self.count_event);
                bedrock_perf_event_disable(self.sample_event);
            }
            self._enabled = false;
        }
    }

    fn read(&self) -> u64 {
        if self.is_valid() {
            // SAFETY: count_event is a valid perf_event pointer. Reading from
            // the free-running counter gives a deterministic cumulative count.
            unsafe { bedrock_perf_event_read(self.count_event) }
        } else {
            0
        }
    }

    fn is_configured(&self) -> bool {
        self.is_valid()
    }

    fn perf_global_ctrl_values(&self) -> Option<(u64, u64)> {
        if !self.is_valid() {
            return None;
        }

        let mut guest_val: u64 = 0;
        let mut host_val: u64 = 0;

        // SAFETY: We pass valid pointers to the helper.
        let found = unsafe {
            bedrock_get_perf_global_ctrl(
                core::ptr::from_mut(&mut guest_val),
                core::ptr::from_mut(&mut host_val),
            )
        };

        if found {
            Some((guest_val, host_val))
        } else {
            None
        }
    }

    fn realign_sampling(&mut self, period: u64) {
        // SAFETY: sample_event is either NULL (when periodic exits are
        // disabled) or a valid perf_event pointer. The helper handles NULL.
        // The PMU stop/start methods used internally are atomic-safe.
        unsafe {
            bedrock_perf_event_realign(self.sample_event, period);
        }
    }
}

/// Check if a pointer is a Linux ERR_PTR.
///
/// Linux error pointers are in the range [MAX_ERRNO, ULONG_MAX].
/// MAX_ERRNO is typically 4095 (0xFFF).
#[inline]
fn is_err_ptr<T>(ptr: *mut T) -> bool {
    const MAX_ERRNO: usize = 4095;
    let addr = ptr as usize;
    // Error pointers are negative values cast to unsigned,
    // so they're >= (usize::MAX - MAX_ERRNO + 1)
    addr >= usize::MAX - MAX_ERRNO
}
