// SPDX-License-Identifier: GPL-2.0
//
// bedrock libpcguard: LLVM SanitizerCoverage trace-pc-guard frontend over
// libfeedback. Link it into a target built with
// `-fsanitize-coverage=trace-pc-guard` (and `-Wl,--build-id`, so the binary
// carries the build-id libfeedback keys the buffer on); the instrumentation
// calls __sanitizer_cov_trace_pc_guard_init once per module (handing us its
// table of 32-bit guard slots) and __sanitizer_cov_trace_pc_guard on every
// edge, which we turn into feedback_record() calls.
//
// Build this (and libfeedback.c) WITHOUT -fsanitize-coverage, or the hooks
// recurse into themselves.

#include <stddef.h>
#include <stdint.h>

#include "libfeedback.h"

// Total guards assigned across all _init calls. Written only from _init, a
// single-threaded load-time hook.
static uint32_t num_guards = 0;

// Called once per module at load (single-threaded, before any edge). Assign
// each still-zero guard a unique 1-based index (0 stays reserved for "disabled";
// the stock guard clauses skip an empty or already-initialized range), then size
// and register the buffer. feedback_buffer_init() is idempotent, so a single
// instrumented binary (one _init) sizes the buffer exactly; extra DSOs extend
// the count but can't grow the pinned buffer, so their guards alias modulo it.
void __sanitizer_cov_trace_pc_guard_init(uint32_t *start, uint32_t *stop) {
    if (start == stop || *start) {
        return;
    }
    for (uint32_t *guard = start; guard < stop; guard++) {
        *guard = ++num_guards;
    }
    // NULL build id: libfeedback keys on the binary's GNU build-id (the target
    // must be linked with -Wl,--build-id).
    feedback_buffer_init(num_guards, NULL);
}

// Every edge: bump its counter. _init already assigned the index and registered
// the buffer, so there's no setup on the hot path.
void __sanitizer_cov_trace_pc_guard(uint32_t *guard) {
    uint32_t idx = *guard;
    if (idx == 0) {
        return;
    }
    feedback_record(idx - 1); // 1-based guard -> 0-based buffer slot
}
