#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

#[cfg(not(feature = "cargo"))]
use super::super::registers::Cr0;
#[cfg(feature = "cargo")]
use crate::registers::Cr0;

use super::{
    cpu_based, pin_based, secondary_exec, vm_entry, vm_exit, HostPhysAddr, InveptError,
    InvvpidError, Kernel, Machine, Page, VmxBasic, VmxCapabilities,
    VmxConfigureFeatureControlError, VmxCpuInitError, VmxInitError, VmxoffError, VmxonAllocError,
    VmxonError,
};

/// Trait representing a VMXON region.
pub trait VmxOnRegion {
    type M: Machine;

    fn from_page(page: <<Self::M as Machine>::K as Kernel>::P) -> Self
    where
        Self: Sized;

    /// Allocate and initialize a new VMXON region.
    fn new(machine: &Self::M) -> Result<Self, VmxonAllocError>
    where
        Self: Sized,
    {
        let kernel = machine.kernel();
        let page = kernel.alloc_zeroed_page().ok_or_else(|| {
            log_err!("afailed to allocate VMXON region page\n");
            VmxonAllocError::MemoryAllocationFailed
        })?;

        let phys_addr = page.physical_address();
        log_debug!("VMXON region allocated at phys={:#x}\n", phys_addr.as_u64());

        let revision_id = <Self::M as Machine>::V::basic_info().vmcs_revision_id & 0x7fff_ffff;
        unsafe {
            let region_ptr = page.virtual_address().as_u64() as *mut u32;
            *region_ptr = revision_id;
        }

        <Self::M as Machine>::V::vmxon(phys_addr).map_err(|e| {
            log_err!("VMXON instruction failed: {:?}\n", e);
            VmxonAllocError::VmxonFailed(e)
        })?;
        log_debug!("VMXON successful\n");

        Ok(Self::from_page(page))
    }
}

/// Trait representing global VMX operations.
pub trait Vmx {
    type M: Machine;

    /// Check if VMX is supported on this machine.
    fn is_supported() -> bool;

    /// Initialize VMX operation on all processors.
    fn initialize(machine: &Self::M) -> Result<(), VmxInitError>
    where
        Self: Sized,
    {
        log_info!("starting initialization\n");

        // Check if VMX is supported
        if !Self::is_supported() {
            log_err!("not supported on this CPU\n");
            return Err(VmxInitError::Unsupported);
        }
        log_debug!("CPU support verified\n");

        // Load and store basic VMX information
        let basic_info = vmx_load_basic_info(machine.msr_access()).map_err(|e| {
            log_err!("failed to read basic info MSR: {:?}\n", e);
            VmxInitError::FailedToReadBasicInfo(e)
        })?;
        log_info!("{:?}\n", basic_info);
        Self::set_basic_info(basic_info);

        // Initialize VMX on all processors
        log_info!("initializing on all CPUs\n");
        let kernel = machine.kernel();
        kernel.call_on_all_cpus_with_data(
            machine,
            |machine: &Self::M| -> Result<(), VmxInitError> {
                let core_id = machine.kernel().current_cpu_id();
                log_debug!("initializing CPU {}\n", core_id);
                Self::current_vcpu().init(machine).map_err(|e| {
                    log_err!("failed to initialize CPU {}: {:?}\n", core_id, e);
                    VmxInitError::FailedToEnableCPU {
                        core: core_id,
                        error: e,
                    }
                })?;
                log_debug!("CPU {} initialized successfully\n", core_id);

                Ok(())
            },
        )?;

        log_info!("initialization complete\n");
        Ok(())
    }

    /// Get the Vcpu for the current processor.
    ///
    /// Note: In production these are allocated as global per-cpu variables, hence the 'static
    /// lifetime.
    fn current_vcpu() -> &'static <Self::M as Machine>::Vcpu;

    /// Get basic VMX information.
    fn basic_info() -> &'static VmxBasic;
    fn set_basic_info(basic: VmxBasic);

