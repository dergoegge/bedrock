// SPDX-License-Identifier: GPL-2.0

//! Instruction counter trait for deterministic guest execution.
//!
//! This module provides an abstraction for counting guest instructions executed
//! during VM runs. The primary use case is supporting deterministic virtual time
//! for fuzzing and replay.
//!
//! The trait abstracts over the underlying implementation (perf_events on Linux)
//! to allow testing without hardware.

/// Trait for counting guest instructions.
///
/// Implementations must track the total number of instructions executed by the
/// guest. The counter should be:
/// - Set guest state before VM entry (for perf_guest_cbs)
/// - Enabled before VM entry
/// - Disabled after VM exit
/// - Clear guest state after VM exit
/// - Read after VM exit to get the exact count
///
/// Note: PF_VCPU flag management (for htop guest time accounting) is handled
/// directly in VmRunner::run() to ensure it wraps only the actual guest execution,
/// not exit handling.
///
/// The polling-based approach (reading on every exit) provides deterministic
/// counts with no skid, unlike overflow-based approaches.
pub trait InstructionCounter {
    /// Set guest state before VM entry.
    ///
    /// This tells the perf subsystem that we're entering guest mode. On Linux,
    /// this updates per-CPU state that perf_guest_cbs uses to determine if
    /// we're in guest mode.
    ///
    /// # Arguments
    /// * `user_mode` - true if guest CPL > 0 (user mode), false if ring 0
    /// * `rip` - guest instruction pointer
    fn set_guest_state(&mut self, user_mode: bool, rip: u64);

    /// Clear guest state after VM exit.
    ///
    /// This tells the perf subsystem that we've exited guest mode.
    fn clear_guest_state(&mut self);

    /// Enable instruction counting.
    ///
    /// Call this before VM entry. If counting is already enabled, this is a no-op.
    fn enable(&mut self);

    /// Disable instruction counting.
    ///
    /// Call this after VM exit. If counting is already disabled, this is a no-op.
    fn disable(&mut self);

    /// Read the current instruction count.
    ///
    /// Returns the total number of guest instructions executed since the counter
    /// was created. This is an exact value with no skid when read after VM exit.
    fn read(&self) -> u64;

    /// Check if instruction counting is configured.
    ///
    /// Returns `true` if this counter is backed by real hardware (e.g., perf_events),
    /// `false` for null/mock implementations.
    fn is_configured(&self) -> bool;

    /// Get the PERF_GLOBAL_CTRL MSR values for hardware-assisted switching.
    ///
    /// Returns `Some((guest_val, host_val))` if hardware perf counter switching is
    /// available, `None` otherwise. The values should be written to the VMCS
    /// `GUEST_IA32_PERF_GLOBAL_CTRL` and `HOST_IA32_PERF_GLOBAL_CTRL` fields.
    ///
    /// When the VM entry/exit control bits `LOAD_IA32_PERF_GLOBAL_CTRL` are set,
    /// the CPU atomically loads these values during VM transitions, eliminating
    /// instruction counting overhead from manual MSR switching.
    fn perf_global_ctrl_values(&self) -> Option<(u64, u64)>;

    /// Returns true if this counter supports precise PMI-triggered VM exits
    /// at target instruction counts (via PEBS + PDist).
    fn supports_overflow(&self) -> bool {
        false
    }

    /// Arm the counter to trigger a PMI at `target` total retired instructions.
    ///
    /// The PMI will cause an NMI VM exit at the precise instruction boundary
    /// (zero skid with PDist). No-op if overflow is not supported.
    fn set_overflow_target(&mut self, _target: u64) {}

    /// Remove any overflow target. The counter runs freely without triggering PMI.
    fn clear_overflow_target(&mut self) {}

    /// After VM exit: read the hardware counter, accumulate into the total,
    /// and clear any overflow status. Must be called before `read()`.
    fn accumulate_after_exit(&mut self) {}

    /// Before VM entry: program the hardware counter for the next guest run.
    fn prepare_for_entry(&mut self) {}

    /// Check if the last NMI VM exit was caused by our PMI (counter overflow).
    ///
    /// If so, clears the overflow status and returns true. The caller should
    /// treat this as a deterministic exit. If false, the NMI is a host NMI
    /// and should be forwarded via `INT 2`.
    fn check_and_clear_pmi(&mut self) -> bool {
        false
    }
}

/// Null implementation for VMs without instruction counting.
///
/// This is used when instruction counting is not requested or not available.
/// All operations are no-ops and `read()` always returns 0.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullInstructionCounter;

impl InstructionCounter for NullInstructionCounter {
    #[inline]
    fn set_guest_state(&mut self, _user_mode: bool, _rip: u64) {}

    #[inline]
    fn clear_guest_state(&mut self) {}

    #[inline]
    fn enable(&mut self) {}

    #[inline]
    fn disable(&mut self) {}

    #[inline]
    fn read(&self) -> u64 {
        0
    }

    #[inline]
    fn is_configured(&self) -> bool {
        false
    }

    #[inline]
    fn perf_global_ctrl_values(&self) -> Option<(u64, u64)> {
        None
    }
}

#[cfg(test)]
#[path = "instruction_counter_tests.rs"]
mod tests;
