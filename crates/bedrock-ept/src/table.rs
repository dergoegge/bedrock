// SPDX-License-Identifier: GPL-2.0

//! EPT page table management.

use super::compat::{ept_vec_init, ept_vec_push, ept_vec_with_capacity, EptVec};

use super::entry::{EptEntry, EptMemoryType, EptPermissions};
use super::traits::{FrameAllocator, GuestPhysAddr, HostPhysAddr, VirtAddr};

/// Error type for EPT remap operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EptRemapError {
    /// The guest physical address is not mapped.
    NotMapped,
}

/// EPT page table structure.
///
/// Manages a 4-level EPT hierarchy (PML4 -> PDPT -> PD -> PT).
/// Generic over the frame type to support different allocators.
/// Frames are stored and freed automatically when EPT is dropped.
pub struct EptPageTable<Frame> {
    /// Host physical address of the PML4 table (used for EPTP).
    pml4_phys: HostPhysAddr,
    /// All allocated EPT frames. Freed automatically on drop.
    /// Uses vmalloc-backed allocation in kernel builds so the backing allocation
    /// can fall back to vmalloc when kmalloc fails for large contiguous allocations.
    frames: EptVec<Frame>,
}

impl<Frame> EptPageTable<Frame> {
    /// Create a new EPT page table structure.
    ///
    /// Allocates and zeroes the PML4 table.
    pub fn new<A: FrameAllocator<Frame = Frame>>(allocator: &mut A) -> Result<Self, A::Error> {
        let pml4_frame = allocator.allocate_frame()?;
        let pml4_phys = A::frame_phys_addr(&pml4_frame);

        // Zero out the PML4 table
        let pml4_virt = allocator.phys_to_virt(pml4_phys);
        unsafe {
            core::ptr::write_bytes(pml4_virt, 0, 4096);
        }

        let frames = ept_vec_init(pml4_frame);

        Ok(Self { pml4_phys, frames })
    }

    /// Construct the EPTP value for use in VMCS.
    ///
    /// Uses write-back memory type and 4-level page walk.
    pub fn eptp(&self) -> u64 {
        let mem_type = 6u64; // WB
        let page_walk_len = 3u64; // 4 levels - 1
        self.pml4_phys.as_u64() | (page_walk_len << 3) | mem_type
    }

    /// Look up the mapping for a guest physical address.
    ///
    /// Returns the host physical address and permissions if the page is mapped,
    /// or None if the page is not mapped.
    ///
    /// Note: This method only uses `phys_to_virt` from the allocator, so any
    /// FrameAllocator that provides the same address translation will work.
    pub fn lookup<A: FrameAllocator>(
        &self,
        allocator: &A,
        guest_phys: GuestPhysAddr,
    ) -> Option<(HostPhysAddr, EptPermissions)> {
        let guest_virt = VirtAddr::new(guest_phys.as_u64());

        // Walk the page table hierarchy
        let pml4 = allocator.phys_to_virt(self.pml4_phys) as *const EptEntry;
        let pml4e = unsafe { &*pml4.add(guest_virt.pml4_index()) };
        if !pml4e.is_present() {
            return None;
        }

        let pdpt = allocator.phys_to_virt(pml4e.addr()) as *const EptEntry;
        let pdpte = unsafe { &*pdpt.add(guest_virt.pdpt_index()) };
        if !pdpte.is_present() {
            return None;
        }

        let pd = allocator.phys_to_virt(pdpte.addr()) as *const EptEntry;
        let pde = unsafe { &*pd.add(guest_virt.pd_index()) };
        if !pde.is_present() {
            return None;
        }

        let pt = allocator.phys_to_virt(pde.addr()) as *const EptEntry;
        let pte = unsafe { &*pt.add(guest_virt.pt_index()) };
        if !pte.is_present() {
            return None;
        }

        Some((pte.addr(), pte.permissions()))
    }

    /// Map a 4KB guest physical page to a host physical page.
    pub fn map_4k<A: FrameAllocator<Frame = Frame>>(
        &mut self,
        allocator: &mut A,
        guest_phys: GuestPhysAddr,
        host_phys: HostPhysAddr,
        perms: EptPermissions,
        mem_type: EptMemoryType,
    ) -> Result<(), A::Error> {
        let guest_virt = VirtAddr::new(guest_phys.as_u64());

        // Walk/create the page table hierarchy
        let pml4_entry =
            self.get_or_create_entry(allocator, self.pml4_phys, guest_virt.pml4_index())?;
        let pdpt_phys = self.ensure_table(allocator, pml4_entry, perms)?;

        let pdpt_entry = self.get_or_create_entry(allocator, pdpt_phys, guest_virt.pdpt_index())?;
        let pd_phys = self.ensure_table(allocator, pdpt_entry, perms)?;

        let pd_entry = self.get_or_create_entry(allocator, pd_phys, guest_virt.pd_index())?;
        let pt_phys = self.ensure_table(allocator, pd_entry, perms)?;

        // Set the final PT entry
        let pt_entry = self.get_entry_mut(allocator, pt_phys, guest_virt.pt_index());
        unsafe {
            *pt_entry = EptEntry::page_entry_4k(host_phys, perms, mem_type);
        }

        Ok(())
    }