    /// Execute VMXON instruction.
    ///
    /// Puts the logical processor in VMX operation with no current VMCS.
    ///
    /// # Arguments
    ///
    /// * `phys_addr` - Physical address of the VMXON region (must be 4KB aligned)
    ///
    /// # Errors
    ///
    /// Returns `VmxonError::InvalidPointer` if the VMXON pointer is invalid
    /// (not 4KB aligned, sets bits beyond physical-address width, or revision ID mismatch).
    ///
    /// Returns `VmxonError::AlreadyInVmxOperation` if already in VMX root operation.
    ///
    /// See Intel SDM Vol 3C, "VMXON—Enter VMX Operation".
    fn vmxon(phys_addr: HostPhysAddr) -> Result<(), VmxonError>;

    /// Execute VMXOFF instruction.
    ///
    /// Takes the logical processor out of VMX operation.
    ///
    /// # Errors
    ///
    /// Returns `VmxoffError::DualMonitorTreatmentActive` if dual-monitor treatment
    /// of SMIs and SMM is active.
    ///
    /// See Intel SDM Vol 3C, "VMXOFF—Leave VMX Operation".
    fn vmxoff() -> Result<(), VmxoffError>;

    /// Execute INVEPT instruction with single-context invalidation (type 1).
    ///
    /// Invalidates all EPT-derived cached translations for the specified EPTP.
    /// Only entries matching the given EPT pointer are invalidated, leaving
    /// other EPT contexts (other VMs) unaffected.
    ///
    /// This should be called when creating a forked VM to ensure the new EPT
    /// doesn't inherit stale TLB entries from the parent.
    ///
    /// # Arguments
    ///
    /// * `eptp` - The EPT pointer (EPTP) whose translations should be invalidated
    ///
    /// # Errors
    ///
    /// Returns `InveptError::InvalidOperand` if not in VMX operation.
    /// Returns `InveptError::NotSupported` if single-context INVEPT is not supported.
    ///
    /// See Intel SDM Vol 3C, "INVEPT—Invalidate Translations Derived from EPT".
    fn invept_single_context(eptp: u64) -> Result<(), InveptError>;

    /// Execute INVVPID instruction with single-context invalidation (type 1).
    ///
    /// Invalidates all linear-address translations and combined translations
    /// for the specified VPID. This flushes all TLB entries tagged with this VPID.
    ///
    /// This should be called when creating a new VM to ensure no stale TLB entries
    /// from previous VMs or the host affect the new VM.
    ///
    /// # Arguments
    ///
    /// * `vpid` - The Virtual Processor Identifier whose translations should be invalidated
    ///
    /// # Errors
    ///
    /// Returns `InvvpidError::InvalidOperand` if not in VMX operation or VPID is 0.
    /// Returns `InvvpidError::NotSupported` if single-context INVVPID is not supported.
    ///
    /// See Intel SDM Vol 3C, "INVVPID—Invalidate Translations Based on VPID".
    fn invvpid_single_context(vpid: u16) -> Result<(), InvvpidError>;

    /// Execute INVVPID instruction with all-context invalidation (type 2).
    ///
    /// Invalidates all linear-address translations and combined translations
    /// for all VPIDs except VPID 0. This is a global TLB flush for all VMs.
    ///
    /// # Errors
    ///
    /// Returns `InvvpidError::InvalidOperand` if not in VMX operation.
    /// Returns `InvvpidError::NotSupported` if all-context INVVPID is not supported.
    ///
    /// See Intel SDM Vol 3C, "INVVPID—Invalidate Translations Based on VPID".
    fn invvpid_all_context() -> Result<(), InvvpidError>;

    /// Fix CR0 value to meet VMX requirements.
    ///
    /// Per Intel SDM Vol 3D, Appendix A.7:
    /// - If bit X is 1 in IA32_VMX_CR0_FIXED0, that bit must be 1 in CR0
    /// - If bit X is 0 in IA32_VMX_CR0_FIXED1, that bit must be 0 in CR0
    ///
    /// Formula: result = (input | FIXED0) & FIXED1
    fn fix_cr0(cr0: &Cr0, cap: &VmxCapabilities) -> Cr0 {
        // OR with FIXED0 to set bits that must be 1
        // AND with FIXED1 to clear bits that must be 0
        Cr0::new((cr0.bits() | cap.cr0_fixed0) & cap.cr0_fixed1)
    }

