use super::Page;

#[cfg(not(feature = "cargo"))]
use crate::memory::{HostPhysAddr, VirtAddr};
#[cfg(feature = "cargo")]
use memory::{HostPhysAddr, VirtAddr};

/// A contiguous block of memory allocated for guest use.
///
/// This represents memory allocated via `vmalloc_user` (or similar) that can be
/// mapped into both the kernel and userspace. The memory is contiguous in virtual
/// address space but may be scattered in physical memory.
///
/// Implementations must ensure the memory is freed when dropped.
pub trait GuestMemory: Sized {
    /// Returns the size of the allocated memory in bytes.
    fn size(&self) -> usize;

    /// Returns the virtual address of the start of the memory region.
    ///
    /// This address is valid in the kernel's virtual address space.
    fn virt_addr(&self) -> VirtAddr;

    /// Returns a pointer to the start of the memory region.
    fn as_ptr(&self) -> *const u8 {
        self.virt_addr().as_u64() as *const u8
    }

    /// Returns a mutable pointer to the start of the memory region.
    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.virt_addr().as_u64() as *mut u8
    }

    /// Get the host physical address for a page at the given offset.
    ///
    /// This is needed for EPT mapping. The offset must be page-aligned.
    ///
    /// # Arguments
    ///
    /// * `page_offset` - Byte offset from the start of guest memory (must be page-aligned)
    ///
    /// # Returns
    ///
    /// The host physical address of the page, or `None` if the offset is out of range.
    fn page_phys_addr(&self, page_offset: usize) -> Option<HostPhysAddr>;
}

/// Trait representing low-level kernel operations.
pub trait Kernel {
    type P: Page;
    type G: GuestMemory;

    /// Allocate a zeroed page of memory.
    fn alloc_zeroed_page(&self) -> Option<Self::P>;

    /// Allocate a contiguous block of memory for guest use.
    ///
    /// The memory is zeroed and can be mapped into userspace.
    /// In the kernel, this uses `vmalloc_user`.
    ///
    /// # Arguments
    ///
    /// * `size` - The size in bytes to allocate (will be rounded up to page size)
    ///
    /// # Returns
    ///
    /// The allocated memory, or `None` if allocation fails.
    fn alloc_guest_memory(&self, size: usize) -> Option<Self::G>;

    /// Convert a host physical address to a virtual address.
    ///
    /// This is used by EPT code to access page table entries.
    fn phys_to_virt(&self, phys: HostPhysAddr) -> *mut u8;

    /// Execute a closure on all online CPUs with shared data.
    ///
    /// The closure executes in interrupt context, so it must not sleep.
    /// Waits for all CPUs to complete before returning.
    ///
    /// Returns the first error encountered, if any.
    fn call_on_all_cpus_with_data<F, T, E>(&self, data: &T, func: F) -> Result<(), E>
    where
        F: Fn(&T) -> Result<(), E> + Sync + Send,
        T: Sync,
        E: Send;

    /// Get current CPU/core ID.
    fn current_cpu_id(&self) -> usize;

    /// Check if the scheduler needs to run.
    ///
    /// This should be called in the VM run loop to allow the kernel scheduler
    /// to preempt VM execution if needed. Returns true if the current task
    /// should yield to the scheduler.
    ///
    /// In the kernel, this checks the TIF_NEED_RESCHED flag.
    /// In tests, this always returns false.
    fn need_resched(&self) -> bool;

    /// Enable local interrupts.
    ///
    /// Called to allow pending external interrupts to be delivered.
    fn local_irq_enable(&self);

    /// Disable local interrupts.
    ///
    /// Called before VM entry to ensure interrupts are disabled.
    fn local_irq_disable(&self);
}

/// RAII guard that disables local interrupts while held.
///
/// Interrupts are disabled when the guard is created and re-enabled when dropped.
/// This protects the XCR0 switch around VM entry/exit: the assembly sets XCR0 to
/// the guest value (which may lack AVX-512 bits) before VMRESUME, and an interrupt
/// firing in that window would crash if the handler uses AVX-512 instructions.
pub struct IrqGuard<'a, K: Kernel> {
    kernel: &'a K,
}

impl<'a, K: Kernel> IrqGuard<'a, K> {
    /// Disable local interrupts and return a guard.
    ///
    /// Interrupts will be re-enabled when the guard is dropped.
    #[inline]
    pub fn new(kernel: &'a K) -> Self {
        kernel.local_irq_disable();
        Self { kernel }
    }
}

impl<K: Kernel> Drop for IrqGuard<'_, K> {
    #[inline]
    fn drop(&mut self) {
        self.kernel.local_irq_enable();
    }
}

/// RAII guard that enables local interrupts while held.
///
/// The inverse of [`IrqGuard`]: interrupts are enabled on creation and disabled
/// on drop. Use this inside an `IrqGuard` scope to create a brief window for
/// servicing pending host interrupts (timer ticks, IPIs).
pub struct ReverseIrqGuard<'a, K: Kernel> {
    kernel: &'a K,
}

impl<'a, K: Kernel> ReverseIrqGuard<'a, K> {
    /// Enable local interrupts and return a guard.
    ///
    /// Interrupts will be disabled again when the guard is dropped.
    #[inline]
    pub fn new(kernel: &'a K) -> Self {
        kernel.local_irq_enable();
        Self { kernel }
    }
}

impl<K: Kernel> Drop for ReverseIrqGuard<'_, K> {
    #[inline]
    fn drop(&mut self) {
        self.kernel.local_irq_disable();
    }
}