    /// Remap an existing 4KB page with new host physical address and/or permissions.
    ///
    /// This is used for copy-on-write: after copying a page, we remap the GPA
    /// to point to the new page with RWX permissions.
    ///
    /// Returns an error if the page is not already mapped.
    ///
    /// Note: This method only uses `phys_to_virt` from the allocator, so any
    /// FrameAllocator that provides the same address translation will work.
    pub fn remap_4k<A: FrameAllocator>(
        &mut self,
        allocator: &A,
        guest_phys: GuestPhysAddr,
        new_host_phys: HostPhysAddr,
        perms: EptPermissions,
        mem_type: EptMemoryType,
    ) -> Result<(), EptRemapError> {
        let guest_virt = VirtAddr::new(guest_phys.as_u64());

        // Walk the page table hierarchy (all levels must exist)
        let pml4 = allocator.phys_to_virt(self.pml4_phys) as *const EptEntry;
        let pml4e = unsafe { &*pml4.add(guest_virt.pml4_index()) };
        if !pml4e.is_present() {
            return Err(EptRemapError::NotMapped);
        }

        let pdpt = allocator.phys_to_virt(pml4e.addr()) as *const EptEntry;
        let pdpte = unsafe { &*pdpt.add(guest_virt.pdpt_index()) };
        if !pdpte.is_present() {
            return Err(EptRemapError::NotMapped);
        }

        let pd = allocator.phys_to_virt(pdpte.addr()) as *const EptEntry;
        let pde = unsafe { &*pd.add(guest_virt.pd_index()) };
        if !pde.is_present() {
            return Err(EptRemapError::NotMapped);
        }

        let pt = allocator.phys_to_virt(pde.addr()) as *mut EptEntry;
        let pte = unsafe { &mut *pt.add(guest_virt.pt_index()) };
        if !pte.is_present() {
            return Err(EptRemapError::NotMapped);
        }

        // Update the leaf entry with new host physical and permissions
        *pte = EptEntry::page_entry_4k(new_host_phys, perms, mem_type);
        Ok(())
    }

    // Helper: Get a mutable pointer to an entry
    fn get_entry_mut<A: FrameAllocator<Frame = Frame>>(
        &self,
        allocator: &A,
        table_phys: HostPhysAddr,
        index: usize,
    ) -> *mut EptEntry {
        let table_virt = allocator.phys_to_virt(table_phys) as *mut EptEntry;
        unsafe { table_virt.add(index) }
    }

    // Helper: Get or create an entry, returning a mutable pointer
    fn get_or_create_entry<A: FrameAllocator<Frame = Frame>>(
        &self,
        allocator: &A,
        table_phys: HostPhysAddr,
        index: usize,
    ) -> Result<*mut EptEntry, A::Error> {
        Ok(self.get_entry_mut(allocator, table_phys, index))
    }

    // Helper: Ensure an entry points to a valid table, allocating if needed
    fn ensure_table<A: FrameAllocator<Frame = Frame>>(
        &mut self,
        allocator: &mut A,
        entry: *mut EptEntry,
        perms: EptPermissions,
    ) -> Result<HostPhysAddr, A::Error> {
        let current = unsafe { *entry };

        if current.is_present() {
            Ok(current.addr())
        } else {
            // Allocate a new table
            let new_frame = allocator.allocate_frame()?;
            let new_phys = A::frame_phys_addr(&new_frame);

            // Zero it out
            let table_virt = allocator.phys_to_virt(new_phys);
            unsafe {
                core::ptr::write_bytes(table_virt, 0, 4096);
            }

            // Set the entry to point to the new table
            unsafe {
                *entry = EptEntry::table_entry(new_phys, perms);
            }

            // Store the frame so it gets freed when EPT is dropped
            ept_vec_push(&mut self.frames, new_frame);

            Ok(new_phys)
        }
    }

