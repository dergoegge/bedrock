// SPDX-License-Identifier: GPL-2.0

//! Shared test mocks for bedrock crates.
//!
//! This module provides mock implementations of core traits for testing in userland.
//! Available when the `test-utils` feature is enabled.

extern crate std;

use core::cell::RefCell;
use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::collections::HashMap;

use memory::{HostPhysAddr, VirtAddr};

use crate::fields::{VmcsField16, VmcsField32, VmcsField64, VmcsFieldNatural};
use crate::registers::{
    Cr0, Cr3, Cr4, CrAccess, CrError, DescriptorTableAccess, Gdtr, Idtr, MsrAccess, MsrError,
    SegmentSelector,
};
use crate::traits::{
    GuestMemory, InveptError, InvvpidError, Kernel, Machine, Page, VirtualMachineControlStructure,
    VmxBasic, VmxCapabilities, VmxCpu, VmxOnRegion, VmxoffError, VmxonError,
};
use crate::Vmx;

// =============================================================================
// Page Mock
// =============================================================================

/// A page backed by real memory for testing.
pub struct MockPage {
    /// Raw pointer to 4KB-aligned memory.
    data: *mut u8,
}

impl MockPage {
    const PAGE_SIZE: usize = 4096;

    pub fn new() -> Self {
        let layout = Layout::from_size_align(Self::PAGE_SIZE, Self::PAGE_SIZE).unwrap();
        let data = unsafe { alloc_zeroed(layout) };
        assert!(!data.is_null(), "Page allocation failed");
        Self { data }
    }
}

impl Default for MockPage {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MockPage {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(Self::PAGE_SIZE, Self::PAGE_SIZE).unwrap();
        unsafe { dealloc(self.data, layout) };
    }
}

impl Page for MockPage {
    fn physical_address(&self) -> HostPhysAddr {
        // Use virtual address as fake physical address in tests
        HostPhysAddr::new(self.data as u64)
    }

    fn virtual_address(&self) -> VirtAddr {
        VirtAddr::new(self.data as u64)
    }
}

// =============================================================================
// Guest Memory Mock
// =============================================================================

/// Mock guest memory for testing - always fails allocation.
pub struct MockGuestMemory;

impl GuestMemory for MockGuestMemory {
    fn size(&self) -> usize {
        0
    }

    fn virt_addr(&self) -> VirtAddr {
        VirtAddr::new(0)
    }

    fn page_phys_addr(&self, _page_offset: usize) -> Option<HostPhysAddr> {
        None
    }
}

// =============================================================================
// Kernel Mock
// =============================================================================

/// Mock Kernel for testing.
pub struct MockKernel;

impl Kernel for MockKernel {
    type P = MockPage;
    type G = MockGuestMemory;

    fn alloc_zeroed_page(&self) -> Option<Self::P> {
        Some(MockPage::new())
    }

    fn alloc_guest_memory(&self, _size: usize) -> Option<Self::G> {
        None
    }

    fn phys_to_virt(&self, phys: HostPhysAddr) -> *mut u8 {
        phys.as_u64() as *mut u8
    }

    fn call_on_all_cpus_with_data<F, T, E>(&self, _data: &T, _func: F) -> Result<(), E>
    where
        F: Fn(&T) -> Result<(), E> + Sync + Send,
        T: Sync,
        E: Send,
    {
        Ok(())
    }

    fn current_cpu_id(&self) -> usize {
        0
    }

    fn need_resched(&self) -> bool {
        false
    }

    fn local_irq_enable(&self) {
        // No-op in tests
    }

    fn local_irq_disable(&self) {
        // No-op in tests
    }
}

// =============================================================================
// MSR Access Mock
// =============================================================================

/// Mock MSR access for testing.
pub struct MockMsrAccess;

impl MsrAccess for MockMsrAccess {
    fn read_msr(&self, _msr: u32) -> Result<u64, MsrError> {
        Err(MsrError::InvalidAddress)
    }

