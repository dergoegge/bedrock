// SPDX-License-Identifier: GPL-2.0
//
// bedrock libvoidstar: the native coverage backend the Antithesis Go SDK
// loads from /usr/lib/libvoidstar.so when a binary built with the
// antithesis-go-instrumentor runs under cgo. We implement the five symbols
// the SDK's voidstar_handler resolves (init_coverage_module, notify_coverage,
// and the three fuzz_* hooks) and turn edge coverage into a bedrock feedback
// buffer.
//
// On init_coverage_module() we mmap a byte-per-edge hitcount map and register
// it once with the hypervisor via HYPERCALL_REGISTER_FEEDBACK_BUFFER, tagged
// with the instrumentor's symbol-table name. From then on notify_coverage()
// just bumps the edge's saturating hit counter in that shared buffer — no
// further hypercalls — and the host (delorean) reads the buffer back by id,
// classifying the raw counts into AFL-style hitcount buckets to drive
// coverage-guided fuzzing.
//
// The map is backed by a named file on a persistent tmpfs (COVERAGE_DIR, a
// host-namespace mount bind-mounted into every container) rather than anonymous
// memory, so its physical pages are owned by the file's inode — not this
// process. When the instrumented process dies (cleanly or by a crash), and even
// when its container is torn down, the pages survive. The hypervisor captured
// their guest-physical addresses at registration and keeps reading them, so it
// sees a valid, frozen bitmap instead of memory the guest has freed and reused
// (which reads back as a flood of bogus "new" coverage edges).
//
// The three fuzz_* entrypoints are part of the SDK ABI (the loader panics if
// any symbol is missing) but only matter for Antithesis' own
// assertion/randomness fuzzing, which bedrock does not use. They are stubs.

#include <fcntl.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

// HYPERCALL_REGISTER_FEEDBACK_BUFFER, from crates/bedrock-vmx/src/hypercalls.rs.
#define HYPERCALL_REGISTER_FEEDBACK_BUFFER 2

// Mirrors FEEDBACK_BUFFER_ID_MAX_LEN (crates/bedrock-vmx) — ids longer than
// this are rejected by the host, so we truncate.
#define FEEDBACK_BUFFER_ID_MAX_LEN 128

// Mirrors MAX_FEEDBACK_BUFFER_SIZE (FEEDBACK_BUFFER_MAX_PAGES * 4096 = 1 MiB).
// One byte per edge, so this also caps us at ~1M distinct edges; beyond that
// edges alias modulo the buffer size (btcd is expected to have only a few
// thousand, so aliasing should never bite in practice).
#define PAGE_SIZE 4096UL
#define MAX_COVERAGE_BYTES (256UL * PAGE_SIZE)

// Persistent tmpfs directory (a host-namespace mount bind-mounted into every
// container; see nix/podman-initrd.nix) where each instrumented process keeps
// its coverage bitmap as a named file, so the pages outlive the process and the
// container. See the file header for why.
#define COVERAGE_DIR "/bedrock/coverage"

static uint8_t *coverage_buffer = NULL;
static size_t coverage_size = 0;

// Issue HYPERCALL_REGISTER_FEEDBACK_BUFFER. Register-pinned operands avoid any
// interaction with -fPIC register allocation (rbx is callee-saved, not the PIC
// base, on x86-64). Returns the assigned slot index, or (unsigned long)-1.
static unsigned long bedrock_register_feedback_buffer(const void *buf,
                                                      unsigned long size,
                                                      const char *id,
                                                      unsigned long id_len) {
    register unsigned long rax __asm__("rax") = HYPERCALL_REGISTER_FEEDBACK_BUFFER;
    register unsigned long rbx __asm__("rbx") = (unsigned long)buf;
    register unsigned long rcx __asm__("rcx") = size;
    register unsigned long rdx __asm__("rdx") = (unsigned long)id;
    register unsigned long rsi __asm__("rsi") = id_len;

    __asm__ volatile("vmcall"
                     : "+r"(rax)
                     : "r"(rbx), "r"(rcx), "r"(rdx), "r"(rsi)
                     : "memory");
    return rax;
}

