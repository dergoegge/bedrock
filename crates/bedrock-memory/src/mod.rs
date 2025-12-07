// SPDX-License-Identifier: GPL-2.0

//! Re-export for use as a submodule in kernel builds.

#![allow(unreachable_pub)]

mod addr;

pub use addr::{GuestPhysAddr, HostPhysAddr, VirtAddr};