    fn write_msr(&self, _msr: u32, _value: u64) -> Result<(), MsrError> {
        Ok(())
    }
}

// =============================================================================
// CR Access Mock
// =============================================================================

/// Mock CR access for testing.
pub struct MockCrAccess;

impl CrAccess for MockCrAccess {
    fn read_cr0(&self) -> Result<Cr0, CrError> {
        Ok(Cr0::new(0))
    }

    fn read_cr3(&self) -> Result<Cr3, CrError> {
        Ok(Cr3::new(0))
    }

    fn read_cr4(&self) -> Result<Cr4, CrError> {
        Ok(Cr4::new(0))
    }

    fn write_cr4(&self, _cr4: &Cr4) -> Result<(), CrError> {
        Ok(())
    }

    fn set_vmxe(&self) -> Result<(), CrError> {
        Ok(())
    }

    fn clear_vmxe(&self) -> Result<(), CrError> {
        Ok(())
    }
}

// =============================================================================
// Descriptor Table Access Mock
// =============================================================================

/// Mock descriptor table access for testing.
pub struct MockDescriptorTableAccess;

impl DescriptorTableAccess for MockDescriptorTableAccess {
    fn read_cs(&self) -> SegmentSelector {
        SegmentSelector::new(0x10)
    }
    fn read_ss(&self) -> SegmentSelector {
        SegmentSelector::new(0x18)
    }
    fn read_ds(&self) -> SegmentSelector {
        SegmentSelector::new(0x18)
    }
    fn read_es(&self) -> SegmentSelector {
        SegmentSelector::new(0x18)
    }
    fn read_fs(&self) -> SegmentSelector {
        SegmentSelector::new(0)
    }
    fn read_gs(&self) -> SegmentSelector {
        SegmentSelector::new(0)
    }
    fn read_tr(&self) -> SegmentSelector {
        SegmentSelector::new(0x40)
    }
    fn read_tr_base(&self) -> u64 {
        0xFFFF_8000_0000_0000
    }
    fn read_gdtr(&self) -> Gdtr {
        Gdtr::new(0xFFFF_8000_0001_0000, 0x7F)
    }
    fn read_idtr(&self) -> Idtr {
        Idtr::new(0xFFFF_8000_0002_0000, 0xFFF)
    }
}

// =============================================================================
// VMXON Region Mock
// =============================================================================

/// Mock VMXON region for testing.
pub struct MockVmxOnRegion;

impl VmxOnRegion for MockVmxOnRegion {
    type M = MockMachine;

    fn from_page(_page: MockPage) -> Self {
        MockVmxOnRegion
    }
}

// =============================================================================
// VmxCpu Mock
// =============================================================================

/// Mock VmxCpu for testing.
pub struct MockVmxCpu {
    capabilities: VmxCapabilities,
}

impl Default for MockVmxCpu {
    fn default() -> Self {
        Self::new()
    }
}

impl MockVmxCpu {
    pub const fn new() -> Self {
        Self {
            capabilities: VmxCapabilities {
                pin_based_exec_ctrl: 0,
                cpu_based_exec_ctrl: 0,
                cpu_based_exec_ctrl2: 0,
                vmexit_ctrl: 0,
                vmentry_ctrl: 0,
                cr0_fixed0: 0,
                cr0_fixed1: !0,
                cr4_fixed0: 0,
                cr4_fixed1: !0,
                has_ept: true,
                has_vpid: false,
            },
        }
    }
}

unsafe impl Send for MockVmxCpu {}
unsafe impl Sync for MockVmxCpu {}

impl VmxCpu for MockVmxCpu {
    type M = MockMachine;
    type R = MockVmxOnRegion;

    fn capabilities(&self) -> &VmxCapabilities {
        &self.capabilities
    }

    fn is_vmxon(&self) -> bool {
        false
    }

    fn set_vmxon(&self, _: bool) {}

