#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

use super::{
    allocate_vpid, cpu_based, errors::VmcsAllocError, pin_based, secondary_exec, HostPhysAddr,
    Kernel, Machine, Page, VmcsReadResult, VmcsWriteResult, Vmx, VmxCpu,
};

/// Trait representing operations on the Virtual Machine Control Structure (VMCS).
///
/// Implementors must ensure that the underlying VMX instructions are executed
/// correctly and that safety invariants are upheld internally. This allows
/// callers to use the API without unsafe blocks.
///
/// Implementation requirements:
/// - `clear` and `load` must properly handle the VMCLEAR and VMPTRLD instructions
/// - Read/write methods must ensure the VMCS is currently loaded
/// - `physical_address` must return a valid, 4KB-aligned physical address
pub trait VirtualMachineControlStructure: Sized {
    type P: Page;
    type M: Machine<P = Self::P>;

    /// Clears the VMCS.
    ///
    /// Implementors must ensure the VMCLEAR instruction is executed safely.
    fn clear(&self) -> Result<(), &'static str>;

    /// Loads the VMCS.
    ///
    /// Implementors must ensure the VMPTRLD instruction is executed safely.
    fn load(&self) -> Result<(), &'static str>;

    /// Reads a 16-bit field from the VMCS.
    ///
    /// Implementors must ensure the VMREAD instruction is executed safely
    /// and that the VMCS is loaded before reading.
    fn read16(&self, field: VmcsField16) -> VmcsReadResult<u16>;

    /// Reads a 32-bit field from the VMCS.
    ///
    /// Implementors must ensure the VMREAD instruction is executed safely
    /// and that the VMCS is loaded before reading.
    fn read32(&self, field: VmcsField32) -> VmcsReadResult<u32>;

    /// Reads a 64-bit field from the VMCS.
    ///
    /// Implementors must ensure the VMREAD instruction is executed safely
    /// and that the VMCS is loaded before reading.
    fn read64(&self, field: VmcsField64) -> VmcsReadResult<u64>;

    /// Reads a natural-width field from the VMCS.
    ///
    /// Natural-width fields are 64-bit on processors that support Intel 64
    /// architecture and 32-bit on processors that do not.
    ///
    /// Implementors must ensure the VMREAD instruction is executed safely
    /// and that the VMCS is loaded before reading.
    fn read_natural(&self, field: VmcsFieldNatural) -> VmcsReadResult<u64>;

    /// Writes a 16-bit field to the VMCS.
    ///
    /// Implementors must ensure the VMWRITE instruction is executed safely
    /// and that the VMCS is loaded before writing.
    fn write16(&self, field: VmcsField16, value: u16) -> VmcsWriteResult;

    /// Writes a 32-bit field to the VMCS.
    ///
    /// Implementors must ensure the VMWRITE instruction is executed safely
    /// and that the VMCS is loaded before writing.
    fn write32(&self, field: VmcsField32, value: u32) -> VmcsWriteResult;

    /// Writes a 64-bit field to the VMCS.
    ///
    /// Implementors must ensure the VMWRITE instruction is executed safely
    /// and that the VMCS is loaded before writing.
    fn write64(&self, field: VmcsField64, value: u64) -> VmcsWriteResult;

    /// Writes a natural-width field to the VMCS.
    ///
    /// Natural-width fields are 64-bit on processors that support Intel 64
    /// architecture and 32-bit on processors that do not.
    ///
    /// Implementors must ensure the VMWRITE instruction is executed safely
    /// and that the VMCS is loaded before writing.
    fn write_natural(&self, field: VmcsFieldNatural, value: u64) -> VmcsWriteResult;

    /// Returns a pointer to the VMCS region for direct memory access.
    ///
    /// This is used for efficient VMCS copying during VM fork operations.
    /// Per Intel SDM, the VMCS data format is implementation-specific; callers
    /// that copy the region must ensure both VMCS regions use the same VMCS
    /// revision and implementation format, and that VMCLEAR has made the
    /// in-memory data current.
    ///
    /// # Safety
    ///
    /// The returned pointer is only valid for direct memory access after
    /// VMCLEAR has been executed to flush VMCS data to memory.
    fn vmcs_region_ptr(&self) -> *mut u8;

