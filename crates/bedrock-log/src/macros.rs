// SPDX-License-Identifier: GPL-2.0

//! Logging macros for bedrock.
//!
//! Each macro has three cfg-gated definitions (evaluated at definition time in this crate):
//! - `feature = "cargo"`: no-op (Cargo/test builds)
//! - `kernel_log` cfg set (kernel builds with `KERNEL_LOG=1`): calls the corresponding `pr_*!`
//! - neither: no-op (kernel builds without logging)
//!
//! The `$dollar:tt` parameter passes a literal `$` into the generated inner `macro_rules!`
//! so its metavariable patterns don't conflict with the outer macro's parser.
macro_rules! define_log_macro {
    ($dollar:tt $(#[$doc:meta])* $name:ident, $kernel_macro:ident) => {
        $(#[$doc])*
        #[macro_export]
        #[cfg(feature = "cargo")]
        macro_rules! $name {
            ($dollar($dollar arg:tt)*) => {
                let _ = ::core::format_args!($dollar($dollar arg)*);
            };
        }

        #[macro_export]
        #[cfg(all(not(feature = "cargo"), kernel_log))]
        macro_rules! $name {
            ($dollar($dollar arg:tt)*) => {{
                ::kernel::$kernel_macro!($dollar($dollar arg)*);
            }};
        }

        #[macro_export]
        #[cfg(all(not(feature = "cargo"), not(kernel_log)))]
        macro_rules! $name {
            ($dollar($dollar arg:tt)*) => {{
                let _ = ::core::format_args!($dollar($dollar arg)*);
            }};
        }
    };
}

define_log_macro!(
    $
    /// Log an informational message. Expands to `pr_info!` when `kernel_log` is set.
    log_info,
    pr_info
);

define_log_macro!(
    $
    /// Log an error message. Expands to `pr_err!` when `kernel_log` is set.
    log_err,
    pr_err
);

define_log_macro!(
    $
    /// Log a warning message. Expands to `pr_warn!` when `kernel_log` is set.
    log_warn,
    pr_warn
);

define_log_macro!(
    $
    /// Log a debug message. Expands to `pr_debug!` when `kernel_log` is set.
    log_debug,
    pr_debug
);
