/* SPDX-License-Identifier: GPL-2.0 */
/*
 * Bedrock VMX Assembly Support
 *
 * Assembly code for VMLAUNCH/VMRESUME and guest/host context switching.
 * Based on KVM's vmenter.S approach where HOST_RSP points to the kernel stack,
 * not to the VmxContext structure.
 */

#include <linux/linkage.h>
#include <linux/objtool.h>
#include <asm/asm.h>
#include <asm/frame.h>
#include <asm/nospec-branch.h>
#include <asm/unwind_hints.h>
#include <asm/segment.h>

.text

/* Offsets in struct vmx_context (must match Rust VmxContext struct) */
#define VMXCTX_GUEST_RAX	0
#define VMXCTX_GUEST_RBX	8
#define VMXCTX_GUEST_RCX	16
#define VMXCTX_GUEST_RDX	24
#define VMXCTX_GUEST_RSI	32
#define VMXCTX_GUEST_RDI	40
#define VMXCTX_GUEST_RBP	48
#define VMXCTX_GUEST_R8		56
#define VMXCTX_GUEST_R9		64
#define VMXCTX_GUEST_R10	72
#define VMXCTX_GUEST_R11	80
#define VMXCTX_GUEST_R12	88
#define VMXCTX_GUEST_R13	96
#define VMXCTX_GUEST_R14	104
#define VMXCTX_GUEST_R15	112

#define VMXCTX_HOST_RAX		120
#define VMXCTX_HOST_RBX		128
#define VMXCTX_HOST_RCX		136
#define VMXCTX_HOST_RDX		144
#define VMXCTX_HOST_RSI		152
#define VMXCTX_HOST_RDI		160
#define VMXCTX_HOST_RBP		168
#define VMXCTX_HOST_R8		176
#define VMXCTX_HOST_R9		184
#define VMXCTX_HOST_R10		192
#define VMXCTX_HOST_R11		200
#define VMXCTX_HOST_R12		208
#define VMXCTX_HOST_R13		216
#define VMXCTX_HOST_R14		224
#define VMXCTX_HOST_R15		232

#define VMXCTX_LAUNCHED		240

/* XSAVE state pointers and mask (for FPU/SSE/AVX isolation) */
#define VMXCTX_GUEST_XSAVE_PTR	248
#define VMXCTX_HOST_XSAVE_PTR	256
#define VMXCTX_XCR0_MASK	264
#define VMXCTX_HOST_XCR0	272
#define VMXCTX_GUEST_CR2	280

/*
 * vmx_run_guest - Enter VMX non-root operation
 *
 * int vmx_run_guest(struct vmx_context *ctx)
 *
 * Arguments:
 *   %rdi - pointer to struct vmx_context
 *
 * Returns:
 *   0 on successful VM exit
 *   Non-zero on VM entry failure
 *
 * IMPORTANT: The caller MUST have already set HOST_RSP in the VMCS to point
 * to the VmxContext. This function does NOT modify HOST_RSP.
 *
 * This function:
 * 1. Saves host GPRs to vmx_context
 * 2. Loads guest GPRs from vmx_context
 * 3. Executes VMLAUNCH (first time) or VMRESUME (subsequent)
 * 4. On VM exit, control transfers to vmx_exit_handler (HOST_RIP)
 *    where RSP = vmx_context (HOST_RSP), allowing guest state save
 */
