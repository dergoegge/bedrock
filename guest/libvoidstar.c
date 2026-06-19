// SPDX-License-Identifier: GPL-2.0
//
// bedrock libvoidstar: the Antithesis Go SDK's coverage backend, a thin frontend
// over libfeedback. The SDK loads it from the hardcoded path
// /usr/lib/libvoidstar.so and resolves five symbols: init_coverage_module,
// notify_coverage, and three unused fuzz_* hooks. We turn the edge callbacks
// into feedback_record() calls.
//
// The compiled .so must be installed as /usr/lib/libvoidstar.so regardless of
// this source's name — that path is hardcoded by the SDK loader.

#include <stddef.h>
#include <stdint.h>

#include "libfeedback.h"

// Called once at startup with the instrumentor's edge count and symbol-table
// name. The symbol-table name is content-hashed per build (e.g.
// "go-<hash>.sym.tsv"), so we pass it as the build id that scopes this binary's
// coverage buffer. Returns the edge base offset; we keep a single module at 0.
uint64_t init_coverage_module(size_t num_edges, const char *symbols) {
    feedback_buffer_init(num_edges, symbols);
    return 0;
}

// Called per edge. Returning true keeps the SDK reporting every hit, so the
// counter tracks hit counts (the host buckets them AFL-style); returning false
// would make it report each edge only once.
_Bool notify_coverage(size_t edge) {
    feedback_record(edge);
    return 1;
}

// Antithesis assertion/randomness hooks — unused, but the SDK loader requires
// all five symbols to be present.
void fuzz_json_data(const char *data, size_t size) {
    (void)data;
    (void)size;
}

void fuzz_flush(void) {}

uint64_t fuzz_get_random(void) {
    return 0;
}