    /// Writes the host-state area to the VMCS.
    ///
    /// This configures the processor state that will be loaded on every VM exit.
    /// The VMCS must already be loaded (via VMPTRLD) before calling this method.
    ///
    /// # Arguments
    ///
    /// * `host` - The host state to configure. All fields must contain valid values.
    ///
    /// # Errors
    ///
    /// Returns `VmcsWriteError` if any VMWRITE operation fails.
    ///
    /// See Intel SDM Vol 3C, Section 26.5.
    fn setup_host_state(&self, host: &HostState) -> VmcsWriteResult {
        // Control registers
        self.write_natural(VmcsFieldNatural::HostCr0, host.cr0)?;
        self.write_natural(VmcsFieldNatural::HostCr3, host.cr3)?;
        self.write_natural(VmcsFieldNatural::HostCr4, host.cr4)?;

        // Segment selectors
        self.write16(VmcsField16::HostCsSelector, host.cs_selector)?;
        self.write16(VmcsField16::HostSsSelector, host.ss_selector)?;
        self.write16(VmcsField16::HostDsSelector, host.ds_selector)?;
        self.write16(VmcsField16::HostEsSelector, host.es_selector)?;
        self.write16(VmcsField16::HostFsSelector, host.fs_selector)?;
        self.write16(VmcsField16::HostGsSelector, host.gs_selector)?;
        self.write16(VmcsField16::HostTrSelector, host.tr_selector)?;

        // Segment bases
        self.write_natural(VmcsFieldNatural::HostFsBase, host.fs_base)?;
        self.write_natural(VmcsFieldNatural::HostGsBase, host.gs_base)?;
        self.write_natural(VmcsFieldNatural::HostTrBase, host.tr_base)?;
        self.write_natural(VmcsFieldNatural::HostGdtrBase, host.gdtr_base)?;
        self.write_natural(VmcsFieldNatural::HostIdtrBase, host.idtr_base)?;

        // SYSENTER MSRs
        self.write32(VmcsField32::HostIa32SysenterCs, host.sysenter_cs)?;
        self.write_natural(VmcsFieldNatural::HostIa32SysenterEsp, host.sysenter_esp)?;
        self.write_natural(VmcsFieldNatural::HostIa32SysenterEip, host.sysenter_eip)?;

        // Other MSRs
        self.write64(VmcsField64::HostIa32Efer, host.efer)?;
        self.write64(VmcsField64::HostIa32Pat, host.pat)?;

        // Exit handler state
        self.write_natural(VmcsFieldNatural::HostRsp, host.rsp)?;
        self.write_natural(VmcsFieldNatural::HostRip, host.rip)?;

        // VMCS link pointer (must be ~0 for non-shadow VMCS)
        self.write64(VmcsField64::VmcsLinkPointer, !0u64)?;

        Ok(())
    }

    fn from_parts(page: Self::P, revision_id: u32) -> Self
    where
        Self: Sized;

    fn new(machine: &Self::M) -> Result<Self, VmcsAllocError>
    where
        Self: Sized,
    {
        let page = machine
            .kernel()
            .alloc_zeroed_page()
            .ok_or(VmcsAllocError::MemoryAllocationFailed)?;

        let revision_id = <Self::M as Machine>::V::basic_info().vmcs_revision_id & 0x7FFFFFFF;

        let ptr = page.virtual_address().as_u64() as *mut u32;
        // SAFETY: ptr is a valid pointer to the beginning of a freshly-allocated
        // zeroed 4KB page; writing 4 bytes at this address is within bounds.
        unsafe {
            core::ptr::write_volatile(ptr, revision_id);
        }

        log_info!(
            "Allocated VMCS page at physical address {:x}",
            page.physical_address().as_u64()
        );

        Ok(Self::from_parts(page, revision_id))
    }