SYM_FUNC_START(vmx_run_guest)
	/*
	 * Create stack frame explicitly - this is the anchor point for
	 * restoring RSP on VM exit.
	 */
	push	%rbp
	mov	%rsp, %rbp

	/*
	 * Save callee-saved registers on the kernel stack.
	 * These will be restored by vmx_exit_handler after VM exit.
	 */
	push	%r15
	push	%r14
	push	%r13
	push	%r12
	push	%rbx

	/*
	 * Save the VmxContext pointer on the stack so vmx_exit_handler
	 * can find it after VM exit when RSP is restored.
	 */
	push	%rdi

	/*
	 * Save host registers to vmx_context (using RDI as base).
	 * We save RBP as the frame pointer value for stack restoration.
	 */
	mov	%rax, VMXCTX_HOST_RAX(%rdi)
	mov	%rbx, VMXCTX_HOST_RBX(%rdi)
	mov	%rcx, VMXCTX_HOST_RCX(%rdi)
	mov	%rdx, VMXCTX_HOST_RDX(%rdi)
	mov	%rsi, VMXCTX_HOST_RSI(%rdi)
	mov	%rbp, VMXCTX_HOST_RBP(%rdi)  /* Save frame pointer for exit handler */
	mov	%r8,  VMXCTX_HOST_R8(%rdi)
	mov	%r9,  VMXCTX_HOST_R9(%rdi)
	mov	%r10, VMXCTX_HOST_R10(%rdi)
	mov	%r11, VMXCTX_HOST_R11(%rdi)
	mov	%r12, VMXCTX_HOST_R12(%rdi)
	mov	%r13, VMXCTX_HOST_R13(%rdi)
	mov	%r14, VMXCTX_HOST_R14(%rdi)
	mov	%r15, VMXCTX_HOST_R15(%rdi)
	/* RDI itself is saved later since we're using it as base */
	mov	%rdi, VMXCTX_HOST_RDI(%rdi)

	/*
	 * Save/restore extended state (FPU/SSE/AVX) for deterministic execution.
	 * Skip if xcr0_mask is 0 (XSAVE disabled).
	 *
	 * We also switch XCR0 between host and guest values so that XGETBV
	 * returns the correct value to the guest (prevents guest from seeing
	 * host's AVX-512 if we only support SSE/AVX).
	 */
	mov	VMXCTX_XCR0_MASK(%rdi), %rax
	test	%rax, %rax
	jz	.Lskip_xsave_entry

	/* Save current (host) XCR0 value */
	push	%rdi			/* Save ctx pointer */
	xor	%ecx, %ecx		/* XCR0 index = 0 */
	xgetbv				/* EDX:EAX = XCR0 */
	pop	%rdi			/* Restore ctx pointer */
	shl	$32, %rdx
	or	%rax, %rdx
	mov	%rdx, VMXCTX_HOST_XCR0(%rdi)

	/* Save host FPU/SSE state to host_xsave_ptr */
	mov	VMXCTX_XCR0_MASK(%rdi), %rax
	mov	%rax, %rdx
	shr	$32, %rdx		/* EDX = high 32 bits of mask */
	mov	VMXCTX_HOST_XSAVE_PTR(%rdi), %rcx
	xsave64	(%rcx)

	/* Set XCR0 to guest's value (xcr0_mask) before loading guest state */
	push	%rdi			/* Save ctx pointer */
	mov	VMXCTX_XCR0_MASK(%rdi), %rax
	mov	%rax, %rdx
	shr	$32, %rdx		/* EDX = high 32 bits */
	xor	%ecx, %ecx		/* XCR0 index = 0 */
	xsetbv				/* XCR0 = EDX:EAX */
	pop	%rdi			/* Restore ctx pointer */

	/* Load guest FPU/SSE state from guest_xsave_ptr */
	mov	VMXCTX_XCR0_MASK(%rdi), %rax
	mov	%rax, %rdx
	shr	$32, %rdx
	mov	VMXCTX_GUEST_XSAVE_PTR(%rdi), %rcx
	xrstor64 (%rcx)

.Lskip_xsave_entry:
	/*
	 * Restore guest CR2.
	 * CR2 is not part of the VMCS, so we must manually save/restore it.
	 * This is needed because page faults in the guest update CR2, and the
	 * host kernel might have page faults between VM exits that would
	 * overwrite the guest's CR2 value.
	 */
	mov	VMXCTX_GUEST_CR2(%rdi), %rax
	mov	%rax, %cr2

	/*
	 * Load guest GPRs from vmx_context.
	 * Keep RDI (ctx pointer) until last.
	 */
	mov	VMXCTX_GUEST_RAX(%rdi), %rax
	mov	VMXCTX_GUEST_RBX(%rdi), %rbx
	mov	VMXCTX_GUEST_RCX(%rdi), %rcx
	mov	VMXCTX_GUEST_RDX(%rdi), %rdx
	mov	VMXCTX_GUEST_RSI(%rdi), %rsi
	mov	VMXCTX_GUEST_RBP(%rdi), %rbp
	mov	VMXCTX_GUEST_R8(%rdi),  %r8
	mov	VMXCTX_GUEST_R9(%rdi),  %r9
	mov	VMXCTX_GUEST_R10(%rdi), %r10
	mov	VMXCTX_GUEST_R11(%rdi), %r11
	mov	VMXCTX_GUEST_R12(%rdi), %r12
	mov	VMXCTX_GUEST_R13(%rdi), %r13
	mov	VMXCTX_GUEST_R14(%rdi), %r14
	mov	VMXCTX_GUEST_R15(%rdi), %r15

	/*
	 * Check launch state and load guest RDI last.
	 * The launched flag tells us whether to use VMLAUNCH or VMRESUME.
	 */
	cmpl	$0, VMXCTX_LAUNCHED(%rdi)
	mov	VMXCTX_GUEST_RDI(%rdi), %rdi
	je	do_vmlaunch

	/* VMRESUME: Resume guest execution */
	vmresume
	jmp	vm_entry_failed

