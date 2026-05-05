// SPDX-License-Identifier: GPL-2.0
/*
 * Bedrock kernel module C helpers
 *
 * These functions wrap inline functions and macros from the Linux kernel
 * that cannot be directly called from Rust.
 */

#include <linux/module.h>
#include <linux/gfp.h>
#include <linux/mm.h>
#include <linux/smp.h>
#include <linux/vmalloc.h>
#include <linux/anon_inodes.h>
#include <linux/file.h>
#include <linux/uaccess.h>
#include <linux/preempt.h>
#include <linux/percpu.h>
#include <linux/xxhash.h>
#include <asm/io.h>
#include <asm/msr.h>
#include <asm/tlbflush.h>

/*
 * VMX capabilities structure.
 * Matches the Rust VmxCapabilities struct layout.
 */
struct bedrock_vmx_caps {
	u32 pin_based_exec_ctrl;
	u32 cpu_based_exec_ctrl;
	u32 cpu_based_exec_ctrl2;
	u32 vmexit_ctrl;
	u32 vmentry_ctrl;
	u64 cr0_fixed0;
	u64 cr0_fixed1;
	u64 cr4_fixed0;
	u64 cr4_fixed1;
	bool has_ept;
	bool has_vpid;
};

/*
 * Per-CPU VMX state.
 * This replaces the Rust PerCpu<RealVmxCpu> which doesn't work correctly
 * because Rust's #[link_section = ".data..percpu"] doesn't generate proper
 * per-CPU relocations like C's DEFINE_PER_CPU does.
 */
struct bedrock_vcpu {
	bool vmxon;
	u64 vmxon_region_phys;
	u64 vmxon_region_virt;
	struct bedrock_vmx_caps capabilities;
};

static DEFINE_PER_CPU(struct bedrock_vcpu, bedrock_pcpu_vcpu);

/*
 * Convert a struct page pointer to its physical address.
 * This wraps the page_to_phys() macro.
 */
phys_addr_t bedrock_page_to_phys(struct page *page)
{
	return page_to_phys(page);
}
EXPORT_SYMBOL_GPL(bedrock_page_to_phys);

/*
 * Get the kernel virtual address for a page.
 * This wraps the page_address() function/macro.
 */
void *bedrock_page_address(struct page *page)
{
	return page_address(page);
}
EXPORT_SYMBOL_GPL(bedrock_page_address);

/*
 * Execute a function on each online CPU sequentially with per-CPU error handling.
 *
 * This uses for_each_online_cpu() + smp_call_function_single() to call the
 * function on each CPU one at a time, allowing early exit on error.
 *
 * The callback function receives a pointer to bedrock_cpu_call_info which
 * contains the user-provided info pointer and an error field that the callback
 * should set on error.
 *
 * Returns: 0 on success, or the first error encountered.
 *          If an error occurs, *failed_cpu will be set to the CPU that failed.
 */
struct bedrock_cpu_call_info {
	void *info;
	int error;
};

int bedrock_for_each_cpu(smp_call_func_t func, void *info, int *failed_cpu)
{
	int cpu, ret;
	struct bedrock_cpu_call_info call_info = {
		.info = info,
		.error = 0,
	};

	for_each_online_cpu(cpu) {
		ret = smp_call_function_single(cpu, func, &call_info, 1);
		if (ret) {
			/* smp_call_function_single itself failed */
			if (failed_cpu)
				*failed_cpu = cpu;
			return ret;
		}
		if (call_info.error) {
			/* The callback reported an error */
			if (failed_cpu)
				*failed_cpu = cpu;
			return call_info.error;
		}
	}

	return 0;
}
EXPORT_SYMBOL_GPL(bedrock_for_each_cpu);

/*
 * Allocate zeroed memory that can be mapped to userspace.
 * This wraps vmalloc_user() which allocates virtually contiguous memory
 * that is suitable for mmap'ing to userspace.
 */
void *bedrock_vmalloc_user(unsigned long size)
{
	return vmalloc_user(size);
}
EXPORT_SYMBOL_GPL(bedrock_vmalloc_user);

/*
 * Free memory allocated with bedrock_vmalloc_user.
 * This wraps vfree().
 */
void bedrock_vfree(void *addr)
{
	vfree(addr);
}
EXPORT_SYMBOL_GPL(bedrock_vfree);

