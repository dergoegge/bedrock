// SPDX-License-Identifier: GPL-2.0

//! RootVm - A concrete VM implementation for running guests.
//!
//! This module provides `RootVm`, a concrete implementation of the `VmContext`
//! trait that can be used in production (kernel module) and testing scenarios.

#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

use super::{ForkableVm, ParentVm};
use core::sync::atomic::{AtomicUsize, Ordering};

const PAGE_SIZE: usize = 4096;

/// A concrete virtual machine implementation.
///
/// `RootVm` provides a complete VM context including:
/// - A VMCS for controlling VM execution
/// - General-purpose register state
/// - Guest physical memory (owned, freed on drop)
/// - EPT page table mapping guest physical to host physical addresses
///
/// # Type Parameters
///
/// * `V` - The VMCS type, must implement `VirtualMachineControlStructure`
/// * `G` - The guest memory type, must implement `GuestMemory`
/// * `I` - The instruction counter type
#[repr(C)]
pub struct RootVm<V: VirtualMachineControlStructure, G: GuestMemory, I: InstructionCounter> {
    /// VM state (shared structure with ForkedVm). Boxed to reduce stack usage.
    pub state: VmStateBox<V, I>,
    /// Guest physical memory. Owned by this VM and freed on drop.
    pub memory: G,
    /// Number of child ForkedVms derived from this VM.
    /// When non-zero, this VM cannot be run (children hold references to our memory).
    /// Uses AtomicUsize for interior mutability (remove_child called via &self).
    children_count: AtomicUsize,
}

/// Error type for RootVm creation.
#[derive(Debug)]
pub enum RootVmError<E> {
    /// EPT page table creation failed.
    EptCreation(E),
    /// EPT mapping failed.
    EptMapping(E),
    /// Guest memory page has no physical address.
    NoPhysAddr(usize),
    /// VmState creation failed.
    VmState(VmStateError<E>),
}

impl<V: VirtualMachineControlStructure, G: GuestMemory, I: InstructionCounter> RootVm<V, G, I> {
    /// Create a new RootVm with the given VMCS, guest memory, machine, and frame allocator.
    ///
    /// This creates an EPT page table, maps all guest memory pages into it, and
    /// allocates an MSR bitmap page if MSR bitmaps are supported.
    /// Guest physical addresses are identity-mapped starting from 0.
    ///
    /// # Arguments
    ///
    /// * `vmcs` - The VMCS, already allocated and initialized with revision ID
    /// * `memory` - Guest physical memory to be owned by this VM
    /// * `machine` - Machine for allocating pages (MSR bitmap)
    /// * `allocator` - Frame allocator for EPT page table structures
    /// * `exit_handler_rip` - Address of the VM exit handler (HOST_RIP in VMCS)
    /// * `instruction_counter` - Instruction counter for deterministic execution
    ///
    /// # Errors
    ///
    /// Returns an error if EPT creation, mapping, or MSR bitmap allocation fails.
    #[inline(never)]
    pub fn new<A: FrameAllocator<Frame = V::P>>(
        vmcs: V,
        memory: G,
        machine: &V::M,
        allocator: &mut A,
        exit_handler_rip: u64,
        instruction_counter: I,
    ) -> Result<Self, RootVmError<A::Error>> {
        // Create the EPT page table
        let mut ept: EptPageTable<V::P> =
            EptPageTable::new(allocator).map_err(RootVmError::EptCreation)?;

        // Map all guest memory pages into the EPT
        // Guest physical address = offset into guest memory (identity mapped from 0)
        let mem_size = memory.size();
        let num_pages = mem_size.div_ceil(PAGE_SIZE);

        for page_idx in 0..num_pages {
            let page_offset = page_idx * PAGE_SIZE;
            let guest_phys = GuestPhysAddr::new(page_offset as u64);
            let host_phys = memory
                .page_phys_addr(page_offset)
                .ok_or(RootVmError::NoPhysAddr(page_offset))?;

            ept.map_4k(
                allocator,
                guest_phys,
                host_phys,
                EptPermissions::READ_WRITE_EXECUTE,
                EptMemoryType::WriteBack,
            )
            .map_err(RootVmError::EptMapping)?;
        }

        // Create VmState with the EPT
        let state = VmState::new::<A>(vmcs, ept, machine, exit_handler_rip, instruction_counter)
            .map_err(RootVmError::VmState)?;

        Ok(Self {
            state: box_vm_state(state),
            memory,
            children_count: AtomicUsize::new(0),
        })
    }
}

