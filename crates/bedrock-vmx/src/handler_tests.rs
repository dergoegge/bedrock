// SPDX-License-Identifier: GPL-2.0

//! Tests for BedrockHandler.
//!
//! These tests verify the handler's VMX state management and VM tracking functionality.
//! In the new architecture, VMs are owned by file descriptors (via anon_inodes),
//! and the handler only maintains weak references for tracking.

extern crate std;

use std::boxed::Box;
use std::cell::RefCell;
use std::sync::Mutex;

use core::ptr::NonNull;

/// Global lock to ensure multi-CPU tests run serially.
/// Tests using GLOBAL_TEST_STATE must acquire this lock.
static MULTI_CPU_TEST_LOCK: Mutex<()> = Mutex::new(());

use crate::{
    registers::{msr, Cr0, Cr3, Cr4, CrAccess, CrError, MsrAccess, MsrError},
    traits::{
        HostPhysAddr, InveptError, InvvpidError, Kernel, Machine, Page, VmxBasic, VmxCapabilities,
        VmxCpu, VmxCpuInitError, VmxOnRegion,
    },
    Vmx, VmxInitError, VmxoffError, VmxonError,
};
use memory::VirtAddr;

use crate::BedrockHandler;

/// Mock Page for testing.
/// Uses a heap-allocated buffer to provide a valid virtual address.
struct MockPage {
    buffer: Box<[u8; 4096]>,
}

impl MockPage {
    fn new() -> Self {
        Self {
            buffer: Box::new([0u8; 4096]),
        }
    }
}

impl Page for MockPage {
    fn physical_address(&self) -> HostPhysAddr {
        // In tests, we use the virtual address as a fake physical address
        HostPhysAddr::new(self.buffer.as_ptr() as u64)
    }

    fn virtual_address(&self) -> VirtAddr {
        VirtAddr::new(self.buffer.as_ptr() as u64)
    }
}

/// Mock guest memory for testing - always fails allocation.
struct MockGuestMemory;

impl crate::traits::GuestMemory for MockGuestMemory {
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

/// Mock Kernel for testing.
struct MockKernel;

impl Kernel for MockKernel {
    type P = MockPage;
    type G = MockGuestMemory;

    fn alloc_zeroed_page(&self) -> Option<Self::P> {
        Some(MockPage::new())
    }