// Append `n` bytes of `src` into `path` at `*p`, mapping anything outside a
// safe filename charset to '_', bounded by `cap`.
static void append_sanitized(char *path, size_t *p, size_t cap, const char *src,
                             size_t n) {
    for (size_t i = 0; i < n && *p < cap - 1; i++) {
        char c = src[i];
        int ok = (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') ||
                 (c >= '0' && c <= '9') || c == '.' || c == '-' || c == '_';
        path[(*p)++] = ok ? c : '_';
    }
}

// Map the coverage buffer onto a persistent tmpfs file (MAP_SHARED), so its
// pages survive this process and its container (see the file header). The file
// name is the buffer id plus the container hostname — both deterministic under
// bedrock and distinct per container — so each instrumented process gets its
// own stable file. Falls back to an anonymous mapping if the tmpfs path is
// unavailable (e.g. the dir isn't mounted), preserving coverage at the cost of
// the survive-death property.
static void *map_coverage_buffer(size_t size, const char *id, size_t id_len) {
    char host[64];
    if (gethostname(host, sizeof(host)) != 0) {
        host[0] = '\0';
    }
    host[sizeof(host) - 1] = '\0';

    char path[256];
    int n = snprintf(path, sizeof(path), "%s/", COVERAGE_DIR);
    if (n > 0 && (size_t)n < sizeof(path)) {
        size_t p = (size_t)n;
        append_sanitized(path, &p, sizeof(path), id, id_len);
        if (p < sizeof(path) - 1) {
            path[p++] = '-';
        }
        append_sanitized(path, &p, sizeof(path), host, strlen(host));
        path[p] = '\0';

        int fd = open(path, O_CREAT | O_RDWR, 0600);
        if (fd >= 0) {
            int truncated = ftruncate(fd, (off_t)size) == 0;
            if (truncated) {
                void *buf =
                    mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
                close(fd);
                if (buf != MAP_FAILED) {
                    return buf;
                }
            } else {
                close(fd);
            }
        }
    }

    void *buf = mmap(NULL, size, PROT_READ | PROT_WRITE,
                     MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    return buf == MAP_FAILED ? NULL : buf;
}

// Called once at program start by the generated notifier's init(). num_edges
// is the instrumentor's edge count; symbols is the symbol-table name, which we
// reuse verbatim as the feedback-buffer id so delorean can key coverage on it.
// Returns the edge offset the SDK adds to every edge before notify; we keep a
// single module based at 0.
uint64_t init_coverage_module(size_t num_edges, const char *symbols) {
    if (coverage_buffer != NULL) {
        return 0;
    }

    size_t edges = num_edges ? num_edges : 1;
    size_t size = (edges + PAGE_SIZE - 1) & ~(PAGE_SIZE - 1);
    if (size > MAX_COVERAGE_BYTES) {
        size = MAX_COVERAGE_BYTES;
    }

    size_t id_len = symbols ? strlen(symbols) : 0;
    if (id_len > FEEDBACK_BUFFER_ID_MAX_LEN) {
        id_len = FEEDBACK_BUFFER_ID_MAX_LEN;
    }

    void *buf = map_coverage_buffer(size, symbols ? symbols : "", id_len);
    if (buf == NULL) {
        return 0;
    }
    memset(buf, 0, size);

    coverage_buffer = (uint8_t *)buf;
    coverage_size = size;

    if (id_len > 0) {
        bedrock_register_feedback_buffer(coverage_buffer, coverage_size,
                                         symbols, id_len);
    }

    return 0;
}

// Called for every edge the instrumented code hits. We saturating-increment
// the edge's hit counter and return true. The return value is the SDK's
// "must call again?" flag: returning false makes the SDK mark the edge visited
// and never report it again (binary coverage), while returning true keeps it
// reporting every hit — which is what lets the counter track how many times
// the edge ran. delorean buckets these raw counts AFL-style. We saturate at
// 0xff so a hot edge never wraps back to 0 (which would read as "never run").
_Bool notify_coverage(size_t edge) {
    if (coverage_buffer == NULL) {
        return 1;
    }
    uint8_t *cell = &coverage_buffer[edge % coverage_size];
    if (*cell != 0xff) {
        (*cell)++;
    }
    return 1;
}

// Antithesis assertion/randomness hooks — unused by bedrock, but the SDK
// loader requires all five symbols to be present.
void fuzz_json_data(const char *data, size_t size) {
    (void)data;
    (void)size;
}

void fuzz_flush(void) {}

uint64_t fuzz_get_random(void) {
    return 0;
}
