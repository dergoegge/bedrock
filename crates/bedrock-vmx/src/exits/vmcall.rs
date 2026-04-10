// SPDX-License-Identifier: GPL-2.0

//! VMCALL exit handler for hypercall dispatch.

use super::ept::translate_gva_range_to_gpas;
use super::helpers::{advance_rip, ExitHandlerResult};
use super::reasons::ExitReason;

#[cfg(not(feature = "cargo"))]
use super::super::prelude::*;
#[cfg(feature = "cargo")]
use crate::prelude::*;

/// Maximum feedback buffer size (1 MB = 256 pages).
const MAX_FEEDBACK_BUFFER_SIZE: u64 = FEEDBACK_BUFFER_MAX_PAGES as u64 * 4096;

/// Handle VMCALL exit by dispatching based on hypercall number in RAX.
pub fn handle_vmcall<C: VmContext, A: CowAllocator<C::CowPage>>(
    ctx: &mut C,
    allocator: &mut A,
) -> ExitHandlerResult {
    let hypercall_nr = ctx.state().gprs.rax;

    match hypercall_nr {
        HYPERCALL_SHUTDOWN => {
            // Log shutdown state if AtShutdown mode is enabled
            ctx.state_mut().log_shutdown();

            if let Err(e) = advance_rip(ctx) {
                return ExitHandlerResult::Error(e);
            }
            ExitHandlerResult::ExitToUserspace(ExitReason::VmcallShutdown)
        }
        HYPERCALL_SNAPSHOT => {
            // Log snapshot state (if logging is enabled)
            ctx.state_mut().log_snapshot();

            if let Err(e) = advance_rip(ctx) {
                return ExitHandlerResult::Error(e);
            }
            ExitHandlerResult::ExitToUserspace(ExitReason::VmcallSnapshot)
        }
        HYPERCALL_REGISTER_FEEDBACK_BUFFER => {
            // Read arguments: GVA in RBX, size in RCX, buffer index in RDX
            let gva = ctx.state().gprs.rbx;
            let size = ctx.state().gprs.rcx;
            let buffer_idx = ctx.state().gprs.rdx as usize;

            // Validate buffer index
            if buffer_idx >= MAX_FEEDBACK_BUFFERS {
                log_err!(
                    "HYPERCALL_REGISTER_FEEDBACK_BUFFER: invalid buffer index {} (max {})\n",
                    buffer_idx,
                    MAX_FEEDBACK_BUFFERS - 1
                );
                ctx.state_mut().gprs.rax = !0u64; // Return -1
                if let Err(e) = advance_rip(ctx) {
                    return ExitHandlerResult::Error(e);
                }
                return ExitHandlerResult::Continue;
            }

            // Validate size: must be > 0 and <= 1MB
            if size == 0 || size > MAX_FEEDBACK_BUFFER_SIZE {
                log_err!(
                    "HYPERCALL_REGISTER_FEEDBACK_BUFFER: invalid size {}\n",
                    size
                );
                ctx.state_mut().gprs.rax = !0u64; // Return -1
                if let Err(e) = advance_rip(ctx) {
                    return ExitHandlerResult::Error(e);
                }
                return ExitHandlerResult::Continue;
            }

            // Translate GVA range to GPAs
            let mut gpas = [0u64; FEEDBACK_BUFFER_MAX_PAGES];
            let num_pages = match translate_gva_range_to_gpas(ctx, gva, size, &mut gpas) {
                Ok(n) => n,
                Err(()) => {
                    log_err!(
                        "HYPERCALL_REGISTER_FEEDBACK_BUFFER: GVA translation failed gva={:#x} size={}\n",
                        gva, size
                    );
                    ctx.state_mut().gprs.rax = !0u64; // Return -1
                    if let Err(e) = advance_rip(ctx) {
                        return ExitHandlerResult::Error(e);
                    }
                    return ExitHandlerResult::Continue;
                }
            };

            // Store feedback buffer info in VmState at the specified index
            ctx.state_mut().feedback_buffers[buffer_idx] = Some(FeedbackBufferInfo {
                gva,
                size,
                num_pages,
                gpas,
            });

            // Pre-COW feedback buffer pages for stable userspace mapping.
            // This handles the case where the feedback buffer is registered after fork.
            ctx.pre_cow_feedback_buffer_at(buffer_idx, allocator);

            log_info!(
                "HYPERCALL_REGISTER_FEEDBACK_BUFFER: registered idx={} gva={:#x} size={} pages={}\n",
                buffer_idx,
                gva,
                size,
                num_pages
            );

            ctx.state_mut().gprs.rax = 0; // Return success
            if let Err(e) = advance_rip(ctx) {
                return ExitHandlerResult::Error(e);
            }
            // Exit to userspace so it can map the feedback buffer
            ExitHandlerResult::ExitToUserspace(ExitReason::VmcallFeedbackBuffer)
        }
        _ => {
            // Unknown hypercall - exit to userspace with generic Vmcall reason
            ExitHandlerResult::ExitToUserspace(ExitReason::Vmcall)
        }
    }
}