    fn set_capabilities(&self, _: VmxCapabilities) {}

    fn set_vmxon_region(&self, _: Self::R) {}
}

// =============================================================================
// VMX Mock
// =============================================================================

static MOCK_VCPU: MockVmxCpu = MockVmxCpu::new();

static MOCK_BASIC_INFO: VmxBasic = VmxBasic {
    vmcs_revision_id: 0x12345,
    vmcs_size: 4096,
    mem_type_wb: true,
    io_exit_info: false,
    vmx_flex_controls: false,
};

/// Mock VMX implementation for testing.
pub struct MockVmx;

impl Vmx for MockVmx {
    type M = MockMachine;

    fn is_supported() -> bool {
        false
    }

    fn current_vcpu() -> &'static MockVmxCpu {
        &MOCK_VCPU
    }

    fn basic_info() -> &'static VmxBasic {
        &MOCK_BASIC_INFO
    }

    fn set_basic_info(_: VmxBasic) {}

    fn vmxon(_: HostPhysAddr) -> Result<(), VmxonError> {
        Err(VmxonError::InvalidPointer)
    }

    fn vmxoff() -> Result<(), VmxoffError> {
        Ok(())
    }

    fn invept_single_context(_eptp: u64) -> Result<(), InveptError> {
        Ok(())
    }

    fn invvpid_single_context(_vpid: u16) -> Result<(), InvvpidError> {
        Ok(())
    }

    fn invvpid_all_context() -> Result<(), InvvpidError> {
        Ok(())
    }
}

// =============================================================================
// Machine Mock
// =============================================================================

/// Mock Machine for testing.
pub struct MockMachine;

unsafe impl Send for MockMachine {}
unsafe impl Sync for MockMachine {}

impl Machine for MockMachine {
    type P = MockPage;
    type K = MockKernel;
    type M = MockMsrAccess;
    type C = MockCrAccess;
    type D = MockDescriptorTableAccess;
    type V = MockVmx;
    type Vcpu = MockVmxCpu;

    fn kernel(&self) -> &Self::K {
        &MockKernel
    }

    fn msr_access(&self) -> &Self::M {
        &MockMsrAccess
    }

    fn cr_access(&self) -> &Self::C {
        &MockCrAccess
    }

    fn descriptor_table_access(&self) -> &Self::D {
        &MockDescriptorTableAccess
    }
}

// =============================================================================
// VMCS Mock
// =============================================================================

/// Mock VMCS implementation using HashMaps for field storage.
/// Uses RefCell for interior mutability since the trait uses &self for writes.
pub struct MockVmcs {
    fields16: RefCell<HashMap<u32, u16>>,
    fields32: RefCell<HashMap<u32, u32>>,
    fields64: RefCell<HashMap<u32, u64>>,
    fields_natural: RefCell<HashMap<u32, u64>>,
}

impl MockVmcs {
    /// Create a new MockVmcs for testing.
    pub fn new() -> Self {
        Self {
            fields16: RefCell::new(HashMap::new()),
            fields32: RefCell::new(HashMap::new()),
            fields64: RefCell::new(HashMap::new()),
            fields_natural: RefCell::new(HashMap::new()),
        }
    }

    /// Set a 32-bit field directly (for test setup).
    pub fn set_field32(&self, field: VmcsField32, value: u32) {
        self.fields32.borrow_mut().insert(field as u32, value);
    }

    /// Set a natural-width field directly (for test setup).
    pub fn set_field_natural(&self, field: VmcsFieldNatural, value: u64) {
        self.fields_natural.borrow_mut().insert(field as u32, value);
    }

    /// Get a 32-bit field directly (for test verification).
    pub fn get_field32(&self, field: VmcsField32) -> Option<u32> {
        self.fields32.borrow().get(&(field as u32)).copied()
    }

    /// Get a natural-width field directly (for test verification).
    pub fn get_field_natural(&self, field: VmcsFieldNatural) -> Option<u64> {
        self.fields_natural.borrow().get(&(field as u32)).copied()
    }
}

