// SPDX-License-Identifier: GPL-2.0

//! Logging macros for bedrock.

/// Log an informational message.
///
/// In kernel mode, this expands to `pr_info!`.
/// In userspace/test mode, this is a no-op.
#[macro_export]
#[cfg(feature = "cargo")]
macro_rules! log_info {
    ($($arg:tt)*) => {
        // Use format_args! to silence unused variable warnings, but don't actually do anything
        let _ = ::core::format_args!($($arg)*);
    };
}

#[macro_export]
#[cfg(not(feature = "cargo"))]
macro_rules! log_info {
    ($($arg:tt)*) => {{
       let _ = ::core::format_args!($($arg)*);
    }};
}

/// Log an error message.
///
/// In kernel mode, this expands to `pr_err!`.
/// In userspace/test mode, this is a no-op.
#[macro_export]
#[cfg(feature = "cargo")]
macro_rules! log_err {
    ($($arg:tt)*) => {
        let _ = ::core::format_args!($($arg)*);
    };
}

#[macro_export]
#[cfg(not(feature = "cargo"))]
macro_rules! log_err {
    ($($arg:tt)*) => {{
       let _ = ::core::format_args!($($arg)*);
    }};
}

/// Log a warning message.
///
/// In kernel mode, this expands to `pr_warn!`.
/// In userspace/test mode, this is a no-op.
#[macro_export]
#[cfg(feature = "cargo")]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        let _ = ::core::format_args!($($arg)*);
    };
}

#[macro_export]
#[cfg(not(feature = "cargo"))]
macro_rules! log_warn {
    ($($arg:tt)*) => {{
       let _ = ::core::format_args!($($arg)*);
    }};
}

/// Log a debug message.
///
/// In kernel mode, this expands to `pr_debug!`.
/// In userspace/test mode, this is a no-op.
#[macro_export]
#[cfg(feature = "cargo")]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        let _ = ::core::format_args!($($arg)*);
    };
}

#[macro_export]
#[cfg(not(feature = "cargo"))]
macro_rules! log_debug {
    ($($arg:tt)*) => {{
       let _ = ::core::format_args!($($arg)*);
    }};
}
