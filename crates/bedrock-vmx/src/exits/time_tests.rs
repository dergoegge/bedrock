// SPDX-License-Identifier: GPL-2.0

use super::*;
use crate::exits::reasons::ExitReason;
use crate::tests::MockVmContext;

#[test]
fn test_rdtsc_handler() {
    let mut ctx = MockVmContext::new();
    ctx.set_emulated_tsc(0x1234_5678_9ABC_DEF0);
    // Set up VMCS fields needed for advance_rip
    ctx.set_exit_reason(ExitReason::Rdtsc);
    ctx.set_instruction_len(2); // RDTSC is 2 bytes (0x0F 0x31)
    ctx.set_guest_rip(0x1000);

    let result = handle_rdtsc(&mut ctx);
    assert!(matches!(result, ExitHandlerResult::Continue));

    // Check that TSC was split into EDX:EAX correctly
    assert_eq!(ctx.state().gprs.rax, 0x9ABC_DEF0);
    assert_eq!(ctx.state().gprs.rdx, 0x1234_5678);
    // Check RIP was advanced
    assert_eq!(ctx.get_guest_rip(), Some(0x1002));
}

#[test]
fn test_rdtscp_handler() {
    let mut ctx = MockVmContext::new();
    ctx.set_emulated_tsc(0xAABB_CCDD_EEFF_0011);
    ctx.state_mut().msr_state.tsc_aux = 0x42;
    // Set up VMCS fields needed for advance_rip
    ctx.set_exit_reason(ExitReason::Rdtscp);
    ctx.set_instruction_len(3); // RDTSCP is 3 bytes (0x0F 0x01 0xF9)
    ctx.set_guest_rip(0x1000);

    let result = handle_rdtscp(&mut ctx);
    assert!(matches!(result, ExitHandlerResult::Continue));

    // Check that TSC was split into EDX:EAX correctly
    assert_eq!(ctx.state().gprs.rax, 0xEEFF_0011);
    assert_eq!(ctx.state().gprs.rdx, 0xAABB_CCDD);
    // Check TSC_AUX in ECX
    assert_eq!(ctx.state().gprs.rcx, 0x42);
    // Check RIP was advanced
    assert_eq!(ctx.get_guest_rip(), Some(0x1003));
}

#[test]
fn test_rdpmc_handler() {
    let mut ctx = MockVmContext::new();
    ctx.state_mut().gprs.rcx = 0; // Counter index
                                  // Set up VMCS fields needed for advance_rip
    ctx.set_exit_reason(ExitReason::Rdpmc);
    ctx.set_instruction_len(2); // RDPMC is 2 bytes (0x0F 0x33)
    ctx.set_guest_rip(0x1000);

    let result = handle_rdpmc(&mut ctx);
    assert!(matches!(result, ExitHandlerResult::Continue));

    // RDPMC should return 0
    assert_eq!(ctx.state().gprs.rax, 0);
    assert_eq!(ctx.state().gprs.rdx, 0);
    // Check RIP was advanced
    assert_eq!(ctx.get_guest_rip(), Some(0x1002));
}
