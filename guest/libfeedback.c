// SPDX-License-Identifier: GPL-2.0
//
// bedrock libfeedback: the generic coverage-feedback backend shared by every
// coverage frontend. It owns the process-wide hitcount buffer — mapping it,
// registering it once with the hypervisor over HYPERCALL_REGISTER_FEEDBACK_BUFFER
// (via libvmcall), and bumping saturating per-edge counters that the host reads
// back to drive coverage-guided fuzzing. Frontends (libvoidstar.c, libpcguard.c)
// just call feedback_record().
//
// The buffer id is "cov-<build>", where <build> uniquely identifies one build
// of the binary, so coverage is scoped to that build: a rebuilt binary
// (different edge/guard layout) gets a distinct id and never shares a map with
// an incompatible one. The frontend supplies <build>: libvoidstar passes the Go
// instrumentor's symbol-table name; libpcguard passes NULL, and we fall back to
// the binary's GNU build-id note (so C/C++/LLVM targets must be linked with
// -Wl,--build-id). With neither available the id degrades to just "cov".
//
// The buffer is backed by a file on a persistent tmpfs (COVERAGE_DIR) named
// "<hostname>-<id>" rather than anonymous memory, so its pages are owned by the
// file's inode and outlive the process and its container. The hypervisor
// captured their guest-physical addresses at registration and keeps reading
// them, so it sees a frozen bitmap instead of memory the guest has since freed
// and reused.

#define _GNU_SOURCE // dl_iterate_phdr / ElfW

#include <elf.h>
#include <fcntl.h>
#include <link.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include "libfeedback.h"
#include "libvmcall.h"

// One byte per edge, so the host's 1 MiB buffer cap also caps us at ~1M edges;
// beyond that edges alias modulo the buffer size. PAGE_SIZE is the mapping's
// rounding granularity.
#define PAGE_SIZE 4096UL
#define MAX_COVERAGE_BYTES ((size_t)VMCALL_FEEDBACK_BUFFER_MAX_SIZE)

// Persistent tmpfs (bind-mounted into every container; see nix/podman-initrd.nix)
// where each process keeps its bitmap file so the pages outlive it.
#define COVERAGE_DIR "/bedrock/coverage"

static uint8_t *coverage_buffer = NULL;
static size_t coverage_size = 0;

// Guards the one-time map+register so feedback_buffer_init() is idempotent and
// thread-safe: the first caller wins the CAS and inits, the rest observe the
// published result. feedback_record()'s plain reads stay correct against the
// winner's plain stores under x86's store/load ordering.
static int init_started = 0;

// dl_iterate_phdr callback: write the main object's GNU build-id as lowercase
// hex into `data` (a char[2 * 32 + 1]), then stop at the first object (the main
// executable, whose build-id is the one we want).
static int build_id_cb(struct dl_phdr_info *info, size_t size, void *data) {
    (void)size;
    char *out = data;
    for (int i = 0; i < info->dlpi_phnum; i++) {
        const ElfW(Phdr) *ph = &info->dlpi_phdr[i];
        if (ph->p_type != PT_NOTE) {
            continue;
        }
        const unsigned char *p = (const unsigned char *)(info->dlpi_addr + ph->p_vaddr);
        const unsigned char *end = p + ph->p_memsz;
        while (p + sizeof(ElfW(Nhdr)) <= end) {
            const ElfW(Nhdr) *n = (const ElfW(Nhdr) *)p;
            const unsigned char *name = p + sizeof(*n);
            const unsigned char *desc = name + ((n->n_namesz + 3) & ~3u);
            if (n->n_type == NT_GNU_BUILD_ID && n->n_namesz == 4 &&
                memcmp(name, "GNU", 4) == 0) {
                unsigned len = n->n_descsz < 32 ? n->n_descsz : 32;
                for (unsigned k = 0; k < len; k++) {
                    sprintf(out + 2 * k, "%02x", desc[k]);
                }
                return 1;
            }
            p = desc + ((n->n_descsz + 3) & ~3u);
        }
    }
    return 1; // main object only
}

// Build the coverage buffer id "cov-<build>" into `out` (capped at `cap`),
// returning its length. `build` is the frontend's build identifier (e.g. the Go
// instrumentor's symbol-table name); when it is NULL/empty we fall back to the
// binary's GNU build-id note. With neither, the id degrades to just "cov".
static size_t coverage_id(char *out, size_t cap, const char *build) {
    char gnu[2 * 32 + 1] = "";
    if (!(build && build[0])) {
        dl_iterate_phdr(build_id_cb, gnu); // may leave gnu == ""
        build = gnu;
    }
    int n = build[0] ? snprintf(out, cap, "cov-%s", build) : snprintf(out, cap, "cov");
    return (n < 0) ? 0 : ((size_t)n < cap ? (size_t)n : cap - 1);
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

// Map the buffer onto a per-build tmpfs file (MAP_SHARED) named
// "<hostname>-<id>", so its pages survive the process and its container; the
// hostname is distinct per container and the id is distinct per build. Falls
// back to an anonymous mapping if the tmpfs path is unavailable, losing only the
// survive-death property.
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
        append_sanitized(path, &p, sizeof(path), host, strlen(host));
        if (p < sizeof(path) - 1) {
            path[p++] = '-';
        }
        append_sanitized(path, &p, sizeof(path), id, id_len);
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

uint8_t *feedback_buffer_init(size_t num_edges, const char *build_id) {
    // Only the winner maps and registers; others get the published buffer (or,
    // briefly, NULL mid-init, making feedback_record() a no-op for a few edges).
    int expected = 0;
    if (!__atomic_compare_exchange_n(&init_started, &expected, 1, 0,
                                     __ATOMIC_ACQ_REL, __ATOMIC_RELAXED)) {
        return coverage_buffer;
    }

    // "cov-<build>", staged on the (resident) stack: the host reads the id by
    // walking the guest page tables and can't fault a not-present page in.
    char id[VMCALL_FEEDBACK_BUFFER_ID_MAX_LEN];
    size_t id_len = coverage_id(id, sizeof(id), build_id);

    size_t edges = num_edges ? num_edges : 1;
    size_t size = (edges + PAGE_SIZE - 1) & ~(PAGE_SIZE - 1);
    if (size > MAX_COVERAGE_BYTES) {
        size = MAX_COVERAGE_BYTES;
    }

    void *buf = map_coverage_buffer(size, id, id_len);
    if (buf != NULL) {
        memset(buf, 0, size);
        // Publish size before the pointer: a reader that sees the pointer sees
        // the size too (x86 store ordering).
        coverage_size = size;
        coverage_buffer = (uint8_t *)buf;
        vmcall_register_feedback_buffer(coverage_buffer, coverage_size, id,
                                        id_len);
    }

    return coverage_buffer;
}

void feedback_record(uint64_t edge) {
    uint8_t *buf = coverage_buffer;
    if (buf == NULL) {
        return;
    }
    uint8_t *cell = &buf[edge % coverage_size];
    if (*cell != 0xff) {
        (*cell)++;
    }
}

uint8_t *feedback_buffer(void) {
    return coverage_buffer;
}

size_t feedback_buffer_size(void) {
    return coverage_size;
}