do_vmlaunch:
	/* VMLAUNCH: Launch guest for first time */
	vmlaunch
	/* Fall through to vm_entry_failed if VMLAUNCH fails */

vm_entry_failed:
	/*
	 * VM entry failed. RSP is still on the kernel stack.
	 * Pop the saved context pointer and restore state.
	 */
	pop	%rdi	/* Restore ctx pointer */

	/*
	 * Restore host XCR0 if we changed it (xcr0_mask != 0).
	 * This is needed because we set XCR0 to guest's value before VMLAUNCH/VMRESUME.
	 */
	mov	VMXCTX_XCR0_MASK(%rdi), %rax
	test	%rax, %rax
	jz	.Lskip_xcr0_restore_fail

	mov	VMXCTX_HOST_XCR0(%rdi), %rax
	mov	%rax, %rdx
	shr	$32, %rdx
	xor	%ecx, %ecx
	xsetbv

.Lskip_xcr0_restore_fail:
	/* Restore callee-saved registers from kernel stack */
	pop	%rbx
	pop	%r12
	pop	%r13
	pop	%r14
	pop	%r15

	/* Return -1 to indicate VM entry failure */
	mov	$-1, %rax

	pop	%rbp
	RET
SYM_FUNC_END(vmx_run_guest)
STACK_FRAME_NON_STANDARD(vmx_run_guest)

/*
 * vmx_exit_handler - VM exit landing point
 *
 * This is where control transfers on every VM exit.
 * HOST_RIP in the VMCS points here.
 *
 * On entry (hardware does this automatically):
 * - RSP is loaded from HOST_RSP (points to vmx_context)
 * - All guest GPRs are in their current CPU state
 * - VMCS contains exit information
 *
 * We must:
 * 1. Save guest GPRs to vmx_context
 * 2. Mark VMCS as launched
 * 3. Restore host GPRs from vmx_context
 * 4. Return to vmx_run_guest's caller via saved frame pointer
 */
