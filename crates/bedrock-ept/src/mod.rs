// SPDX-License-Identifier: GPL-2.0

//! Re-export for use as a submodule in kernel builds.
//!
//! Note: `extern crate alloc` must be declared at the kernel crate root
//! (bedrock_main.rs) for alloc types to be available here.

#![allow(unreachable_pub)]

mod entry;
mod table;
mod traits;

pub use entry::{EptMemoryType, EptPermissions};
pub use table::EptPageTable;
pub use traits::FrameAllocator;