/*
 * Get the physical address of a page within vmalloc memory.
 * This wraps vmalloc_to_page() + page_to_phys().
 *
 * Returns the physical address, or 0 if the address is not valid vmalloc memory.
 */
phys_addr_t bedrock_vmalloc_to_phys(void *addr)
{
	struct page *page = vmalloc_to_page(addr);
	if (!page)
		return 0;
	return page_to_phys(page);
}
EXPORT_SYMBOL_GPL(bedrock_vmalloc_to_phys);

/*
 * Convert any kernel virtual address to its physical address.
 * Handles both vmalloc and direct-mapped (kmalloc/alloc_page) addresses.
 *
 * Returns the physical address, or 0 if the address is invalid.
 */
phys_addr_t bedrock_kva_to_phys(void *addr)
{
	struct page *page;

	if (is_vmalloc_addr(addr)) {
		page = vmalloc_to_page(addr);
	} else {
		page = virt_to_page(addr);
	}

	if (!page)
		return 0;

	return page_to_phys(page) + offset_in_page(addr);
}
EXPORT_SYMBOL_GPL(bedrock_kva_to_phys);

/*
 * Convert a physical address to a kernel virtual address.
 * This wraps __va() for direct-mapped physical memory.
 */
void *bedrock_phys_to_virt(phys_addr_t phys)
{
	return __va(phys);
}
EXPORT_SYMBOL_GPL(bedrock_phys_to_virt);

/*
 * Create an anonymous inode and return a file descriptor for it.
 * This wraps anon_inode_getfd() which creates a new file descriptor
 * pointing to an anonymous inode with the given file operations.
 *
 * The priv pointer is stored in file->private_data and can be retrieved
 * in the file operations callbacks.
 *
 * Returns: file descriptor on success, negative error code on failure.
 */
int bedrock_anon_inode_getfd(const char *name,
			     const struct file_operations *fops,
			     void *priv, int flags)
{
	return anon_inode_getfd(name, fops, priv, flags);
}
EXPORT_SYMBOL_GPL(bedrock_anon_inode_getfd);

/*
 * Copy data from userspace to kernel space.
 * This wraps copy_from_user().
 *
 * Returns: Number of bytes that could NOT be copied (0 on success).
 */
unsigned long bedrock_copy_from_user(void *to, const void __user *from,
				     unsigned long n)
{
	return copy_from_user(to, from, n);
}
EXPORT_SYMBOL_GPL(bedrock_copy_from_user);

/*
 * Copy data from kernel space to userspace.
 * This wraps copy_to_user().
 *
 * Returns: Number of bytes that could NOT be copied (0 on success).
 */
unsigned long bedrock_copy_to_user(void __user *to, const void *from,
				   unsigned long n)
{
	return copy_to_user(to, from, n);
}
EXPORT_SYMBOL_GPL(bedrock_copy_to_user);

/*
 * Map vmalloc memory into a userspace VMA.
 * This wraps remap_vmalloc_range().
 *
 * The vmalloc memory must have been allocated with vmalloc_user().
 *
 * Returns: 0 on success, negative error code on failure.
 */
int bedrock_remap_vmalloc_range(struct vm_area_struct *vma, void *addr,
				unsigned long pgoff)
{
	return remap_vmalloc_range(vma, addr, pgoff);
}
EXPORT_SYMBOL_GPL(bedrock_remap_vmalloc_range);

/*
 * Map a single page into a userspace VMA.
 * This wraps remap_pfn_range() for a single page.
 *
 * The page should be allocated via alloc_page() or similar.
 *
 * Returns: 0 on success, negative error code on failure.
 */
int bedrock_remap_page(struct vm_area_struct *vma, struct page *page)
{
	unsigned long pfn = page_to_pfn(page);
	unsigned long size = vma->vm_end - vma->vm_start;

	/* Mark VMA as IO memory to prevent merging and other issues */
	vm_flags_set(vma, VM_IO | VM_PFNMAP | VM_DONTEXPAND | VM_DONTDUMP);

	return remap_pfn_range(vma, vma->vm_start, pfn, size, vma->vm_page_prot);
}
EXPORT_SYMBOL_GPL(bedrock_remap_page);

/*
 * Map multiple (potentially non-contiguous) physical pages into a userspace VMA.
 *
 * This function maps an array of host physical addresses to a contiguous
 * userspace virtual address range. Each HPA is a page-aligned physical address.
 *
 * The VMA size must equal num_pages * PAGE_SIZE.
 *
 * Returns: 0 on success, negative error code on failure.
 */