impl Default for MockVmcs {
    fn default() -> Self {
        Self::new()
    }
}

use crate::traits::{VmcsReadError, VmcsReadResult, VmcsWriteResult};

impl VirtualMachineControlStructure for MockVmcs {
    type P = MockPage;
    type M = MockMachine;

    fn clear(&self) -> Result<(), &'static str> {
        Ok(())
    }

    fn load(&self) -> Result<(), &'static str> {
        Ok(())
    }

    fn read16(&self, field: VmcsField16) -> VmcsReadResult<u16> {
        self.fields16
            .borrow()
            .get(&(field as u32))
            .copied()
            .ok_or(VmcsReadError::InvalidField)
    }

    fn read32(&self, field: VmcsField32) -> VmcsReadResult<u32> {
        self.fields32
            .borrow()
            .get(&(field as u32))
            .copied()
            .ok_or(VmcsReadError::InvalidField)
    }

    fn read64(&self, field: VmcsField64) -> VmcsReadResult<u64> {
        self.fields64
            .borrow()
            .get(&(field as u32))
            .copied()
            .ok_or(VmcsReadError::InvalidField)
    }

    fn read_natural(&self, field: VmcsFieldNatural) -> VmcsReadResult<u64> {
        self.fields_natural
            .borrow()
            .get(&(field as u32))
            .copied()
            .ok_or(VmcsReadError::InvalidField)
    }

    fn write16(&self, field: VmcsField16, value: u16) -> VmcsWriteResult {
        self.fields16.borrow_mut().insert(field as u32, value);
        Ok(())
    }

    fn write32(&self, field: VmcsField32, value: u32) -> VmcsWriteResult {
        self.fields32.borrow_mut().insert(field as u32, value);
        Ok(())
    }

    fn write64(&self, field: VmcsField64, value: u64) -> VmcsWriteResult {
        self.fields64.borrow_mut().insert(field as u32, value);
        Ok(())
    }

    fn write_natural(&self, field: VmcsFieldNatural, value: u64) -> VmcsWriteResult {
        self.fields_natural.borrow_mut().insert(field as u32, value);
        Ok(())
    }

    fn vmcs_region_ptr(&self) -> *mut u8 {
        // Return a dummy pointer for tests - memcpy is skipped in cargo builds
        core::ptr::null_mut()
    }

    fn from_parts(_page: MockPage, _revision_id: u32) -> Self
    where
        Self: Sized,
    {
        Self::new()
    }
}

// =============================================================================
// Frame Allocator Mock
// =============================================================================

use crate::traits::CowAllocator;
use bedrock_ept::FrameAllocator;

/// Mock frame allocator for testing.
pub struct MockFrameAllocator {
    next_addr: RefCell<u64>,
}

impl MockFrameAllocator {
    pub fn new() -> Self {
        Self {
            next_addr: RefCell::new(0x1_0000_0000), // Start at 4GB
        }
    }
}

impl Default for MockFrameAllocator {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct MockAllocError;

impl FrameAllocator for MockFrameAllocator {
    type Error = MockAllocError;
    type Frame = MockPage;

    fn allocate_frame(&mut self) -> Result<MockPage, Self::Error> {
        let page = MockPage::new();
        let addr = page.physical_address().as_u64();
        *self.next_addr.borrow_mut() = addr + 4096;
        Ok(page)
    }

    fn frame_phys_addr(frame: &MockPage) -> HostPhysAddr {
        frame.physical_address()
    }

    fn phys_to_virt(&self, phys: HostPhysAddr) -> *mut u8 {
        phys.as_u64() as *mut u8
    }
}

impl CowAllocator<MockPage> for MockFrameAllocator {
    fn allocate_cow_page(&mut self) -> Result<MockPage, Self::Error> {
        Ok(MockPage::new())
    }
}