    /// Fix CR4 value to meet VMX requirements.
    ///
    /// Per Intel SDM Vol 3D, Appendix A.8:
    /// - If bit X is 1 in IA32_VMX_CR4_FIXED0, that bit must be 1 in CR4
    /// - If bit X is 0 in IA32_VMX_CR4_FIXED1, that bit must be 0 in CR4
    ///
    /// Formula: result = (input | FIXED0) & FIXED1
    fn fix_cr4(cr4: &Cr4, cap: &VmxCapabilities) -> Cr4 {
        // OR with FIXED0 to set bits that must be 1
        // AND with FIXED1 to clear bits that must be 0
        Cr4::new((cr4.bits() | cap.cr4_fixed0) & cap.cr4_fixed1)
    }
}

pub fn vmx_load_basic_info<M: MsrAccess>(msr: &M) -> Result<VmxBasic, MsrError> {
    let vmx_basic_msr = msr.read_msr(msr::IA32_VMX_BASIC)?;

    Ok(VmxBasic {
        vmcs_revision_id: (vmx_basic_msr & 0x7fff_ffff) as u32,
        vmcs_size: ((vmx_basic_msr >> 32) & 0x1fff) as u16,
        mem_type_wb: ((vmx_basic_msr >> 50) & 1) != 0,
        io_exit_info: ((vmx_basic_msr >> 54) & 1) != 0,
        vmx_flex_controls: ((vmx_basic_msr >> 55) & 1) != 0,
    })
}

/// Trait representing a VMX-capable virtual CPU core.
pub trait VmxCpu {
    type M: Machine;
    type R: VmxOnRegion<M = Self::M>;

    /// Get VMX capabilities.
    fn capabilities(&self) -> &VmxCapabilities;
    /// Check if VMX operation is enabled.
    fn is_vmxon(&self) -> bool;

    fn set_vmxon(&self, enabled: bool);
    fn set_capabilities(&self, caps: VmxCapabilities);
    fn set_vmxon_region(&self, region: Self::R);

    /// Create a new instance of the VmxCpu.
    fn init(&self, machine: &Self::M) -> Result<(), VmxCpuInitError> {
        assert!(!self.is_vmxon(), "VmxCpu is already initialized");

        // Step 1: Configure IA32_FEATURE_CONTROL MSR
        log_debug!("configuring IA32_FEATURE_CONTROL MSR\n");
        Self::configure_feature_control(machine).map_err(|e| {
            log_err!("failed to configure feature control: {:?}\n", e);
            VmxCpuInitError::FeatureControlConfigFailed(e)
        })?;

        // Step 2: Enable VMX in CR4 (bit 13)
        // Uses set_vmxe() which updates the kernel's CR4 shadow (cpu_tlbstate.cr4)
        // in addition to the actual CR4. A raw MOV to CR4 would desync the shadow,
        // causing #GP when the kernel writes CR4 without VMXE during context switches.
        log_debug!("enabling VMXE in CR4\n");
        let cr = machine.cr_access();
        cr.set_vmxe().map_err(|e| {
            log_err!("failed to set CR4.VMXE: {:?}\n", e);
            VmxCpuInitError::FailedToEnableVMX(e)
        })?;

        // Step 3: Allocate VMXON region and execute VMXON instruction
        log_debug!("allocating VMXON region\n");
        let vmxon_region = VmxOnRegion::new(machine).map_err(|e| {
            log_err!("failed to allocate VMXON region: {:?}\n", e);
            VmxCpuInitError::VmxonAllocFailed(e)
        })?;

        // Step 4: Read VMX capabilities
        log_debug!("reading capabilities\n");
        let caps = Self::read_capabilities(machine);
        log_info!("{:?}\n", caps);

        self.set_vmxon(true);
        self.set_vmxon_region(vmxon_region);
        self.set_capabilities(caps);

        Ok(())
    }

    /// Adjust control field using allowed-0 and allowed-1 bits
    ///
    /// Intel SDM Vol 3C, Appendix A.3:
    /// - Bits set to 1 in allowed-0 MSR *must* be 1
    /// - Bits set to 0 in allowed-1 MSR *must* be 0
    fn adjust_controls(msr_value: u64, requested: u32) -> u32 {
        let allowed0 = msr_value as u32; // Bits that must be 1
        let allowed1 = (msr_value >> 32) as u32; // Bits that can be 1

        // Set all bits that must be 1
        let mut adjusted = requested | allowed0;

        // Clear all bits that must be 0
        adjusted &= allowed1;

        adjusted
    }

