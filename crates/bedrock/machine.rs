// SPDX-License-Identifier: GPL-2.0

//! Linux kernel machine implementation including MSR, CR, and descriptor table access.

use core::arch::asm;
use core::cell::UnsafeCell;

use kernel::bindings;

use super::c_helpers;
use super::memory::HostPhysAddr;
use super::page::{alloc_zeroed_page, KernelGuestMemory, KernelPage};
use super::vmx::registers::{
    Cr0, Cr3, Cr4, CrAccess, CrError, DescriptorTableAccess, Gdtr, Idtr, MsrAccess, MsrError,
    SegmentSelector,
};
use super::vmx::traits::{Kernel, Machine};
use super::vmxon::RealVmx;

/// Kernel operations implementation.
pub(crate) struct LinuxKernel;

impl Kernel for LinuxKernel {
    type P = KernelPage;
    type G = KernelGuestMemory;

    fn alloc_zeroed_page(&self) -> Option<Self::P> {
        alloc_zeroed_page()
    }

    fn alloc_guest_memory(&self, size: usize) -> Option<Self::G> {
        KernelGuestMemory::new(size)
    }

    fn phys_to_virt(&self, phys: HostPhysAddr) -> *mut u8 {
        // SAFETY: bedrock_phys_to_virt wraps __va() which is valid for direct-mapped physical memory.
        unsafe { c_helpers::bedrock_phys_to_virt(phys.as_u64()).cast::<u8>() }
    }

    fn call_on_all_cpus_with_data<F, T, E>(&self, data: &T, func: F) -> Result<(), E>
    where
        F: Fn(&T) -> Result<(), E> + Sync + Send,
        T: Sync,
        E: Send,
    {
        // Store the closure and data in a struct we can pass through the C callback.
        struct CallbackData<'a, F, T, E> {
            func: &'a F,
            data: &'a T,
            error: UnsafeCell<Option<E>>,
        }

        // SAFETY: This is only accessed from one CPU at a time via for_each_cpu.
        unsafe impl<F, T, E> Sync for CallbackData<'_, F, T, E> {}

        // The C callback that bedrock_for_each_cpu will invoke on each CPU.
        // The info parameter points to BedrockCpuCallInfo, which contains our CallbackData.
        extern "C" fn trampoline<F, T, E>(info: *mut core::ffi::c_void)
        where
            F: Fn(&T) -> Result<(), E>,
        {
            // SAFETY: info points to BedrockCpuCallInfo from the C helper.
            let call_info = unsafe { &mut *(info.cast::<c_helpers::BedrockCpuCallInfo>()) };
            // SAFETY: call_info.info points to our CallbackData struct.
            let cb = unsafe { &*(call_info.info as *const CallbackData<'_, F, T, E>) };
            if let Err(e) = (cb.func)(cb.data) {
                // Store the error and signal to C code that we failed.
                // SAFETY: Only one CPU at a time accesses this.
                unsafe {
                    *cb.error.get() = Some(e);
                }
                // Set error code to signal failure to the C code.
                // This will cause bedrock_for_each_cpu to stop iterating.
                call_info.error = -1;
            }
        }

        let cb_data = CallbackData {
            func: &func,
            data,
            error: UnsafeCell::new(None),
        };

        let mut failed_cpu: i32 = -1;

        // SAFETY: bedrock_for_each_cpu is our C helper that uses for_each_online_cpu
        // with smp_call_function_single to call the callback on each CPU sequentially.
        // It stops on the first error and returns the failed CPU.
        let ret = unsafe {
            c_helpers::bedrock_for_each_cpu(
                Some(trampoline::<F, T, E>),
                core::ptr::from_ref(&cb_data)
                    .cast_mut()
                    .cast::<core::ffi::c_void>(),
                &mut failed_cpu,
            )
        };

        // Check if any CPU encountered an error.
        if ret != 0 {
            // SAFETY: bedrock_for_each_cpu has completed, so no more writes to error.
            if let Some(e) = unsafe { (*cb_data.error.get()).take() } {
                return Err(e);
            }
        }

        Ok(())
    }

    fn current_cpu_id(&self) -> usize {
        // SAFETY: raw_smp_processor_id() is always safe to call.
        unsafe { bindings::raw_smp_processor_id() as usize }
    }

    fn need_resched(&self) -> bool {
        // SAFETY: bedrock_need_resched just reads thread flags, no side effects
        unsafe { c_helpers::bedrock_need_resched() != 0 }
    }

    fn local_irq_enable(&self) {
        c_helpers::local_irq_enable();
    }

    fn local_irq_disable(&self) {
        c_helpers::local_irq_disable();
    }
}

/// Real MSR access using RDMSR/WRMSR instructions.
pub(crate) struct RealMsrAccess;

impl MsrAccess for RealMsrAccess {
    fn read_msr(&self, address: u32) -> Result<u64, MsrError> {
        let low: u32;
        let high: u32;
        // SAFETY: RDMSR is safe when the MSR address is valid; the caller is responsible for providing a valid address.
        unsafe {
            asm!(
                "rdmsr",
                in("ecx") address,
                out("eax") low,
                out("edx") high,
                options(nomem, nostack)
            );
        }
        Ok((u64::from(high) << 32) | u64::from(low))
    }