impl<V: VirtualMachineControlStructure, G: GuestMemory, I: InstructionCounter> VmContext
    for RootVm<V, G, I>
{
    type Vmcs = V;
    type V = <V::M as Machine>::V;
    type I = I;
    type CowPage = V::P; // RootVm doesn't use COW, but type is needed for trait

    fn state(&self) -> &VmState<Self::Vmcs, Self::I> {
        &self.state
    }

    fn state_mut(&mut self) -> &mut VmState<Self::Vmcs, Self::I> {
        &mut self.state
    }

    fn read_guest_memory(&self, gpa: GuestPhysAddr, buf: &mut [u8]) -> Result<(), MemoryError> {
        let offset = gpa.as_u64() as usize;
        let end = offset
            .checked_add(buf.len())
            .ok_or(MemoryError::OutOfRange)?;

        if end > self.memory.size() {
            return Err(MemoryError::OutOfRange);
        }

        // SAFETY: We've verified the offset and length are within bounds above.
        let src = unsafe { self.memory.as_ptr().add(offset) };
        // SAFETY: src points within guest memory, buf is a valid mutable slice,
        // and we verified offset + buf.len() <= memory.size() above.
        unsafe {
            core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), buf.len());
        }
        Ok(())
    }

    fn write_guest_memory(&mut self, gpa: GuestPhysAddr, buf: &[u8]) -> Result<(), MemoryError> {
        let offset = gpa.as_u64() as usize;
        let end = offset
            .checked_add(buf.len())
            .ok_or(MemoryError::OutOfRange)?;

        if end > self.memory.size() {
            return Err(MemoryError::OutOfRange);
        }

        // SAFETY: We've verified the offset and length are within bounds above.
        let dst = unsafe { self.memory.as_mut_ptr().add(offset) };
        // SAFETY: dst points within guest memory, buf is a valid slice,
        // and we verified offset + buf.len() <= memory.size() above.
        unsafe {
            core::ptr::copy_nonoverlapping(buf.as_ptr(), dst, buf.len());
        }
        Ok(())
    }

    fn finalize_log_entry<K: Kernel>(&mut self, _kernel: &K) {
        let log_idx = match self.state.pending_log_idx.take() {
            Some(idx) => idx,
            None => return,
        };

        let mem_ptr = self.memory.as_ptr();
        let mem_size = self.memory.size();

        let memory_hash = if self.state.skip_memory_hash {
            0
        } else {
            match self.state.log_mode {
                LogMode::AtTsc
                | LogMode::AtShutdown
                | LogMode::AllExits
                | LogMode::Checkpoints
                | LogMode::TscRange => {
                    // Hash full guest memory
                    let mut hasher = Xxh64Hasher::new();
                    // SAFETY: mem_ptr is valid and mem_size is the correct size
                    let memory = unsafe { core::slice::from_raw_parts(mem_ptr, mem_size) };
                    hasher.write_bytes(memory);
                    hasher.finish()
                }
                LogMode::Disabled => 0,
            }
        };

        // Update the log entry's memory_hash field
        if let Some(ptr) = self.state.log_buffer_ptr {
            // SAFETY: log_idx was valid when set, ptr is valid for 1MB
            unsafe {
                let entry_ptr = ptr
                    .add(log_idx * core::mem::size_of::<LogEntry>())
                    .cast::<LogEntry>();
                (*entry_ptr).memory_hash = memory_hash;
            }
        }
    }
}

/// Ensure VMCS is cleared when RootVm is dropped.
///
/// This is important because the VMCS page must not be freed while
/// the VMCS is still "loaded" or associated with a CPU.
impl<V: VirtualMachineControlStructure, G: GuestMemory, I: InstructionCounter> Drop
    for RootVm<V, G, I>
{
    fn drop(&mut self) {
        // Clear the VMCS to transition it to "clear" state before freeing
        // the underlying page. If the VMCS is not currently loaded, this
        // is a no-op. If it is loaded, this ensures proper cleanup.
        if let Err(_e) = self.state.vmcs.clear() {
            // Log error but continue - we're in drop, can't do much else
            log_err!("Failed to clear VMCS during drop\n");
        }
        // Return the VPID to the pool for reuse
        deallocate_vpid(self.state.vpid);
    }
}

impl<V: VirtualMachineControlStructure, G: GuestMemory, I: InstructionCounter> ParentVm
    for RootVm<V, G, I>
{
    fn read_page(&self, gpa: GuestPhysAddr) -> Option<*const u8> {
        // Align to page boundary
        let page_gpa = gpa.as_u64() & !0xFFF;
        let offset = page_gpa as usize;

        if offset + PAGE_SIZE <= self.memory.size() {
            // SAFETY: We verified offset + PAGE_SIZE is within the guest memory bounds.
            Some(unsafe { self.memory.as_ptr().add(offset) })
        } else {
            None
        }
    }

    fn memory_size(&self) -> usize {
        self.memory.size()
    }

    fn remove_child(&self) {
        self.children_count.fetch_sub(1, Ordering::SeqCst);
    }
}

impl<V: VirtualMachineControlStructure, G: GuestMemory, I: InstructionCounter> ForkableVm<V, I>
    for RootVm<V, G, I>
{
    type Page = V::P;

    fn vm_state(&self) -> &VmState<V, I> {
        &self.state
    }

    fn vm_state_mut(&mut self) -> &mut VmState<V, I> {
        &mut self.state
    }

    fn add_child(&self) {
        self.children_count.fetch_add(1, Ordering::SeqCst);
    }

    fn remove_child(&self) {
        self.children_count.fetch_sub(1, Ordering::SeqCst);
    }

    fn children_count(&self) -> usize {
        self.children_count.load(Ordering::SeqCst)
    }
}