    /// Read VMX capabilities from MSRs.
    ///
    /// This function reads the VMX capability MSRs and returns the adjusted
    /// control values that can be used when configuring VMCS control fields.
    ///
    /// The requested controls are:
    /// - Pin-based: NMI exiting
    /// - CPU-based: HLT exiting, MSR bitmaps, secondary controls, unconditional I/O,
    ///              CR3 load/store exiting, CR8 load/store exiting
    /// - Secondary: EPT, unrestricted guest
    /// - VM-exit: 64-bit host, save/load EFER
    /// - VM-entry: IA-32e mode, load EFER
    ///
    /// # Arguments
    ///
    /// * `msr` - An implementation of the `MsrAccess` trait for reading MSRs
    ///
    /// # Returns
    ///
    /// A `VmxCapabilities` struct containing the adjusted control values.
    /// If any MSR read fails, the corresponding field will use default values.
    ///
    /// See Intel SDM Vol 3C, Appendix A.
    fn read_capabilities<M: Machine>(machine: &M) -> VmxCapabilities {
        let msr = machine.msr_access();
        let mut cap = VmxCapabilities::default();

        // Read pin-based controls
        // - EXT_INTR_EXITING: Exit on external interrupts so host can service them
        // - NMI_EXITING: Exit on NMIs
        // - PREEMPTION_TIMER: Guarantee periodic exits even if guest is in tight loop
        let requested =
            pin_based::EXT_INTR_EXITING | pin_based::NMI_EXITING | pin_based::PREEMPTION_TIMER;
        if let Ok(msr_value) = msr.read_msr(msr::IA32_VMX_PINBASED_CTLS) {
            cap.pin_based_exec_ctrl = Self::adjust_controls(msr_value, requested);
        }

        // Read primary processor-based controls
        // CR3_LOAD/STORE_EXITING are enabled for determinism. The CR3 handler calls
        // INVVPID to maintain TLB coherency when the guest changes page tables.
        let requested = cpu_based::HLT_EXITING
            | cpu_based::MWAIT_EXITING   // Exit on MWAIT (idle instruction)
            | cpu_based::MONITOR_EXITING // Exit on MONITOR (so address-range monitoring is never armed)
            | cpu_based::RDPMC_EXITING   // Exit on RDPMC (performance counter reads)
            | cpu_based::RDTSC_EXITING   // Exit on RDTSC/RDTSCP (for deterministic time)
            | cpu_based::USE_MSR_BITMAPS
            | cpu_based::ACTIVATE_SECONDARY_CONTROLS
            | cpu_based::UNCOND_IO_EXITING
            | cpu_based::CR3_LOAD_EXITING
            | cpu_based::CR3_STORE_EXITING
            | cpu_based::CR8_LOAD_EXITING
            | cpu_based::CR8_STORE_EXITING;
        if let Ok(msr_value) = msr.read_msr(msr::IA32_VMX_PROCBASED_CTLS) {
            cap.cpu_based_exec_ctrl = Self::adjust_controls(msr_value, requested);
        }

        // Read secondary processor-based controls (if supported)
        if cap.cpu_based_exec_ctrl & cpu_based::ACTIVATE_SECONDARY_CONTROLS != 0 {
            let requested = secondary_exec::ENABLE_EPT
                | secondary_exec::ENABLE_VPID
                | secondary_exec::UNRESTRICTED_GUEST
                | secondary_exec::ENABLE_RDTSCP
                | secondary_exec::ENABLE_INVPCID // Allow native INVPCID execution
                | secondary_exec::RDRAND_EXITING  // Intercept RDRAND for emulation
                | secondary_exec::RDSEED_EXITING; // Intercept RDSEED for emulation

            if let Ok(msr_value) = msr.read_msr(msr::IA32_VMX_PROCBASED_CTLS2) {
                cap.cpu_based_exec_ctrl2 = Self::adjust_controls(msr_value, requested);
            }

            cap.has_ept = cap.cpu_based_exec_ctrl2 & secondary_exec::ENABLE_EPT != 0;
            cap.has_vpid = cap.cpu_based_exec_ctrl2 & secondary_exec::ENABLE_VPID != 0;
        } else {
            cap.cpu_based_exec_ctrl2 = 0;
            cap.has_ept = false;
            cap.has_vpid = false;
        }

        // Read VM-exit controls
        // Note: We do NOT use ACK_INTR_ON_EXIT. Instead, on external interrupt exit,
        // we briefly enable interrupts to let the CPU deliver the interrupt naturally
        // through the IDT (similar to AMD SVM approach in KVM).
        let requested =
            vm_exit::HOST_ADDR_SPACE_SIZE | vm_exit::SAVE_IA32_EFER | vm_exit::LOAD_IA32_EFER;
        if let Ok(msr_value) = msr.read_msr(msr::IA32_VMX_EXIT_CTLS) {
            cap.vmexit_ctrl = Self::adjust_controls(msr_value, requested);
        }

        // Read VM-entry controls
        let requested = vm_entry::IA32E_MODE | vm_entry::LOAD_IA32_EFER;
        if let Ok(msr_value) = msr.read_msr(msr::IA32_VMX_ENTRY_CTLS) {
            cap.vmentry_ctrl = Self::adjust_controls(msr_value, requested);
        }

        // Read CR0 and CR4 fixed bits
        if let Ok(value) = msr.read_msr(msr::IA32_VMX_CR0_FIXED0) {
            cap.cr0_fixed0 = value;
        }
        if let Ok(value) = msr.read_msr(msr::IA32_VMX_CR0_FIXED1) {
            cap.cr0_fixed1 = value;
        }
        if let Ok(value) = msr.read_msr(msr::IA32_VMX_CR4_FIXED0) {
            cap.cr4_fixed0 = value;
        }
        if let Ok(value) = msr.read_msr(msr::IA32_VMX_CR4_FIXED1) {
            cap.cr4_fixed1 = value;
        }

        cap
    }

