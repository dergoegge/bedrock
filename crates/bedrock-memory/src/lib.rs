// SPDX-License-Identifier: GPL-2.0

//! Memory address types for x86-64 virtualization.
//!
//! This crate provides type-safe wrappers for physical and virtual addresses
//! used in virtualization contexts (EPT, VMCS, etc.).

#![no_std]

mod addr;

#[cfg(test)]
mod tests;

pub use addr::{GuestPhysAddr, HostPhysAddr, PhysAddr, VirtAddr};