    fn write_msr(&self, address: u32, value: u64) -> Result<(), MsrError> {
        let low = value as u32;
        let high = (value >> 32) as u32;
        // SAFETY: WRMSR is safe when the MSR address and value are valid; the caller is responsible for correctness.
        unsafe {
            asm!(
                "wrmsr",
                in("ecx") address,
                in("eax") low,
                in("edx") high,
                options(nomem, nostack)
            );
        }
        Ok(())
    }
}

/// Real CR access using MOV CR instructions.
pub(crate) struct RealCrAccess;

impl CrAccess for RealCrAccess {
    fn read_cr0(&self) -> Result<Cr0, CrError> {
        let value: u64;
        // SAFETY: Reading CR0 is always safe and has no side effects.
        unsafe {
            asm!("mov {}, cr0", out(reg) value, options(nomem, nostack));
        }
        Ok(Cr0::new(value))
    }

    fn read_cr3(&self) -> Result<Cr3, CrError> {
        let value: u64;
        // SAFETY: Reading CR3 is always safe and has no side effects.
        unsafe {
            asm!("mov {}, cr3", out(reg) value, options(nomem, nostack));
        }
        Ok(Cr3::new(value))
    }

    fn read_cr4(&self) -> Result<Cr4, CrError> {
        let value: u64;
        // SAFETY: Reading CR4 is always safe and has no side effects.
        unsafe {
            asm!("mov {}, cr4", out(reg) value, options(nomem, nostack));
        }
        Ok(Cr4::new(value))
    }

    fn write_cr4(&self, cr4: &Cr4) -> Result<(), CrError> {
        // SAFETY: Writing CR4 with a valid value is safe; the caller ensures the value is correct.
        unsafe {
            asm!("mov cr4, {}", in(reg) cr4.bits(), options(nomem, nostack));
        }
        Ok(())
    }

    fn set_vmxe(&self) -> Result<(), CrError> {
        // SAFETY: Setting the VMXE bit in CR4 is safe when called before VMXON and the CPU supports VMX.
        unsafe { c_helpers::bedrock_cr4_set_vmxe() };
        Ok(())
    }

    fn clear_vmxe(&self) -> Result<(), CrError> {
        // SAFETY: Clearing the VMXE bit in CR4 is safe after VMXOFF has been executed.
        unsafe { c_helpers::bedrock_cr4_clear_vmxe() };
        Ok(())
    }
}

/// Real descriptor table access using segment register and descriptor table instructions.
pub(crate) struct RealDescriptorTableAccess;

impl DescriptorTableAccess for RealDescriptorTableAccess {
    fn read_cs(&self) -> SegmentSelector {
        let sel: u16;
        // SAFETY: Reading the CS segment selector is always safe and has no side effects.
        unsafe {
            asm!("mov {:x}, cs", out(reg) sel, options(nomem, nostack));
        }
        SegmentSelector::new(sel)
    }

    fn read_ss(&self) -> SegmentSelector {
        let sel: u16;
        // SAFETY: Reading the SS segment selector is always safe and has no side effects.
        unsafe {
            asm!("mov {:x}, ss", out(reg) sel, options(nomem, nostack));
        }
        SegmentSelector::new(sel)
    }

    fn read_ds(&self) -> SegmentSelector {
        let sel: u16;
        // SAFETY: Reading the DS segment selector is always safe and has no side effects.
        unsafe {
            asm!("mov {:x}, ds", out(reg) sel, options(nomem, nostack));
        }
        SegmentSelector::new(sel)
    }

    fn read_es(&self) -> SegmentSelector {
        let sel: u16;
        // SAFETY: Reading the ES segment selector is always safe and has no side effects.
        unsafe {
            asm!("mov {:x}, es", out(reg) sel, options(nomem, nostack));
        }
        SegmentSelector::new(sel)
    }

    fn read_fs(&self) -> SegmentSelector {
        let sel: u16;
        // SAFETY: Reading the FS segment selector is always safe and has no side effects.
        unsafe {
            asm!("mov {:x}, fs", out(reg) sel, options(nomem, nostack));
        }
        SegmentSelector::new(sel)
    }