    /// Configure IA32_FEATURE_CONTROL MSR
    ///
    /// This MSR controls VMX enablement. We need:
    /// - Bit 0 (lock bit) set to 1
    /// - Bit 2 (enable VMX outside SMX) set to 1
    fn configure_feature_control<M: Machine>(
        machine: &M,
    ) -> Result<(), VmxConfigureFeatureControlError> {
        let mut feature_control = machine
            .msr_access()
            .read_msr(msr::IA32_FEATURE_CONTROL)
            .map_err(|e| VmxConfigureFeatureControlError::MsrReadFailed(e))?;

        const FEAT_CTL_LOCKED: u64 = 1 << 0;
        const FEAT_CTL_VMX_ENABLED_OUTSIDE_SMX: u64 = 1 << 2;

        // Check if already locked and configured correctly - this is fine, VMX is enabled
        if (feature_control & FEAT_CTL_LOCKED) != 0
            && (feature_control & FEAT_CTL_VMX_ENABLED_OUTSIDE_SMX) != 0
        {
            return Ok(());
        }

        // If locked but not configured, we can't change it
        if (feature_control & FEAT_CTL_LOCKED) != 0 {
            return Err(VmxConfigureFeatureControlError::Locked);
        }

        // Enable VMX outside SMX and lock
        feature_control |= FEAT_CTL_VMX_ENABLED_OUTSIDE_SMX;
        feature_control |= FEAT_CTL_LOCKED;

        machine
            .msr_access()
            .write_msr(msr::IA32_FEATURE_CONTROL, feature_control)
            .map_err(|e| VmxConfigureFeatureControlError::MsrWriteFailed(e))?;

        Ok(())
    }

    fn deinitialize<M: Machine>(&self, machine: &M) -> Result<(), VmxoffError> {
        if self.is_vmxon() {
            log_debug!("executing VMXOFF\n");
            // Execute VMXOFF instruction
            M::V::vmxoff().map_err(|e| {
                log_err!("VMXOFF failed: {:?}\n", e);
                e
            })?;

            // Disable VMX in CR4 (updates kernel's CR4 shadow too)
            log_debug!("disabling VMXE in CR4\n");
            let cr = machine.cr_access();
            let _ = cr.clear_vmxe();

            self.set_vmxon(false);
            log_debug!("deinitialization complete\n");
        }

        Ok(())
    }
}