    /// Setup VMCS control fields.
    ///
    /// Configures VM-execution controls, VM-exit controls, and VM-entry controls.
    /// The VMCS must already be loaded before calling this method.
    ///
    /// # Arguments
    ///
    /// * `msr_bitmap_addr` - Optional physical address of the 4KB MSR bitmap. If provided
    ///   and USE_MSR_BITMAPS is enabled in the CPU-based controls, this address will be
    ///   written to the MSR bitmap address field.
    ///
    /// Intel SDM Vol 3C, Chapter 26:
    /// - Pin-based VM-execution controls (Section 26.6.1)
    /// - Processor-based VM-execution controls (Section 26.6.2)
    /// - VM-exit controls (Section 26.7.1)
    /// - VM-entry controls (Section 26.8.1)
    /// - MSR-bitmap address (Section 26.6.9)
    fn setup_controls(&self, msr_bitmap_addr: Option<HostPhysAddr>) -> Result<(), VmcsSetupError> {
        let vcpu = <Self::M as Machine>::V::current_vcpu();
        let caps = vcpu.capabilities();

        // MSR bitmap (required if USE_MSR_BITMAPS control is set)
        // Intel SDM Vol 3C, Section 26.6.9
        if caps.cpu_based_exec_ctrl & cpu_based::USE_MSR_BITMAPS != 0 {
            if let Some(addr) = msr_bitmap_addr {
                self.write64(VmcsField64::MsrBitmapAddr, addr.as_u64())
                    .map_err(VmcsSetupError::Controls)?;
            }
        }

        // Pin-based VM-execution controls
        // Intel SDM Vol 3C, Section 26.6.1
        self.write32(
            VmcsField32::PinBasedVmExecControls,
            caps.pin_based_exec_ctrl,
        )
        .map_err(VmcsSetupError::Controls)?;

        // VMX-preemption timer value
        // Intel SDM Vol 3C, Section 26.4 and Section 27.5.1
        // The actual timeout depends on TSC frequency and VMX_MISC[4:0] divisor.
        // A value of 0x100000 gives roughly 10ms on typical hardware.
        //
        // NOTE: The preemption timer is NOT deterministic - it depends on wall-clock
        // time and host scheduling. It serves only as a heartbeat to ensure periodic
        // exits even if the guest is in a tight loop. May be removed in the future.
        if caps.pin_based_exec_ctrl & pin_based::PREEMPTION_TIMER != 0 {
            self.write32(VmcsField32::VmxPreemptionTimerValue, 0x100000)
                .map_err(VmcsSetupError::Controls)?;
        }

        // Primary processor-based VM-execution controls
        // Intel SDM Vol 3C, Section 26.6.2
        self.write32(
            VmcsField32::PrimaryProcBasedVmExecControls,
            caps.cpu_based_exec_ctrl,
        )
        .map_err(VmcsSetupError::Controls)?;

        log_info!(
            "CPU-based VM-exec controls = 0x{:08x}",
            caps.cpu_based_exec_ctrl
        );

        // Secondary processor-based VM-execution controls (if supported)
        // Only written if "activate secondary controls" bit is set in primary controls
        if caps.cpu_based_exec_ctrl & cpu_based::ACTIVATE_SECONDARY_CONTROLS != 0 {
            self.write32(
                VmcsField32::SecondaryProcBasedVmExecControls,
                caps.cpu_based_exec_ctrl2,
            )
            .map_err(VmcsSetupError::Controls)?;

            // VPID (Virtual Processor Identifier)
            // Intel SDM Vol 3C, Section 28.2.1.1: If "enable VPID" is 1, VPID must not be 0000H.
            // VPID 0 is reserved for VMX root operation.
            // Each VM gets a unique VPID for TLB isolation.
            if caps.cpu_based_exec_ctrl2 & secondary_exec::ENABLE_VPID != 0 {
                let vpid = allocate_vpid();
                self.write16(VmcsField16::VirtualProcessorId, vpid)
                    .map_err(VmcsSetupError::Controls)?;

                // Flush all TLB entries for this VPID to ensure no stale entries
                // from previous VMs or the host affect this VM.
                // This is critical for correct operation of text_poke and other
                // code that relies on TLB coherency after CR3 switches.
                if let Err(e) = <Self::M as Machine>::V::invvpid_single_context(vpid) {
                    log_err!("INVVPID failed for VPID {}: {:?}", vpid, e);
                    // Don't fail setup - INVVPID might not be supported in nested VM
                }

                log_info!("Allocated VPID={}", vpid);
            }
        }

        // VM-exit controls
        // Intel SDM Vol 3C, Section 26.7.1
        self.write32(VmcsField32::PrimaryVmExitControls, caps.vmexit_ctrl)
            .map_err(VmcsSetupError::Controls)?;

        // VM-entry controls
        // Intel SDM Vol 3C, Section 26.8.1
        self.write32(VmcsField32::VmEntryControls, caps.vmentry_ctrl)
            .map_err(VmcsSetupError::Controls)?;

        // Exception bitmap: only intercept #MC (Machine Check, vector 18).
        // Following bhyve's minimal approach - let guest handle its own exceptions.
        // KVM intercepts more but has sophisticated exception injection.
        // Intel SDM Vol 3C, Section 26.6.3
        self.write32(VmcsField32::ExceptionBitmap, 1 << 18) // #MC only
            .map_err(VmcsSetupError::Controls)?;

        // CR0/CR4 guest/host masks: mask the VMX-constrained bits.
        // When mask bit is 1, guest writes cause VM exit, reads return shadow.
        // This allows us to emulate CR writes while enforcing VMX constraints.
        // Intel SDM Vol 3C, Section 26.6.6
        //
        // Following bhyve's approach:
        // - ones_mask = fixed0 & fixed1 (bits that must be 1)
        // - zeros_mask = ~fixed0 & ~fixed1 (bits that must be 0)
        // - mask = ones_mask | zeros_mask (all constrained bits)
        let cr0_ones_mask = caps.cr0_fixed0 & caps.cr0_fixed1;
        let cr0_zeros_mask = !caps.cr0_fixed0 & !caps.cr0_fixed1;
        let cr0_mask = cr0_ones_mask | cr0_zeros_mask;

        let cr4_ones_mask = caps.cr4_fixed0 & caps.cr4_fixed1;
        let cr4_zeros_mask = !caps.cr4_fixed0 & !caps.cr4_fixed1;
        let cr4_mask = cr4_ones_mask | cr4_zeros_mask;

        self.write_natural(VmcsFieldNatural::Cr0GuestHostMask, cr0_mask)
            .map_err(VmcsSetupError::Controls)?;
        self.write_natural(VmcsFieldNatural::Cr4GuestHostMask, cr4_mask)
            .map_err(VmcsSetupError::Controls)?;

        // CR0/CR4 read shadows: initial values the guest sees when reading.
        // Set to 0 initially; will be updated when guest writes CR0/CR4.
        self.write_natural(VmcsFieldNatural::Cr0ReadShadow, 0)
            .map_err(VmcsSetupError::Controls)?;
        self.write_natural(VmcsFieldNatural::Cr4ReadShadow, 0)
            .map_err(VmcsSetupError::Controls)?;

        // Page fault error code matching (disabled)
        // Intel SDM Vol 3C, Section 26.6.3
        self.write32(VmcsField32::PageFaultErrorCodeMask, 0)
            .map_err(VmcsSetupError::Controls)?;
        self.write32(VmcsField32::PageFaultErrorCodeMatch, 0)
            .map_err(VmcsSetupError::Controls)?;

        // CR3 target count (no CR3 target values)
        // Intel SDM Vol 3C, Section 26.6.7
        self.write32(VmcsField32::Cr3TargetCount, 0)
            .map_err(VmcsSetupError::Controls)?;

        // VM-entry interrupt injection (disabled - bit 31 is valid bit)
        // Intel SDM Vol 3C, Section 26.8.3
        self.write32(VmcsField32::VmEntryInterruptionInfo, 0)
            .map_err(VmcsSetupError::Controls)?;

        Ok(())
    }