    fn alloc_guest_memory(&self, _size: usize) -> Option<Self::G> {
        // Tests don't need real guest memory allocation
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

/// Mock MSR access for testing.
struct MockMsrAccess;

impl MsrAccess for MockMsrAccess {
    fn read_msr(&self, _msr: u32) -> Result<u64, MsrError> {
        Err(MsrError::InvalidAddress)
    }

    fn write_msr(&self, _msr: u32, _value: u64) -> Result<(), MsrError> {
        Ok(())
    }
}

/// Mock CR access for testing.
struct MockCrAccess;

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

use crate::registers::{DescriptorTableAccess, Gdtr, Idtr, SegmentSelector};

/// Mock descriptor table access for testing.
struct MockDescriptorTableAccess;

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

/// Mock VMXON region for testing.
struct MockVmxOnRegion;

impl VmxOnRegion for MockVmxOnRegion {
    type M = MockMachine;

    fn from_page(_page: MockPage) -> Self {
        MockVmxOnRegion
    }
}

/// Mock VmxCpu for testing.
struct MockVmxCpu {
    capabilities: RefCell<VmxCapabilities>,
    vmxon: RefCell<bool>,
    vmxon_region: RefCell<Option<MockVmxOnRegion>>,
}

static mut MOCK_VCPU: Option<MockVmxCpu> = None;

impl VmxCpu for MockVmxCpu {
    type M = MockMachine;
    type R = MockVmxOnRegion;

    fn capabilities(&self) -> &VmxCapabilities {
        // SAFETY: This is a test mock; the borrow will not outlive the RefCell
        unsafe { &*self.capabilities.as_ptr() }
    }

    fn is_vmxon(&self) -> bool {
        *self.vmxon.borrow()
    }

    fn set_vmxon(&self, enabled: bool) {
        *self.vmxon.borrow_mut() = enabled;
    }

    fn set_capabilities(&self, caps: VmxCapabilities) {
        *self.capabilities.borrow_mut() = caps;
    }

    fn set_vmxon_region(&self, region: Self::R) {
        *self.vmxon_region.borrow_mut() = Some(region);
    }
}

/// Mock Machine for testing.
struct MockMachine;

impl Machine for MockMachine {
    type P = MockPage;
    type K = MockKernel;
    type M = MockMsrAccess;
    type Vcpu = MockVmxCpu;
    type C = MockCrAccess;
    type D = MockDescriptorTableAccess;
    type V = MockVmx;

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

/// Mock VMX implementation for testing.
struct MockVmx;

static mut BASIC_INFO: Option<VmxBasic> = None;

#[allow(static_mut_refs)]
impl Vmx for MockVmx {
    type M = MockMachine;

    fn is_supported() -> bool {
        true
    }

    fn initialize(_machine: &Self::M) -> Result<(), VmxInitError> {
        Ok(())
    }

    fn current_vcpu() -> &'static <Self::M as Machine>::Vcpu {
        unsafe { MOCK_VCPU.as_ref().unwrap() }
    }

    fn basic_info() -> &'static VmxBasic {
        unsafe { BASIC_INFO.as_ref().unwrap() }
    }

    fn set_basic_info(basic: VmxBasic) {
        unsafe {
            BASIC_INFO = Some(basic);
        }
    }

    fn vmxon(_phys_addr: HostPhysAddr) -> Result<(), VmxonError> {
        Ok(())
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
// Multi-CPU Mock Infrastructure for VMX Initialization Testing
// =============================================================================

const MAX_TEST_CPUS: usize = 8;

/// Per-CPU state for multi-CPU simulation.
#[derive(Clone, Copy, Default)]
struct MultiCpuState {
    /// Whether VMX is enabled (VMXON executed).
    vmxon: bool,
    /// VMX capabilities read from MSRs.
    capabilities: VmxCapabilities,
    /// Whether VMXON region is allocated.
    has_vmxon_region: bool,
    /// CR4 value for this CPU.
    cr4: u64,
    /// Feature control MSR value for this CPU.
    /// Feature control starts unlocked with VMX disabled.
    feature_control: u64,
}

/// Configuration for MSR behavior in tests.
#[derive(Clone, Copy)]
struct MsrConfig {
    vmx_basic: u64,
    pinbased_ctls: u64,
    procbased_ctls: u64,
    procbased_ctls2: u64,
    exit_ctls: u64,
    entry_ctls: u64,
    cr0_fixed0: u64,
    cr0_fixed1: u64,
    cr4_fixed0: u64,
    cr4_fixed1: u64,
}

impl Default for MsrConfig {
    fn default() -> Self {
        Self {
            // Revision ID = 1, VMCS size = 4096 bytes, memory type = WB
            vmx_basic: 0x0000_0001_0000_0001 | (1u64 << 50),
            // Allow all bits to be 0 or 1 (low 32 bits = must be 1, high 32 bits = can be 1)
            pinbased_ctls: 0xFFFF_FFFF_0000_0000,
            procbased_ctls: 0xFFFF_FFFF_0000_0000,
            procbased_ctls2: 0xFFFF_FFFF_0000_0000,
            exit_ctls: 0xFFFF_FFFF_0000_0000,
            entry_ctls: 0xFFFF_FFFF_0000_0000,
            cr0_fixed0: 0,
            cr0_fixed1: 0xFFFF_FFFF_FFFF_FFFF,
            cr4_fixed0: 0,
            cr4_fixed1: 0xFFFF_FFFF_FFFF_FFFF,
        }
    }
}

/// Error injection configuration for testing error paths.
#[derive(Clone, Copy, Default)]
struct ErrorConfig {
    /// If Some, VMXON will fail on this CPU with the given error code.
    /// 0 = InvalidPointer, 1 = AlreadyInVmxOperation
    vmxon_fails_on_cpu: Option<(usize, u8)>,
    /// If true, VMX is not supported (CPUID check fails).
    vmx_not_supported: bool,
    /// If true, page allocation fails.
    page_alloc_fails: bool,
}

const DEFAULT_CPU_STATE: MultiCpuState = MultiCpuState {
    vmxon: false,
    capabilities: VmxCapabilities {
        pin_based_exec_ctrl: 0,
        cpu_based_exec_ctrl: 0,
        cpu_based_exec_ctrl2: 0,
        vmexit_ctrl: 0,
        vmentry_ctrl: 0,
        cr0_fixed0: 0,
        cr0_fixed1: 0,
        cr4_fixed0: 0,
        cr4_fixed1: 0,
        has_ept: false,
        has_vpid: false,
    },
    has_vmxon_region: false,
    cr4: 0,
    feature_control: 0,
};

const DEFAULT_MSR_CONFIG: MsrConfig = MsrConfig {
    vmx_basic: 0x0000_0001_0000_0001 | (1u64 << 50),
    pinbased_ctls: 0xFFFF_FFFF_0000_0000,
    procbased_ctls: 0xFFFF_FFFF_0000_0000,
    procbased_ctls2: 0xFFFF_FFFF_0000_0000,
    exit_ctls: 0xFFFF_FFFF_0000_0000,
    entry_ctls: 0xFFFF_FFFF_0000_0000,
    cr0_fixed0: 0,
    cr0_fixed1: 0xFFFF_FFFF_FFFF_FFFF,
    cr4_fixed0: 0,
    cr4_fixed1: 0xFFFF_FFFF_FFFF_FFFF,
};

/// Global test state - accessed via unsafe static mut.
/// SAFETY: Tests are single-threaded, so concurrent access is not a concern.
struct GlobalTestState {
    num_cpus: usize,
    current_cpu: usize,
    cpu_states: [MultiCpuState; MAX_TEST_CPUS],
    msr_config: MsrConfig,
    error_config: ErrorConfig,
    basic_info: Option<VmxBasic>,
}

impl GlobalTestState {
    const fn new() -> Self {
        Self {
            num_cpus: 1,
            current_cpu: 0,
            cpu_states: [DEFAULT_CPU_STATE; MAX_TEST_CPUS],
            msr_config: DEFAULT_MSR_CONFIG,
            error_config: ErrorConfig {
                vmxon_fails_on_cpu: None,
                vmx_not_supported: false,
                page_alloc_fails: false,
            },
            basic_info: None,
        }
    }

    fn reset(&mut self, num_cpus: usize, error_config: ErrorConfig) {
        self.num_cpus = num_cpus;
        self.current_cpu = 0;
        self.msr_config = MsrConfig::default();
        self.error_config = error_config;
        self.basic_info = None;
        for state in self.cpu_states.iter_mut() {
            *state = MultiCpuState::default();
        }
    }
}

static mut GLOBAL_TEST_STATE: GlobalTestState = GlobalTestState::new();

/// SAFETY: All access is single-threaded in tests.
fn with_state<R>(f: impl FnOnce(&GlobalTestState) -> R) -> R {
    unsafe { f(&*core::ptr::addr_of!(GLOBAL_TEST_STATE)) }
}

/// SAFETY: All access is single-threaded in tests.
fn with_state_mut<R>(f: impl FnOnce(&mut GlobalTestState) -> R) -> R {
    unsafe { f(&mut *core::ptr::addr_of_mut!(GLOBAL_TEST_STATE)) }
}

/// Multi-CPU aware Page implementation with guaranteed 4KB alignment.
struct MultiCpuPage {
    // Over-allocate to ensure we can find a 4KB-aligned region
    buffer: Box<[u8; 8192]>,
    aligned_offset: usize,
}

impl MultiCpuPage {
    fn new() -> Self {
        let buffer = Box::new([0u8; 8192]);
        let ptr = buffer.as_ptr() as usize;
        // Find offset to next 4KB boundary
        let aligned_offset = if ptr & 0xFFF == 0 {
            0
        } else {
            0x1000 - (ptr & 0xFFF)
        };
        Self {
            buffer,
            aligned_offset,
        }
    }

    fn aligned_ptr(&self) -> *const u8 {
        unsafe { self.buffer.as_ptr().add(self.aligned_offset) }
    }
}

impl Page for MultiCpuPage {
    fn physical_address(&self) -> HostPhysAddr {
        // Return the 4KB-aligned address
        HostPhysAddr::new(self.aligned_ptr() as u64)
    }

    fn virtual_address(&self) -> VirtAddr {
        VirtAddr::new(self.aligned_ptr() as u64)
    }
}

/// Multi-CPU aware Kernel implementation.
struct MultiCpuKernel;

impl Kernel for MultiCpuKernel {
    type P = MultiCpuPage;
    type G = MockGuestMemory;

    fn alloc_zeroed_page(&self) -> Option<Self::P> {
        if with_state(|s| s.error_config.page_alloc_fails) {
            None
        } else {
            Some(MultiCpuPage::new())
        }
    }

    fn alloc_guest_memory(&self, _size: usize) -> Option<Self::G> {
        None
    }

    fn phys_to_virt(&self, phys: HostPhysAddr) -> *mut u8 {
        phys.as_u64() as *mut u8
    }

    fn call_on_all_cpus_with_data<F, T, E>(&self, data: &T, func: F) -> Result<(), E>
    where
        F: Fn(&T) -> Result<(), E> + Sync + Send,
        T: Sync,
        E: Send,
    {
        let num_cpus = with_state(|s| s.num_cpus);
        for cpu_id in 0..num_cpus {
            with_state_mut(|s| s.current_cpu = cpu_id);
            func(data)?;
        }
        Ok(())
    }

    fn current_cpu_id(&self) -> usize {
        with_state(|s| s.current_cpu)
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

/// Multi-CPU aware MSR access implementation.
struct MultiCpuMsrAccess;

impl MsrAccess for MultiCpuMsrAccess {
    fn read_msr(&self, msr_addr: u32) -> Result<u64, MsrError> {
        with_state(|s| {
            let config = &s.msr_config;
            match msr_addr {
                msr::IA32_FEATURE_CONTROL => Ok(s.cpu_states[s.current_cpu].feature_control),
                msr::IA32_VMX_BASIC => Ok(config.vmx_basic),
                msr::IA32_VMX_PINBASED_CTLS => Ok(config.pinbased_ctls),
                msr::IA32_VMX_PROCBASED_CTLS => Ok(config.procbased_ctls),
                msr::IA32_VMX_PROCBASED_CTLS2 => Ok(config.procbased_ctls2),
                msr::IA32_VMX_EXIT_CTLS => Ok(config.exit_ctls),
                msr::IA32_VMX_ENTRY_CTLS => Ok(config.entry_ctls),
                msr::IA32_VMX_CR0_FIXED0 => Ok(config.cr0_fixed0),
                msr::IA32_VMX_CR0_FIXED1 => Ok(config.cr0_fixed1),
                msr::IA32_VMX_CR4_FIXED0 => Ok(config.cr4_fixed0),
                msr::IA32_VMX_CR4_FIXED1 => Ok(config.cr4_fixed1),
                _ => Err(MsrError::InvalidAddress),
            }
        })
    }

    fn write_msr(&self, msr_addr: u32, value: u64) -> Result<(), MsrError> {
        with_state_mut(|s| {
            if msr_addr == msr::IA32_FEATURE_CONTROL {
                s.cpu_states[s.current_cpu].feature_control = value;
            }
            Ok(())
        })
    }
}

/// Multi-CPU aware CR access implementation.
struct MultiCpuCrAccess;

impl CrAccess for MultiCpuCrAccess {
    fn read_cr0(&self) -> Result<Cr0, CrError> {
        Ok(Cr0::new(0))
    }
    fn read_cr3(&self) -> Result<Cr3, CrError> {
        Ok(Cr3::new(0))
    }
    fn read_cr4(&self) -> Result<Cr4, CrError> {
        Ok(Cr4::new(with_state(|s| s.cpu_states[s.current_cpu].cr4)))
    }
    fn write_cr4(&self, cr4: &Cr4) -> Result<(), CrError> {
        with_state_mut(|s| s.cpu_states[s.current_cpu].cr4 = cr4.bits());
        Ok(())
    }
    fn set_vmxe(&self) -> Result<(), CrError> {
        with_state_mut(|s| s.cpu_states[s.current_cpu].cr4 |= Cr4::VMXE);
        Ok(())
    }
    fn clear_vmxe(&self) -> Result<(), CrError> {
        with_state_mut(|s| s.cpu_states[s.current_cpu].cr4 &= !Cr4::VMXE);
        Ok(())
    }
}

/// Multi-CPU aware VMXON region.
struct MultiCpuVmxOnRegion {
    _page: MultiCpuPage,
}

impl VmxOnRegion for MultiCpuVmxOnRegion {
    type M = MultiCpuMachine;

    fn from_page(_page: MultiCpuPage) -> Self {
        Self {
            _page: MultiCpuPage::new(),
        }
    }
}

/// Multi-CPU aware VmxCpu implementation.
struct MultiCpuVmxCpu;

// SAFETY: We're lying to the compiler - this isn't really Send+Sync safe,
// but our tests are single-threaded so it doesn't matter.
unsafe impl Send for MultiCpuVmxCpu {}
unsafe impl Sync for MultiCpuVmxCpu {}

impl VmxCpu for MultiCpuVmxCpu {
    type M = MultiCpuMachine;
    type R = MultiCpuVmxOnRegion;

    fn capabilities(&self) -> &VmxCapabilities {
        // Leak to get 'static lifetime - acceptable in tests
        Box::leak(Box::new(with_state(|s| {
            s.cpu_states[s.current_cpu].capabilities
        })))
    }

    fn is_vmxon(&self) -> bool {
        with_state(|s| s.cpu_states[s.current_cpu].vmxon)
    }

    fn set_vmxon(&self, enabled: bool) {
        with_state_mut(|s| s.cpu_states[s.current_cpu].vmxon = enabled);
    }

    fn set_capabilities(&self, caps: VmxCapabilities) {
        with_state_mut(|s| s.cpu_states[s.current_cpu].capabilities = caps);
    }

    fn set_vmxon_region(&self, _region: Self::R) {
        with_state_mut(|s| s.cpu_states[s.current_cpu].has_vmxon_region = true);
    }
}

/// Multi-CPU aware Machine implementation.
struct MultiCpuMachine;

// SAFETY: Single-threaded tests only.
unsafe impl Send for MultiCpuMachine {}
unsafe impl Sync for MultiCpuMachine {}

impl Machine for MultiCpuMachine {
    type P = MultiCpuPage;
    type K = MultiCpuKernel;
    type M = MultiCpuMsrAccess;
    type C = MultiCpuCrAccess;
    type D = MockDescriptorTableAccess;
    type V = MultiCpuVmx;
    type Vcpu = MultiCpuVmxCpu;

    fn kernel(&self) -> &Self::K {
        &MultiCpuKernel
    }
    fn msr_access(&self) -> &Self::M {
        &MultiCpuMsrAccess
    }
    fn cr_access(&self) -> &Self::C {
        &MultiCpuCrAccess
    }
    fn descriptor_table_access(&self) -> &Self::D {
        &MockDescriptorTableAccess
    }
}

static MULTI_CPU_VCPU: MultiCpuVmxCpu = MultiCpuVmxCpu;
static mut MULTI_CPU_BASIC_INFO: VmxBasic = VmxBasic {
    vmcs_revision_id: 0,
    vmcs_size: 0,
    mem_type_wb: false,
    io_exit_info: false,
    vmx_flex_controls: false,
};

/// Multi-CPU aware VMX implementation that uses the default initialize().
struct MultiCpuVmx;

impl Vmx for MultiCpuVmx {
    type M = MultiCpuMachine;

    fn is_supported() -> bool {
        !with_state(|s| s.error_config.vmx_not_supported)
    }

    // Uses the default implementation from the trait!
    // This exercises the real VMX initialization logic.

    fn current_vcpu() -> &'static <Self::M as Machine>::Vcpu {
        &MULTI_CPU_VCPU
    }

    fn basic_info() -> &'static VmxBasic {
        unsafe { &*core::ptr::addr_of!(MULTI_CPU_BASIC_INFO) }
    }

    fn set_basic_info(basic: VmxBasic) {
        unsafe {
            MULTI_CPU_BASIC_INFO = basic;
        }
    }

    fn vmxon(phys_addr: HostPhysAddr) -> Result<(), VmxonError> {
        with_state(|s| {
            if let Some((fail_cpu, error_code)) = s.error_config.vmxon_fails_on_cpu {
                if s.current_cpu == fail_cpu {
                    return Err(if error_code == 0 {
                        VmxonError::InvalidPointer
                    } else {
                        VmxonError::AlreadyInVmxOperation
                    });
                }
            }
            if phys_addr.as_u64() & 0xFFF != 0 {
                return Err(VmxonError::InvalidPointer);
            }
            Ok(())
        })
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

/// Helper to set up multi-CPU test environment.
fn setup_multi_cpu_test(num_cpus: usize) -> MultiCpuMachine {
    with_state_mut(|s| s.reset(num_cpus, ErrorConfig::default()));
    MultiCpuMachine
}

/// Helper to set up multi-CPU test with error injection.
fn setup_multi_cpu_test_with_errors(num_cpus: usize, error_config: ErrorConfig) -> MultiCpuMachine {
    with_state_mut(|s| s.reset(num_cpus, error_config));
    MultiCpuMachine
}

/// Get a snapshot of the CPU states for verification.
fn get_cpu_states() -> [MultiCpuState; MAX_TEST_CPUS] {
    with_state(|s| s.cpu_states)
}

// =============================================================================
// Handler VM Tracking Tests
// =============================================================================

/// Mock VM for testing VM tracking.
struct MockVm;

#[test]
fn test_handler_new_succeeds() {
    let machine = MockMachine;
    let handler = BedrockHandler::<MockVmx, 64>::new(&machine);
    assert!(handler.is_ok());
}

#[test]
fn test_handler_alloc_vm_id() {
    let machine = MockMachine;
    let mut handler = BedrockHandler::<MockVmx, 64>::new(&machine).unwrap();

    assert_eq!(handler.alloc_vm_id(), Some(1));
    assert_eq!(handler.alloc_vm_id(), Some(2));
    assert_eq!(handler.alloc_vm_id(), Some(3));
}

#[test]
fn test_handler_vm_tracking() {
    let machine = MockMachine;
    let mut handler = BedrockHandler::<MockVmx, 64>::new(&machine).unwrap();

    // Simulate adding VMs
    let vm1 = Box::new(MockVm);
    let vm2 = Box::new(MockVm);
    let vm1_ptr = Box::into_raw(vm1);
    let vm2_ptr = Box::into_raw(vm2);

    // Add VMs to tracking
    handler.add_vm(NonNull::new(vm1_ptr).unwrap(), 1);
    handler.add_vm(NonNull::new(vm2_ptr).unwrap(), 2);

    // Remove VMs (verifies remove_vm doesn't panic)
    handler.remove_vm(vm1_ptr);
    handler.remove_vm(vm2_ptr);

    // Clean up
    unsafe {
        let _ = Box::from_raw(vm1_ptr);
        let _ = Box::from_raw(vm2_ptr);
    }
}

#[test]
fn test_handler_vm_limit() {
    let machine = MockMachine;
    let mut handler = BedrockHandler::<MockVmx, 2>::new(&machine).unwrap();

    assert!(handler.can_create_vm());
    assert!(handler.alloc_vm_id().is_some());

    // Simulate adding a VM
    let vm1 = Box::new(MockVm);
    let vm1_ptr = Box::into_raw(vm1);
    handler.add_vm(NonNull::new(vm1_ptr).unwrap(), 1);

    assert!(handler.can_create_vm());
    assert!(handler.alloc_vm_id().is_some());

    // Add second VM
    let vm2 = Box::new(MockVm);
    let vm2_ptr = Box::into_raw(vm2);
    handler.add_vm(NonNull::new(vm2_ptr).unwrap(), 2);

    // Should not be able to create more
    assert!(!handler.can_create_vm());
    assert!(handler.alloc_vm_id().is_none());

    // Clean up
    unsafe {
        let _ = Box::from_raw(vm1_ptr);
        let _ = Box::from_raw(vm2_ptr);
    }
}

// =============================================================================
// Multi-CPU VMX Initialization Tests
// =============================================================================

#[test]
fn test_handler_new_initializes_vmx_on_all_cpus() {
    let _lock = MULTI_CPU_TEST_LOCK.lock().unwrap();
    let machine = setup_multi_cpu_test(4);

    // Create the handler - this should initialize VMX on all 4 CPUs
    let _handler = BedrockHandler::<MultiCpuVmx, 64>::new(&machine).unwrap();

    // Verify VMX was initialized on all CPUs
    let cpu_states = get_cpu_states();
    for (cpu_id, state) in cpu_states.iter().enumerate().take(4) {
        assert!(state.vmxon, "CPU {} should have VMXON enabled", cpu_id);
        assert!(
            state.has_vmxon_region,
            "CPU {} should have VMXON region allocated",
            cpu_id
        );
        // Verify CR4.VMXE (bit 13) is set
        assert!(
            state.cr4 & (1 << 13) != 0,
            "CPU {} should have CR4.VMXE set",
            cpu_id
        );
        // Verify feature control MSR was configured (lock bit + VMX enable bit)
        let lock_bit = 1u64;
        let vmx_outside_smx = 1u64 << 2;
        assert!(
            state.feature_control & lock_bit != 0,
            "CPU {} should have feature control lock bit set",
            cpu_id
        );
        assert!(
            state.feature_control & vmx_outside_smx != 0,
            "CPU {} should have VMX outside SMX enabled",
            cpu_id
        );
    }
}

#[test]
fn test_handler_new_fails_if_vmx_unsupported() {
    let _lock = MULTI_CPU_TEST_LOCK.lock().unwrap();
    let error_config = ErrorConfig {
        vmx_not_supported: true,
        ..Default::default()
    };
    let machine = setup_multi_cpu_test_with_errors(4, error_config);

    let result = BedrockHandler::<MultiCpuVmx, 64>::new(&machine);

    assert!(matches!(result, Err(VmxInitError::Unsupported)));
}

#[test]
fn test_handler_new_fails_on_feature_control_locked() {
    let _lock = MULTI_CPU_TEST_LOCK.lock().unwrap();
    // Set up CPU 0 with feature control locked but VMX not enabled
    let machine = setup_multi_cpu_test(4);

    // Lock the feature control MSR on CPU 0 without enabling VMX
    with_state_mut(|s| s.cpu_states[0].feature_control = 1); // Lock bit set, VMX not enabled

    let result = BedrockHandler::<MultiCpuVmx, 64>::new(&machine);

    // Should fail with feature control config error
    match result {
        Err(VmxInitError::FailedToEnableCPU { core: 0, error }) => {
            assert!(matches!(
                error,
                VmxCpuInitError::FeatureControlConfigFailed(_)
            ));
        }
        Err(e) => panic!("Expected FailedToEnableCPU error on core 0, got {:?}", e),
        Ok(_) => panic!("Expected error but got Ok"),
    }
}

#[test]
fn test_handler_new_fails_on_vmxon_failure() {
    let _lock = MULTI_CPU_TEST_LOCK.lock().unwrap();
    let error_config = ErrorConfig {
        vmxon_fails_on_cpu: Some((2, 0)), // 0 = InvalidPointer
        ..Default::default()
    };
    let machine = setup_multi_cpu_test_with_errors(4, error_config);

    let result = BedrockHandler::<MultiCpuVmx, 64>::new(&machine);

    // Should fail on CPU 2 with VMXON error
    match result {
        Err(VmxInitError::FailedToEnableCPU { core: 2, error }) => {
            assert!(matches!(error, VmxCpuInitError::VmxonAllocFailed(_)));
        }
        Err(e) => panic!("Expected FailedToEnableCPU error on core 2, got {:?}", e),
        Ok(_) => panic!("Expected error but got Ok"),
    }
}

#[test]
fn test_handler_new_fails_on_page_alloc_failure() {
    let _lock = MULTI_CPU_TEST_LOCK.lock().unwrap();
    let error_config = ErrorConfig {
        page_alloc_fails: true,
        ..Default::default()
    };
    let machine = setup_multi_cpu_test_with_errors(4, error_config);

    let result = BedrockHandler::<MultiCpuVmx, 64>::new(&machine);

    // Should fail with VMXON alloc error
    match result {
        Err(VmxInitError::FailedToEnableCPU { error, .. }) => {
            assert!(matches!(error, VmxCpuInitError::VmxonAllocFailed(_)));
        }
        Err(e) => panic!("Expected FailedToEnableCPU error, got {:?}", e),
        Ok(_) => panic!("Expected error but got Ok"),
    }
}
