// SPDX-License-Identifier: GPL-2.0

//! Configuration types for VM ioctls.

/// Single-step configuration for MTF (Monitor Trap Flag) mode.
///
/// Configures the VM to single-step (exit after each instruction) within
/// a specified emulated TSC range. This is useful for debugging determinism
/// issues by tracing every instruction in a specific region.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SingleStepConfig {
    /// Whether single-stepping is enabled.
    /// 0 = disabled, non-zero = enabled.
    pub enabled: u64,
    /// Start of TSC range (inclusive).
    pub tsc_start: u64,
    /// End of TSC range (exclusive).
    pub tsc_end: u64,
}

/// Logging mode for deterministic exit capture.
///
/// Controls when and how exit logging occurs:
/// - `Disabled`: No logging (default)
/// - `AllExits`: Log every deterministic exit (for debugging, higher overhead)
/// - `AtTsc`: Log once when TSC >= target, hash full memory (for binary search)
/// - `AtShutdown`: Log once at vmcall shutdown, hash full memory (for comparison)
/// - `Checkpoints`: Log state snapshots at configurable TSC intervals
/// - `TscRange`: Log only exits within a TSC range (used with single-stepping)
#[repr(u32)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogMode {
    /// No logging.
    #[default]
    Disabled = 0,
    /// Log every deterministic exit.
    AllExits = 1,
    /// Log once when TSC >= target_tsc, hash full memory.
    /// Used for binary search to find divergence point.
    AtTsc = 2,
    /// Log once at vmcall shutdown, hash full memory.
    /// Used for comparing final state across runs.
    AtShutdown = 3,
    /// Log checkpoints at configurable TSC intervals.
    /// Uses target_tsc as the checkpoint interval.
    /// Each checkpoint includes registers and device state hashes.
    /// Memory hash is set to 0 (skipped for performance).
    Checkpoints = 4,
    /// Log only exits within a TSC range.
    /// Uses single_step_tsc_range field for bounds.
    /// Used with single-stepping for fine-grained debugging.
    TscRange = 5,
}

/// Synthetic exit reason for checkpoint entries.
/// This value is used to identify checkpoint log entries.
pub const EXIT_REASON_CHECKPOINT: u32 = 0xFFFFFFFF;

/// Bit flag: skip memory hashing in log entries (set memory_hash to 0).
pub const LOG_FLAG_NO_MEMORY_HASH: u32 = 1 << 0;
/// Bit flag: intercept guest #PF exceptions for determinism analysis.
pub const LOG_FLAG_INTERCEPT_PF: u32 = 1 << 1;

/// Unified logging configuration passed to the kernel via ioctl.
///
/// This struct combines all logging-related settings:
/// - `enabled`: Whether the log buffer is allocated and logging is active
/// - `mode`: The logging mode (when/how to log)
/// - `target_tsc`: Mode-specific TSC value (threshold, trigger, or interval)
/// - `start_tsc`: Universal start threshold (no logging until TSC reaches this)
/// - `flags`: Bitfield for optional behavior (e.g. LOG_FLAG_NO_MEMORY_HASH)
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LogConfig {
    /// Whether logging is enabled.
    /// When transitioning from disabled to enabled, allocates the log buffer.
    /// When transitioning from enabled to disabled, frees the buffer.
    pub enabled: u32,
    /// Mode value (see LogMode enum).
    pub mode: u32,
    /// Target TSC value:
    /// - AllExits: only log when emulated_tsc >= target_tsc
    /// - AtTsc: log once when emulated_tsc >= target_tsc
    /// - Checkpoints: interval between checkpoint entries
    /// - AtShutdown/Disabled/TscRange: ignored
    pub target_tsc: u64,
    /// Universal start threshold - no logging occurs until TSC reaches this value.
    /// Set to 0 to log from the start.
    pub start_tsc: u64,
    /// Flags bitfield (see LOG_FLAG_* constants).
    pub flags: u32,
    /// Reserved for alignment.
    pub _reserved: u32,
}

impl LogConfig {
    /// Create a config with logging disabled and buffer deallocated.
    pub fn disabled() -> Self {
        Self {
            enabled: 0,
            mode: LogMode::Disabled as u32,
            ..Default::default()
        }
    }

    /// Create a config for AllExits mode.
    ///
    /// Logs every deterministic exit when TSC >= threshold.
    pub fn all_exits(threshold: u64) -> Self {
        Self {
            enabled: 1,
            mode: LogMode::AllExits as u32,
            target_tsc: threshold,
            ..Default::default()
        }
    }

    /// Create a config for AtTsc mode.
    ///
    /// Logs once when TSC >= target, then stops. Includes full memory hash.
    pub fn at_tsc(target_tsc: u64) -> Self {
        Self {
            enabled: 1,
            mode: LogMode::AtTsc as u32,
            target_tsc,
            ..Default::default()
        }
    }

    /// Create a config for AtShutdown mode.
    ///
    /// Logs once at vmcall shutdown. Includes full memory hash.
    pub fn at_shutdown() -> Self {
        Self {
            enabled: 1,
            mode: LogMode::AtShutdown as u32,
            ..Default::default()
        }
    }

    /// Create a config for Checkpoints mode.
    ///
    /// Logs checkpoint entries every `interval` TSC ticks.
    /// Each checkpoint captures registers and device state hashes.
    /// Memory hash is skipped (set to 0) for performance.
    pub fn checkpoints(interval: u64) -> Self {
        Self {
            enabled: 1,
            mode: LogMode::Checkpoints as u32,
            target_tsc: interval,
            ..Default::default()
        }
    }

    /// Create a config for TscRange mode.
    ///
    /// Logs exits only within the single_step_tsc_range.
    /// Used with single-stepping (MTF) for fine-grained debugging.
    /// The TSC range must be set separately via `set_single_step_range()`.
    pub fn tsc_range() -> Self {
        Self {
            enabled: 1,
            mode: LogMode::TscRange as u32,
            ..Default::default()
        }
    }

    /// Set the universal start threshold.
    ///
    /// No logging occurs until the emulated TSC reaches this value.
    pub fn with_start_tsc(mut self, start_tsc: u64) -> Self {
        self.start_tsc = start_tsc;
        self
    }

    /// Disable memory hashing in log entries (memory_hash will be 0).
    pub fn with_no_memory_hash(mut self) -> Self {
        self.flags |= LOG_FLAG_NO_MEMORY_HASH;
        self
    }

    /// Enable #PF interception for determinism analysis.
    pub fn with_intercept_pf(mut self) -> Self {
        self.flags |= LOG_FLAG_INTERCEPT_PF;
        self
    }
}