    fn read_gs(&self) -> SegmentSelector {
        let sel: u16;
        // SAFETY: Reading the GS segment selector is always safe and has no side effects.
        unsafe {
            asm!("mov {:x}, gs", out(reg) sel, options(nomem, nostack));
        }
        SegmentSelector::new(sel)
    }

    fn read_tr(&self) -> SegmentSelector {
        let sel: u16;
        // SAFETY: STR reads the task register selector and is always safe with no side effects.
        unsafe {
            asm!("str {:x}", out(reg) sel, options(nomem, nostack));
        }
        SegmentSelector::new(sel)
    }

    fn read_tr_base(&self) -> u64 {
        // On Linux, the TSS base is stored in the per-CPU TSS structure.
        // We need to get this from the kernel's cpu_tss_rw.
        // For now, we read it by parsing the GDT entry pointed to by TR.
        // This is complex, so we use a simpler approach: read from the kernel's
        // per-CPU data structure.
        //
        // The kernel stores the TSS at a known per-CPU location.
        // We can use the kernel's this_cpu_ptr(&cpu_tss_rw) equivalent.
        //
        // For a minimal implementation, we read it from the GDT.
        // The TR selector points to a TSS descriptor in the GDT.
        let gdtr = self.read_gdtr();
        let tr = self.read_tr();
        let index = tr.bits() as usize >> 3; // Remove RPL and TI bits

        if index == 0 {
            return 0;
        }

        // TSS descriptor in 64-bit mode is 16 bytes (system segment descriptor)
        let desc_addr = gdtr.base as usize + index * 8;

        // Read the 16-byte TSS descriptor
        // Format (64-bit TSS descriptor):
        // Bytes 0-1: Limit 15:0
        // Bytes 2-3: Base 15:0
        // Byte 4: Base 23:16
        // Byte 5: Type/Attributes
        // Byte 6: Limit 19:16 / Flags
        // Byte 7: Base 31:24
        // Bytes 8-11: Base 63:32
        // Bytes 12-15: Reserved
        // SAFETY: desc_addr points into the GDT which is valid kernel memory, and the TR selector index is validated above.
        unsafe {
            let desc_ptr = desc_addr as *const u8;
            let base_low = u64::from(u16::from_le_bytes([*desc_ptr.add(2), *desc_ptr.add(3)]));
            let base_mid = u64::from(*desc_ptr.add(4));
            let base_high = u64::from(*desc_ptr.add(7));
            let base_upper = u64::from(u32::from_le_bytes([
                *desc_ptr.add(8),
                *desc_ptr.add(9),
                *desc_ptr.add(10),
                *desc_ptr.add(11),
            ]));

            base_low | (base_mid << 16) | (base_high << 24) | (base_upper << 32)
        }
    }

    fn read_gdtr(&self) -> Gdtr {
        let mut gdtr = Gdtr::new(0, 0);
        // SAFETY: SGDT stores the GDTR into the provided memory location; gdtr is a valid mutable reference.
        unsafe {
            asm!("sgdt [{}]", in(reg) &mut gdtr, options(nostack));
        }
        gdtr
    }

    fn read_idtr(&self) -> Idtr {
        let mut idtr = Idtr::new(0, 0);
        // SAFETY: SIDT stores the IDTR into the provided memory location; idtr is a valid mutable reference.
        unsafe {
            asm!("sidt [{}]", in(reg) &mut idtr, options(nostack));
        }
        idtr
    }
}

/// Linux kernel machine implementation.
pub(crate) struct LinuxMachine;

// SAFETY: LinuxMachine has no mutable state; all operations are CPU-local.
unsafe impl Send for LinuxMachine {}
// SAFETY: LinuxMachine is a zero-sized stateless type with no shared mutable data.
unsafe impl Sync for LinuxMachine {}

impl Machine for LinuxMachine {
    type P = KernelPage;
    type K = LinuxKernel;
    type M = RealMsrAccess;
    type C = RealCrAccess;
    type D = RealDescriptorTableAccess;
    type V = RealVmx;
    type Vcpu = super::vmxon::RealVmxCpu;

    fn kernel(&self) -> &Self::K {
        &LinuxKernel
    }

    fn msr_access(&self) -> &Self::M {
        &RealMsrAccess
    }

    fn cr_access(&self) -> &Self::C {
        &RealCrAccess
    }

    fn descriptor_table_access(&self) -> &Self::D {
        &RealDescriptorTableAccess
    }
}

/// Static machine instance for the module lifetime.
pub(crate) static MACHINE: LinuxMachine = LinuxMachine;
