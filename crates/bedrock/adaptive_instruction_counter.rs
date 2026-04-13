// SPDX-License-Identifier: GPL-2.0

//! Adaptive instruction counter that dispatches to PEBS or perf_event at runtime.
//!
//! At VM creation time, the factory detects CPU capabilities and selects the
//! best available implementation:
//! - `Pebs`: Direct PMU programming with PEBS+PDist for zero-skid overflow exits
//! - `PerfEvent`: Linux perf_event polling (fallback for older hardware)

use crate::instruction_counter::LinuxInstructionCounter;
use crate::pebs_instruction_counter::PebsInstructionCounter;
use crate::vmx::traits::InstructionCounter;

/// Instruction counter that adapts to available hardware capabilities.
pub(crate) enum AdaptiveInstructionCounter {
    /// Direct PMU with PEBS+PDist for zero-skid overflow (Ice Lake+).
    Pebs(PebsInstructionCounter),
    /// Linux perf_event polling (fallback).
    PerfEvent(LinuxInstructionCounter),
}

impl InstructionCounter for AdaptiveInstructionCounter {
    #[inline]
    fn set_guest_state(&mut self, user_mode: bool, rip: u64) {
        match self {
            Self::Pebs(c) => c.set_guest_state(user_mode, rip),
            Self::PerfEvent(c) => c.set_guest_state(user_mode, rip),
        }
    }

    #[inline]
    fn clear_guest_state(&mut self) {
        match self {
            Self::Pebs(c) => c.clear_guest_state(),
            Self::PerfEvent(c) => c.clear_guest_state(),
        }
    }

    #[inline]
    fn enable(&mut self) {
        match self {
            Self::Pebs(c) => c.enable(),
            Self::PerfEvent(c) => c.enable(),
        }
    }

    #[inline]
    fn disable(&mut self) {
        match self {
            Self::Pebs(c) => c.disable(),
            Self::PerfEvent(c) => c.disable(),
        }
    }

    #[inline]
    fn read(&self) -> u64 {
        match self {
            Self::Pebs(c) => c.read(),
            Self::PerfEvent(c) => c.read(),
        }
    }

    #[inline]
    fn is_configured(&self) -> bool {
        match self {
            Self::Pebs(c) => c.is_configured(),
            Self::PerfEvent(c) => c.is_configured(),
        }
    }

    #[inline]
    fn perf_global_ctrl_values(&self) -> Option<(u64, u64)> {
        match self {
            Self::Pebs(c) => c.perf_global_ctrl_values(),
            Self::PerfEvent(c) => c.perf_global_ctrl_values(),
        }
    }

    #[inline]
    fn supports_overflow(&self) -> bool {
        match self {
            Self::Pebs(c) => c.supports_overflow(),
            Self::PerfEvent(c) => c.supports_overflow(),
        }
    }

    #[inline]
    fn set_overflow_target(&mut self, target: u64) {
        match self {
            Self::Pebs(c) => c.set_overflow_target(target),
            Self::PerfEvent(c) => c.set_overflow_target(target),
        }
    }

    #[inline]
    fn clear_overflow_target(&mut self) {
        match self {
            Self::Pebs(c) => c.clear_overflow_target(),
            Self::PerfEvent(c) => c.clear_overflow_target(),
        }
    }

    #[inline]
    fn accumulate_after_exit(&mut self) {
        match self {
            Self::Pebs(c) => c.accumulate_after_exit(),
            Self::PerfEvent(c) => c.accumulate_after_exit(),
        }
    }

    #[inline]
    fn prepare_for_entry(&mut self) {
        match self {
            Self::Pebs(c) => c.prepare_for_entry(),
            Self::PerfEvent(c) => c.prepare_for_entry(),
        }
    }

    #[inline]
    fn check_and_clear_pmi(&mut self) -> bool {
        match self {
            Self::Pebs(c) => c.check_and_clear_pmi(),
            Self::PerfEvent(c) => c.check_and_clear_pmi(),
        }
    }
}
