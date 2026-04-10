// SPDX-License-Identifier: GPL-2.0

//! VMCS (Virtual Machine Control Structure) implementation.

use core::arch::asm;

use super::machine::LinuxMachine;
use super::memory::HostPhysAddr;
use super::page::KernelPage;
use super::vmx::traits::Page as PageTrait;
use super::vmx::{
    VirtualMachineControlStructure, VmcsField16, VmcsField32, VmcsField64, VmcsFieldNatural,
    VmcsReadError, VmcsReadResult, VmcsWriteError, VmcsWriteResult,
};

/// Real VMCS implementation backed by a kernel page.
pub(crate) struct RealVmcs {
    /// The underlying kernel page for the VMCS region.
    /// When dropped, the page is freed.
    page: Option<KernelPage>,
}

impl RealVmcs {
    /// Get the physical address, panicking if the VMCS is uninitialized.
    fn phys_addr(&self) -> HostPhysAddr {
        self.page
            .as_ref()
            .expect("VMCS is uninitialized")
            .physical_address()
    }

    fn vmread(&self, field: u64) -> VmcsReadResult<u64> {
        let value: u64;
        let rflags: u64;
        // SAFETY: VMREAD is valid when a VMCS is loaded; caller ensures the VMCS is active.
        unsafe {
            asm!(
                "vmread {0}, {1}",
                "pushfq",
                "pop {2}",
                out(reg) value,
                in(reg) field,
                out(reg) rflags,
                options(nostack)
            );
        }

        let cf = rflags & 1;
        let zf = (rflags >> 6) & 1;

        if cf == 1 {
            Err(VmcsReadError::VmcsNotLoaded)
        } else if zf == 1 {
            Err(VmcsReadError::InvalidField)
        } else {
            Ok(value)
        }
    }

    fn vmwrite(&self, field: u64, value: u64) -> VmcsWriteResult {
        let rflags: u64;
        // SAFETY: VMWRITE is valid when a VMCS is loaded; caller ensures the VMCS is active.
        unsafe {
            asm!(
                "vmwrite {0}, {1}",
                "pushfq",
                "pop {2}",
                in(reg) field,
                in(reg) value,
                out(reg) rflags,
                options(nostack)
            );
        }

        let cf = rflags & 1;
        let zf = (rflags >> 6) & 1;

        if cf == 1 {
            Err(VmcsWriteError::VmcsNotLoaded)
        } else if zf == 1 {
            Err(VmcsWriteError::InvalidField)
        } else {
            Ok(())
        }
    }
}

impl VirtualMachineControlStructure for RealVmcs {
    type P = KernelPage;
    type M = LinuxMachine;

    fn clear(&self) -> Result<(), &'static str> {
        let addr = self.phys_addr().as_u64();

        let rflags: u64;
        // SAFETY: VMCLEAR with a valid physical address clears the VMCS launch state.
        unsafe {
            asm!(
                "vmclear [{0}]",
                "pushfq",
                "pop {1}",
                in(reg) &addr,
                out(reg) rflags,
                options(nostack)
            );
        }

        let cf = rflags & 1;
        let zf = (rflags >> 6) & 1;

        if cf == 1 || zf == 1 {
            // Try to read VM instruction error if ZF is set
            let vm_err: u64 = if zf == 1 {
                let err: u64;
                // SAFETY: VMREAD of the error field is valid after a failed VM instruction.
                unsafe {
                    asm!(
                        "vmread {0}, {1}",
                        out(reg) err,
                        in(reg) 0x4400u64, // VM_INSTRUCTION_ERROR field
                        options(nostack)
                    );
                }
                err
            } else {
                0
            };
            log_err!(
                "VMCLEAR failed: addr={:#x} CF={} ZF={} vm_err={}\n",
                addr,
                cf,
                zf,
                vm_err
            );
            Err("VMCLEAR failed")
        } else {
            Ok(())
        }
    }

    fn load(&self) -> Result<(), &'static str> {
        let addr = self.phys_addr().as_u64();

        let rflags: u64;
        // SAFETY: VMPTRLD with a valid physical address loads the VMCS as active.
        unsafe {
            asm!(
                "vmptrld [{0}]",
                "pushfq",
                "pop {1}",
                in(reg) &addr,
                out(reg) rflags,
                options(nostack)
            );
        }

        let cf = rflags & 1;
        let zf = (rflags >> 6) & 1;

        if cf == 1 || zf == 1 {
            // Try to read VM instruction error if ZF is set
            let vm_err: u64 = if zf == 1 {
                let err: u64;
                // SAFETY: VMREAD of the error field is valid after a failed VM instruction.
                unsafe {
                    asm!(
                        "vmread {0}, {1}",
                        out(reg) err,
                        in(reg) 0x4400u64, // VM_INSTRUCTION_ERROR field
                        options(nostack)
                    );
                }
                err
            } else {
                0
            };
            log_err!(
                "VMPTRLD failed: addr={:#x} CF={} ZF={} vm_err={}\n",
                addr,
                cf,
                zf,
                vm_err
            );
            Err("VMPTRLD failed")
        } else {
            Ok(())
        }
    }

    fn read16(&self, field: VmcsField16) -> VmcsReadResult<u16> {
        self.vmread(field as u64).map(|v| v as u16)
    }

    fn read32(&self, field: VmcsField32) -> VmcsReadResult<u32> {
        self.vmread(field as u64).map(|v| v as u32)
    }

    fn read64(&self, field: VmcsField64) -> VmcsReadResult<u64> {
        self.vmread(field as u64)
    }

    fn read_natural(&self, field: VmcsFieldNatural) -> VmcsReadResult<u64> {
        self.vmread(field as u64)
    }

    fn write16(&self, field: VmcsField16, value: u16) -> VmcsWriteResult {
        self.vmwrite(field as u64, u64::from(value))
    }

    fn write32(&self, field: VmcsField32, value: u32) -> VmcsWriteResult {
        self.vmwrite(field as u64, u64::from(value))
    }

    fn write64(&self, field: VmcsField64, value: u64) -> VmcsWriteResult {
        self.vmwrite(field as u64, value)
    }

    fn write_natural(&self, field: VmcsFieldNatural, value: u64) -> VmcsWriteResult {
        self.vmwrite(field as u64, value)
    }

    fn vmcs_region_ptr(&self) -> *mut u8 {
        self.page
            .as_ref()
            .expect("VMCS is uninitialized")
            .virtual_address()
            .as_u64() as *mut u8
    }

    fn from_parts(page: KernelPage, _revision_id: u32) -> Self {
        Self { page: Some(page) }
    }
}
