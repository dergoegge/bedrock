// SPDX-License-Identifier: GPL-2.0

//! Linux perf_event-based instruction counter implementation.
//!
//! This module provides `LinuxInstructionCounter`, which uses the Linux kernel's
//! perf_event subsystem to count guest instructions executed during VM runs.

use crate::c_helpers::{
    bedrock_clear_guest_state, bedrock_create_instruction_counter,
    bedrock_destroy_instruction_counter, bedrock_get_perf_global_ctrl, bedrock_perf_event_disable,
    bedrock_perf_event_enable, bedrock_perf_event_read, bedrock_set_guest_state, PerfEvent,
};
use crate::vmx::traits::InstructionCounter;

/// Linux perf_event-based instruction counter implementation.
///
/// Uses the kernel's perf_event subsystem with `exclude_host=1` to only count
/// guest instructions. The counter is created on the current CPU - userspace
/// must pin the thread to the desired CPU before creating the VM.
///
/// On hybrid CPUs (like Raptor Lake), userspace should pin to a P-core for
/// reliable instruction counting.
pub(crate) struct LinuxInstructionCounter {
    /// Pointer to the kernel perf_event structure.
    /// NULL if creation failed.
    event: *mut PerfEvent,
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
    /// Returns `None` if perf_event creation fails.
    pub(crate) fn new() -> Option<Self> {
        // SAFETY: We call the helper which creates a perf_event on the current CPU.
        // The pointer is owned by us and will be freed in Drop.
        let event = unsafe { bedrock_create_instruction_counter() };

        // Check for ERR_PTR - error pointers have high bits set
        // IS_ERR_VALUE checks if the value is in the error range
        if is_err_ptr(event) {
            return None;
        }

        Some(Self {
            event,
            _enabled: false,
        })
    }

    /// Create a null instruction counter that doesn't actually count.
    ///
    /// This is used when instruction counting is not requested.
    pub(crate) fn null() -> Self {
        Self {
            event: core::ptr::null_mut(),
            _enabled: false,
        }
    }

    /// Check if this counter has a valid perf_event.
    pub(crate) fn is_valid(&self) -> bool {
        !self.event.is_null()
    }
}

impl Drop for LinuxInstructionCounter {
    fn drop(&mut self) {
        // SAFETY: The helper handles NULL and ERR_PTR safely.
        unsafe {
            bedrock_destroy_instruction_counter(self.event);
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
            // SAFETY: event is a valid perf_event pointer.
            unsafe {
                bedrock_perf_event_enable(self.event);
            }
            self._enabled = true;
        }
    }

    fn disable(&mut self) {
        if self._enabled && self.is_valid() {
            // SAFETY: event is a valid perf_event pointer.
            unsafe {
                bedrock_perf_event_disable(self.event);
            }
            self._enabled = false;
        }
    }

    fn read(&self) -> u64 {
        if self.is_valid() {
            // SAFETY: event is a valid perf_event pointer.
            unsafe { bedrock_perf_event_read(self.event) }
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
            bedrock_get_perf_global_ctrl(&mut guest_val as *mut _, &mut host_val as *mut _)
        };

        if found {
            Some((guest_val, host_val))
        } else {
            None
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