int bedrock_remap_pages(struct vm_area_struct *vma, u64 *hpas, int num_pages)
{
	int i, ret;
	unsigned long addr;
	unsigned long expected_size = (unsigned long)num_pages * PAGE_SIZE;
	unsigned long actual_size = vma->vm_end - vma->vm_start;

	if (actual_size != expected_size) {
		pr_err("bedrock: remap_pages size mismatch: expected %lu, got %lu\n",
		       expected_size, actual_size);
		return -EINVAL;
	}

	/* Mark VMA as IO memory to prevent merging and other issues */
	vm_flags_set(vma, VM_IO | VM_PFNMAP | VM_DONTEXPAND | VM_DONTDUMP);

	addr = vma->vm_start;
	for (i = 0; i < num_pages; i++) {
		unsigned long pfn = hpas[i] >> PAGE_SHIFT;

		ret = remap_pfn_range(vma, addr, pfn, PAGE_SIZE, vma->vm_page_prot);
		if (ret) {
			pr_err("bedrock: remap_pfn_range failed for page %d: %d\n", i, ret);
			return ret;
		}

		addr += PAGE_SIZE;
	}

	return 0;
}
EXPORT_SYMBOL_GPL(bedrock_remap_pages);

/*
 * Get VMA start address.
 * Wraps vma->vm_start for Rust compatibility.
 */
unsigned long bedrock_vma_start(struct vm_area_struct *vma)
{
	return vma->vm_start;
}
EXPORT_SYMBOL_GPL(bedrock_vma_start);

/*
 * Get VMA end address.
 * Wraps vma->vm_end for Rust compatibility.
 */
unsigned long bedrock_vma_end(struct vm_area_struct *vma)
{
	return vma->vm_end;
}
EXPORT_SYMBOL_GPL(bedrock_vma_end);

/*
 * Get VMA page offset.
 * Wraps vma->vm_pgoff for Rust compatibility.
 */
unsigned long bedrock_vma_pgoff(struct vm_area_struct *vma)
{
	return vma->vm_pgoff;
}
EXPORT_SYMBOL_GPL(bedrock_vma_pgoff);

/*
 * Disable preemption on the current CPU.
 * This wraps preempt_disable().
 */
void bedrock_preempt_disable(void)
{
	preempt_disable();
}
EXPORT_SYMBOL_GPL(bedrock_preempt_disable);

/*
 * Enable preemption on the current CPU.
 * This wraps preempt_enable().
 */
void bedrock_preempt_enable(void)
{
	preempt_enable();
}
EXPORT_SYMBOL_GPL(bedrock_preempt_enable);

/*
 * Check if the current task needs to be rescheduled.
 * This wraps the need_resched() inline function.
 *
 * Returns: non-zero if TIF_NEED_RESCHED is set, 0 otherwise.
 */
int bedrock_need_resched(void)
{
	return need_resched();
}
EXPORT_SYMBOL_GPL(bedrock_need_resched);

/*
 * Enable local interrupts.
 * This wraps local_irq_enable() for Rust code.
 */
void bedrock_local_irq_enable(void)
{
	local_irq_enable();
}
EXPORT_SYMBOL_GPL(bedrock_local_irq_enable);

/*
 * Disable local interrupts.
 * This wraps local_irq_disable() for Rust code.
 */
void bedrock_local_irq_disable(void)
{
	local_irq_disable();
}
EXPORT_SYMBOL_GPL(bedrock_local_irq_disable);

/*
 * Per-CPU VCPU accessors.
 * These use this_cpu_ptr() to properly access per-CPU data.
 */

/*
 * Check if VMX is enabled on the current CPU.
 */
bool bedrock_vcpu_is_vmxon(void)
{
	struct bedrock_vcpu *vcpu = this_cpu_ptr(&bedrock_pcpu_vcpu);
	return vcpu->vmxon;
}
EXPORT_SYMBOL_GPL(bedrock_vcpu_is_vmxon);

/*
 * Set VMX enabled state on the current CPU.
 */
void bedrock_vcpu_set_vmxon(bool enabled)
{
	struct bedrock_vcpu *vcpu = this_cpu_ptr(&bedrock_pcpu_vcpu);
	vcpu->vmxon = enabled;
}
EXPORT_SYMBOL_GPL(bedrock_vcpu_set_vmxon);

/*
 * Get VMX capabilities for the current CPU.
 * Returns a pointer that is valid while preemption is disabled.
 */
