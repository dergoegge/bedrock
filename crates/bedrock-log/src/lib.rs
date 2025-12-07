// SPDX-License-Identifier: GPL-2.0

//! Conditional logging macros for bedrock.
//!
//! This crate provides logging macros that work in both kernel and userspace contexts:
//! - In kernel mode (without `cargo` feature): uses `pr_info!`, `pr_err!`, etc. from the kernel crate
//! - In userspace/tests (with `cargo` feature): no-op (compiles to nothing)
//!
//! # Usage
//!
//! ```ignore
//! use bedrock_log::{log_info, log_err, log_warn, log_debug};
//!
//! log_info!("Hello from bedrock!\n");
//! log_err!("An error occurred: {}\n", error_code);
//! log_warn!("Warning: value {} is deprecated\n", value);
//! log_debug!("Debug info: {:?}\n", data);
//! ```

#![no_std]

#[macro_use]
mod macros;
