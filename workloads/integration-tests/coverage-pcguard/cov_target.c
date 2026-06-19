// SPDX-License-Identifier: GPL-2.0
// Integration-test driver: an LLVM trace-pc-guard-instrumented program whose
// edge coverage is recorded through libpcguard -> libfeedback for the host to
// read back (tests/integration/coverage.rs).
//
// cov_target.c is compiled WITH -fsanitize-coverage=trace-pc-guard and linked
// with -Wl,--build-id (so libfeedback can key the buffer on the build-id); the
// runtime (libpcguard.c, libfeedback.c) is compiled WITHOUT the coverage flag
// (see the Dockerfile).
//
// To let the test observe a coverage *increase* on one accumulating buffer, the
// driver runs in two phases against the same registered buffer:
//   1. run the shallow stage, then touch READY_PATH (baseline coverage is in);
//   2. block until the test creates MORE_PATH, then run the deeper stages, which
//      light edges the shallow stage didn't.
// It then idles so the buffer stays mapped.

#include <fcntl.h>
#include <unistd.h>

#define READY_PATH "/tmp/cov-ready"
#define MORE_PATH "/tmp/cov-more"

// Distinct noinline stages, each with its own branches so trace-pc-guard gives
// each its own guards; running more of them lights strictly more edges. noinline
// keeps the edges from being folded away at -O2.
__attribute__((noinline)) static long stage0(int n) {
    long a = 0;
    for (int i = 0; i < n; i++) {
        a += (i & 1) ? i : -i;
    }
    return a;
}

__attribute__((noinline)) static long stage1(int n) {
    long a = 0;
    for (int i = 0; i < n; i++) {
        if (i % 3 == 0) {
            a += i;
        } else if (i % 3 == 1) {
            a -= i;
        } else {
            a ^= i;
        }
    }
    return a;
}

__attribute__((noinline)) static long stage2(int n) {
    long a = 0;
    for (int i = 0; i < n; i++) {
        switch (i % 4) {
        case 0:
            a += i * 2;
            break;
        case 1:
            a -= i;
            break;
        case 2:
            a += i / 2;
            break;
        default:
            a ^= (i << 1);
            break;
        }
    }
    return a;
}

__attribute__((noinline)) static long stage3(int n) {
    long a = 0;
    for (int i = 0; i < n; i++) {
        if (i > 50) {
            a += (i & 1) ? i : -i;
        } else {
            a += i % 7;
        }
    }
    return a;
}

static void touch(const char *path) {
    int fd = open(path, O_CREAT | O_WRONLY, 0600);
    if (fd >= 0) {
        close(fd);
    }
}

int main(void) {
    // Detach into our own session so the podman-exec teardown that follows the
    // launcher's return doesn't take us down with it (like the feedback-buffer
    // drivers).
    setsid();

    // Phase 1: shallow coverage baseline, then announce it.
    volatile long sink = stage0(64);
    touch(READY_PATH);

    // Phase 2: wait for the test to release more work, then run the deeper
    // stages on the same accumulating buffer.
    while (access(MORE_PATH, F_OK) != 0) {
        usleep(1000);
    }
    sink += stage1(64);
    sink += stage2(64);
    sink += stage3(64);
    (void)sink;

    // Idle forever: libpcguard registered the buffer at load and the stages
    // filled in the hit counters, so the host can read it back.
    for (;;) {
        pause();
    }
}
