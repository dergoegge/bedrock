// SPDX-License-Identifier: GPL-2.0

//! Traits for platform-agnostic EPT implementation.

// For kernel builds, memory is a sibling module at the crate root.
// For cargo builds, memory is an external crate (aliased from bedrock-memory).
#[cfg(not(feature = "cargo"))]
pub use crate::memory::{GuestPhysAddr, HostPhysAddr, VirtAddr};

#[cfg(feature = "cargo")]
pub use memory::{GuestPhysAddr, HostPhysAddr, PhysAddr, VirtAddr};

/// Trait for allocating physical memory frames for EPT structures.
pub trait FrameAllocator {
    /// Error type for allocation failures.
    type Error;

    /// The allocated frame type. EPT stores these frames and they are
    /// freed when the EPT is dropped.
    type Frame;

    /// Allocate a zeroed 4KB-aligned physical frame.
    /// Returns ownership of the allocated frame.
    fn allocate_frame(&mut self) -> Result<Self::Frame, Self::Error>;

    /// Get the host physical address of an allocated frame.
    fn frame_phys_addr(frame: &Self::Frame) -> HostPhysAddr;

    /// Convert a host physical address to a virtual address for access.
    /// This is needed to write to EPT structures.
    fn phys_to_virt(&self, phys: HostPhysAddr) -> *mut u8;
}