    fn setup(
        &self,
        ept_pointer: u64,
        msr_bitmap_addr: Option<HostPhysAddr>,
        host: &HostState,
    ) -> Result<(), VmcsSetupError> {
        self.clear().map_err(VmcsSetupError::Clear)?;

        {
            let _guard = VmcsGuard::new(self).map_err(VmcsSetupError::Guard)?;

            self.setup_host_state(host)
                .map_err(VmcsSetupError::HostState)?;

            self.setup_controls(msr_bitmap_addr)?;

            // Configure EPTP (Extended Page Table Pointer)
            self.write64(VmcsField64::EptPointer, ept_pointer)
                .map_err(VmcsSetupError::EptPointer)?;
            log_info!("Configured EPTP=0x{:x}", ept_pointer);
        }

        log_info!("VMCS setup complete (EPTP=0x{:x})", ept_pointer);
        Ok(())
    }
}

/// RAII guard that loads a VMCS on creation and clears it on drop.
pub struct VmcsGuard<'a, T: VirtualMachineControlStructure> {
    vmcs: &'a T,
}

impl<'a, T: VirtualMachineControlStructure> VmcsGuard<'a, T> {
    /// Loads the VMCS and returns a guard that will unload it on drop.
    ///
    /// The caller should ensure that no other VMCS is loaded while this guard is active.
    pub fn new(vmcs: &'a T) -> Result<Self, &'static str> {
        vmcs.load()?;
        Ok(Self { vmcs })
    }
}

impl<'a, T: VirtualMachineControlStructure> Drop for VmcsGuard<'a, T> {
    fn drop(&mut self) {
        let _ = self.vmcs.clear();
    }
}