SYM_FUNC_START(vmx_exit_handler)
	/*
	 * On VM exit, RSP = HOST_RSP = vmx_context pointer.
	 * Use RSP as base to save guest registers.
	 */
	mov	%rax, VMXCTX_GUEST_RAX(%rsp)
	mov	%rbx, VMXCTX_GUEST_RBX(%rsp)
	mov	%rcx, VMXCTX_GUEST_RCX(%rsp)
	mov	%rdx, VMXCTX_GUEST_RDX(%rsp)
	mov	%rsi, VMXCTX_GUEST_RSI(%rsp)
	mov	%rdi, VMXCTX_GUEST_RDI(%rsp)
	mov	%rbp, VMXCTX_GUEST_RBP(%rsp)
	mov	%r8,  VMXCTX_GUEST_R8(%rsp)
	mov	%r9,  VMXCTX_GUEST_R9(%rsp)
	mov	%r10, VMXCTX_GUEST_R10(%rsp)
	mov	%r11, VMXCTX_GUEST_R11(%rsp)
	mov	%r12, VMXCTX_GUEST_R12(%rsp)
	mov	%r13, VMXCTX_GUEST_R13(%rsp)
	mov	%r14, VMXCTX_GUEST_R14(%rsp)
	mov	%r15, VMXCTX_GUEST_R15(%rsp)

	/*
	 * Save guest CR2.
	 * CR2 is not part of the VMCS, so we must manually save/restore it.
	 * Use RAX which we already saved above.
	 */
	mov	%cr2, %rax
	mov	%rax, VMXCTX_GUEST_CR2(%rsp)

	/* Mark VMCS as launched (for next VM entry to use VMRESUME) */
	movl	$1, VMXCTX_LAUNCHED(%rsp)

	/*
	 * Save/restore extended state (FPU/SSE/AVX) for deterministic execution.
	 * Skip if xcr0_mask is 0 (XSAVE disabled).
	 *
	 * We restore host XCR0 after saving guest state so the host sees
	 * its original XCR0 value.
	 */
	mov	VMXCTX_XCR0_MASK(%rsp), %rax
	test	%rax, %rax
	jz	.Lskip_xsave_exit

	/* Save guest FPU/SSE state to guest_xsave_ptr (XCR0 is still guest's value) */
	mov	%rax, %rdx
	shr	$32, %rdx		/* EDX = high 32 bits of mask */
	mov	VMXCTX_GUEST_XSAVE_PTR(%rsp), %rcx
	xsave64	(%rcx)

	/* Restore host XCR0 before loading host FPU state */
	mov	VMXCTX_HOST_XCR0(%rsp), %rax
	mov	%rax, %rdx
	shr	$32, %rdx		/* EDX = high 32 bits */
	xor	%ecx, %ecx		/* XCR0 index = 0 */
	xsetbv				/* XCR0 = EDX:EAX */

	/* Load host FPU/SSE state from host_xsave_ptr */
	mov	VMXCTX_XCR0_MASK(%rsp), %rax
	mov	%rax, %rdx
	shr	$32, %rdx
	mov	VMXCTX_HOST_XSAVE_PTR(%rsp), %rcx
	xrstor64 (%rcx)

.Lskip_xsave_exit:
	/*
	 * Restore host GPRs from vmx_context.
	 * HOST_RBP contains the saved frame pointer from vmx_run_guest.
	 */
	mov	VMXCTX_HOST_RAX(%rsp), %rax
	mov	VMXCTX_HOST_RBX(%rsp), %rbx
	mov	VMXCTX_HOST_RCX(%rsp), %rcx
	mov	VMXCTX_HOST_RDX(%rsp), %rdx
	mov	VMXCTX_HOST_RSI(%rsp), %rsi
	mov	VMXCTX_HOST_RDI(%rsp), %rdi
	mov	VMXCTX_HOST_RBP(%rsp), %rbp  /* Restore frame pointer */
	mov	VMXCTX_HOST_R8(%rsp),  %r8
	mov	VMXCTX_HOST_R9(%rsp),  %r9
	mov	VMXCTX_HOST_R10(%rsp), %r10
	mov	VMXCTX_HOST_R11(%rsp), %r11
	mov	VMXCTX_HOST_R12(%rsp), %r12
	mov	VMXCTX_HOST_R13(%rsp), %r13
	mov	VMXCTX_HOST_R14(%rsp), %r14
	mov	VMXCTX_HOST_R15(%rsp), %r15

	/*
	 * Restore RSP to the kernel stack frame from vmx_run_guest.
	 * RBP was restored above and points to the saved frame.
	 * The stack layout at RBP is:
	 *   [rbp+8]  = return address
	 *   [rbp]    = saved rbp
	 *   [rbp-8]  = saved r15
	 *   [rbp-16] = saved r14
	 *   [rbp-24] = saved r13
	 *   [rbp-32] = saved r12
	 *   [rbp-40] = saved rbx
	 *   [rbp-48] = saved ctx pointer (already consumed by exit handler using RSP)
	 *
	 * Set RSP to rbp-40 so we can pop the callee-saved registers.
	 */
	lea	-40(%rbp), %rsp

	/* Restore callee-saved registers */
	pop	%rbx
	pop	%r12
	pop	%r13
	pop	%r14
	pop	%r15

	/* Return 0 to indicate successful VM exit */
	xor	%eax, %eax

	pop	%rbp
	RET
SYM_FUNC_END(vmx_exit_handler)
STACK_FRAME_NON_STANDARD(vmx_exit_handler)