    /// Returns the number of allocated frames (page table nodes + PML4).
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Clone this EPT for forking, changing all leaf permissions to READ_EXECUTE.
    ///
    /// This creates a deep copy of the EPT structure where:
    /// - All intermediate tables (PML4, PDPT, PD) are newly allocated
    /// - All leaf entries (PT) keep their host physical addresses but get R+X (no W)
    ///
    /// This enables copy-on-write: the forked VM can read all pages but writes
    /// will cause EPT violations that trigger page copying.
    pub fn clone_for_fork<A: FrameAllocator<Frame = Frame>>(
        &self,
        allocator: &mut A,
    ) -> Result<Self, A::Error> {
        let mut frames = ept_vec_with_capacity(self.frames.len());

        // Allocate new PML4
        let new_pml4_frame = allocator.allocate_frame()?;
        let new_pml4_phys = A::frame_phys_addr(&new_pml4_frame);
        let new_pml4_virt = allocator.phys_to_virt(new_pml4_phys);
        unsafe {
            core::ptr::write_bytes(new_pml4_virt, 0, 4096);
        }

        ept_vec_push(&mut frames, new_pml4_frame);

        let src_pml4 = allocator.phys_to_virt(self.pml4_phys) as *const EptEntry;
        let dst_pml4 = new_pml4_virt as *mut EptEntry;

        // Walk source PML4
        for pml4_idx in 0..512 {
            let src_pml4e = unsafe { &*src_pml4.add(pml4_idx) };
            if !src_pml4e.is_present() {
                continue;
            }

            // Allocate new PDPT
            let new_pdpt_frame = allocator.allocate_frame()?;
            let new_pdpt_phys = A::frame_phys_addr(&new_pdpt_frame);
            let new_pdpt_virt = allocator.phys_to_virt(new_pdpt_phys);
            unsafe {
                core::ptr::write_bytes(new_pdpt_virt, 0, 4096);
            }

            ept_vec_push(&mut frames, new_pdpt_frame);

            // Set PML4 entry pointing to new PDPT (same permissions as source)
            unsafe {
                *dst_pml4.add(pml4_idx) =
                    EptEntry::table_entry(new_pdpt_phys, src_pml4e.permissions());
            }

            let src_pdpt = allocator.phys_to_virt(src_pml4e.addr()) as *const EptEntry;
            let dst_pdpt = new_pdpt_virt as *mut EptEntry;

            // Walk source PDPT
            for pdpt_idx in 0..512 {
                let src_pdpte = unsafe { &*src_pdpt.add(pdpt_idx) };
                if !src_pdpte.is_present() {
                    continue;
                }

                // Allocate new PD
                let new_pd_frame = allocator.allocate_frame()?;
                let new_pd_phys = A::frame_phys_addr(&new_pd_frame);
                let new_pd_virt = allocator.phys_to_virt(new_pd_phys);
                unsafe {
                    core::ptr::write_bytes(new_pd_virt, 0, 4096);
                }

                ept_vec_push(&mut frames, new_pd_frame);

                // Set PDPT entry pointing to new PD
                unsafe {
                    *dst_pdpt.add(pdpt_idx) =
                        EptEntry::table_entry(new_pd_phys, src_pdpte.permissions());
                }

                let src_pd = allocator.phys_to_virt(src_pdpte.addr()) as *const EptEntry;
                let dst_pd = new_pd_virt as *mut EptEntry;

                // Walk source PD
                for pd_idx in 0..512 {
                    let src_pde = unsafe { &*src_pd.add(pd_idx) };
                    if !src_pde.is_present() {
                        continue;
                    }

                    // Allocate new PT
                    let new_pt_frame = allocator.allocate_frame()?;
                    let new_pt_phys = A::frame_phys_addr(&new_pt_frame);
                    let new_pt_virt = allocator.phys_to_virt(new_pt_phys);
                    unsafe {
                        core::ptr::write_bytes(new_pt_virt, 0, 4096);
                    }

                    ept_vec_push(&mut frames, new_pt_frame);

                    // Set PD entry pointing to new PT
                    unsafe {
                        *dst_pd.add(pd_idx) =
                            EptEntry::table_entry(new_pt_phys, src_pde.permissions());
                    }

                    let src_pt = allocator.phys_to_virt(src_pde.addr()) as *const EptEntry;
                    let dst_pt = new_pt_virt as *mut EptEntry;

                    // Copy PT entries, changing permissions to READ_EXECUTE
                    for pt_idx in 0..512 {
                        let src_pte = unsafe { *src_pt.add(pt_idx) };
                        if !src_pte.is_present() {
                            continue;
                        }

                        // Create new entry with same host physical but R+X (no W)
                        let host_phys = src_pte.addr();
                        // Use WriteBack - the only memory type we currently support
                        let new_entry = EptEntry::page_entry_4k(
                            host_phys,
                            EptPermissions::READ_EXECUTE,
                            EptMemoryType::WriteBack,
                        );

                        unsafe {
                            *dst_pt.add(pt_idx) = new_entry;
                        }
                    }
                }
            }
        }

        Ok(Self {
            pml4_phys: new_pml4_phys,
            frames,
        })
    }
}