const struct bedrock_vmx_caps *bedrock_vcpu_get_capabilities(void)
{
	struct bedrock_vcpu *vcpu = this_cpu_ptr(&bedrock_pcpu_vcpu);
	return &vcpu->capabilities;
}
EXPORT_SYMBOL_GPL(bedrock_vcpu_get_capabilities);

/*
 * Set VMX capabilities for the current CPU.
 */
void bedrock_vcpu_set_capabilities(u32 pin_based, u32 cpu_based, u32 cpu_based2,
				   u32 vmexit, u32 vmentry,
				   u64 cr0_fixed0, u64 cr0_fixed1,
				   u64 cr4_fixed0, u64 cr4_fixed1,
				   bool has_ept, bool has_vpid)
{
	struct bedrock_vcpu *vcpu = this_cpu_ptr(&bedrock_pcpu_vcpu);
	vcpu->capabilities.pin_based_exec_ctrl = pin_based;
	vcpu->capabilities.cpu_based_exec_ctrl = cpu_based;
	vcpu->capabilities.cpu_based_exec_ctrl2 = cpu_based2;
	vcpu->capabilities.vmexit_ctrl = vmexit;
	vcpu->capabilities.vmentry_ctrl = vmentry;
	vcpu->capabilities.cr0_fixed0 = cr0_fixed0;
	vcpu->capabilities.cr0_fixed1 = cr0_fixed1;
	vcpu->capabilities.cr4_fixed0 = cr4_fixed0;
	vcpu->capabilities.cr4_fixed1 = cr4_fixed1;
	vcpu->capabilities.has_ept = has_ept;
	vcpu->capabilities.has_vpid = has_vpid;
}
EXPORT_SYMBOL_GPL(bedrock_vcpu_set_capabilities);

/*
 * Set VMXON region for the current CPU.
 */
void bedrock_vcpu_set_vmxon_region(u64 phys, u64 virt)
{
	struct bedrock_vcpu *vcpu = this_cpu_ptr(&bedrock_pcpu_vcpu);
	vcpu->vmxon_region_phys = phys;
	vcpu->vmxon_region_virt = virt;
}
EXPORT_SYMBOL_GPL(bedrock_vcpu_set_vmxon_region);

/*
 * Set CR4.VMXE using cr4_set_bits() to properly update the kernel's CR4 shadow.
 *
 * Raw MOV to CR4 does NOT update the kernel's cpu_tlbstate.cr4 shadow.
 * If the shadow is stale, the kernel may later write CR4 without VMXE during
 * context switches (cr4_update_irqsoff), causing #GP while in VMX operation.
 */
void bedrock_cr4_set_vmxe(void)
{
	cr4_set_bits(X86_CR4_VMXE);
}
EXPORT_SYMBOL_GPL(bedrock_cr4_set_vmxe);

/*
 * Clear CR4.VMXE using cr4_clear_bits() to properly update the kernel's CR4 shadow.
 * Must only be called after VMXOFF (outside VMX operation).
 */
void bedrock_cr4_clear_vmxe(void)
{
	cr4_clear_bits(X86_CR4_VMXE);
}
EXPORT_SYMBOL_GPL(bedrock_cr4_clear_vmxe);

/*
 * XXH64 hashing wrappers.
 * These wrap the kernel's xxhash implementation for use from Rust.
 */

/*
 * One-shot xxh64 hash.
 */
u64 bedrock_xxh64(const void *input, size_t length, u64 seed)
{
	return xxh64(input, length, seed);
}
EXPORT_SYMBOL_GPL(bedrock_xxh64);

/*
 * Reset xxh64 state for streaming hashing.
 */
void bedrock_xxh64_reset(struct xxh64_state *state, u64 seed)
{
	xxh64_reset(state, seed);
}
EXPORT_SYMBOL_GPL(bedrock_xxh64_reset);

/*
 * Update xxh64 state with more data.
 */
void bedrock_xxh64_update(struct xxh64_state *state, const void *input,
			  size_t length)
{
	xxh64_update(state, input, length);
}
EXPORT_SYMBOL_GPL(bedrock_xxh64_update);

/*
 * Finalize and return the xxh64 hash.
 */
u64 bedrock_xxh64_digest(const struct xxh64_state *state)
{
	return xxh64_digest(state);
}
EXPORT_SYMBOL_GPL(bedrock_xxh64_digest);
