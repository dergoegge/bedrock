// SPDX-License-Identifier: GPL-2.0

//! Re-export for use as a submodule in kernel builds.
//!
//! Macros are defined with `#[macro_export]` so they're available at crate root.
//! The `#[macro_use]` attribute on this module makes them available to sibling modules.

#![allow(missing_docs)]

#[macro_use]
mod macros;
