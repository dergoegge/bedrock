/* SPDX-License-Identifier: GPL-2.0 */
/*
 * libfeedback — generic guest-side coverage-feedback backend.
 *
 * Owns the process-wide hitcount buffer the host reads back as fuzzing
 * coverage: maps it onto a persistent-tmpfs file, registers it once with the
 * host over HYPERCALL_REGISTER_FEEDBACK_BUFFER (libvmcall), and bumps saturating
 * per-edge counters. The buffer id is "cov-<build>", scoping coverage to one
 * build of the binary. Frontends translate an instrumentation ABI into
 * feedback_record() calls and supply <build>: libvoidstar.c (the Antithesis Go
 * SDK's init_coverage_module / notify_coverage hooks) passes the instrumentor's
 * symbol-table name; libpcguard.c (LLVM trace-pc-guard) passes NULL and lets the
 * GNU build-id stand in. See libfeedback.c for details.
 */

#ifndef BEDROCK_LIBFEEDBACK_H
#define BEDROCK_LIBFEEDBACK_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Map and register the process-wide coverage buffer: `num_edges` byte counters
 * (rounded up to a page, capped at the host's 1 MiB limit), registered under
 * the id "cov-<build>". `build_id` is the binary's build identifier; pass NULL
 * to fall back to its GNU build-id note. Idempotent and thread-safe. Returns the
 * buffer base, or NULL on failure (after which feedback_record() is a safe
 * no-op).
 */
uint8_t *feedback_buffer_init(size_t num_edges, const char *build_id);

/*
 * Saturating-increment the counter for `edge` (taken modulo the buffer size;
 * saturates at 0xff). A no-op until feedback_buffer_init() has succeeded.
 */
void feedback_record(uint64_t edge);

/* The live buffer base and size in bytes (NULL / 0 before init), for frontends
 * that index the map directly. */
uint8_t *feedback_buffer(void);
size_t feedback_buffer_size(void);

#ifdef __cplusplus
}
#endif

#endif /* BEDROCK_LIBFEEDBACK_H */
