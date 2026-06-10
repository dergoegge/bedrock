// SPDX-License-Identifier: GPL-2.0
// Bedrock PEBS scratch page registration.
//
// Allocates one 4KB page, mlocks it so it stays resident at a stable guest
// physical address, and issues VMCALL with RAX=3 (HYPERCALL_REGISTER_PEBS_PAGE)
// passing the page's guest virtual address in RBX. The hypervisor walks the
// guest's page tables to translate to a GPA, populates the DS Management
// Area at that page, and remaps it R+E in EPT so subsequent PEBS record
// writes trap as precise EPT-violation VM exits. The page is never actually
// written to; we just need it to be mapped and writable in guest paging so
// the EPT layer is what produces the trap.
//
// After registration the program forks and the child sleeps forever to keep
// the mmap'd page pinned. If it ever exits the kernel reclaims the page and
// could re-allocate the underlying host-physical frame for some other use —
// which would then take a spurious (non-PEBS) EPT write trap when the
// guest kernel touches it, breaking the dispatch.

#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#define HYPERCALL_REGISTER_PEBS_PAGE 3
#define PAGE_SIZE 4096

static uint64_t register_pebs_page(void *page) {
    uint64_t result;
    __asm__ volatile(
        "mov $3, %%rax\n\t"   // HYPERCALL_REGISTER_PEBS_PAGE = 3
        "mov %1, %%rbx\n\t"   // RBX = page virtual address
        "vmcall\n\t"
        "mov %%rax, %0\n\t"
        : "=r"(result)
        : "r"((uint64_t)page)
        : "rax", "rbx"
    );
    return result;
}

int main(void) {
    void *page = mmap(NULL, PAGE_SIZE, PROT_READ | PROT_WRITE,
                      MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (page == MAP_FAILED) {
        perror("mmap");
        return 1;
    }

    // Touch the page so the kernel actually allocates a backing physical
    // frame and installs the PTE — without this the GVA→GPA walk in the
    // hypervisor would fault.
    memset(page, 0, PAGE_SIZE);

    // Pin the page so it can't be reclaimed / migrated to a different GPA.
    if (mlock(page, PAGE_SIZE) != 0) {
        perror("mlock");
        return 1;
    }

    printf("Registering PEBS scratch page at %p...\n", page);
    uint64_t result = register_pebs_page(page);
    if (result == 0) {
        printf("PEBS scratch page registered successfully; pinning page\n");
        // Sleep forever to keep the mmap'd page pinned for the lifetime of
        // the guest. If we exited, the kernel would reclaim the page and
        // potentially re-allocate the underlying host-physical frame for
        // something else — and any write to it would then take an EPT
        // trap that's not PEBS-induced, confusing the dispatcher. Run this
        // program in the background from init so it stays alive.
        for (;;) {
            pause();
        }
    }

    // Failure codes mirror RegisterPebsPageResult in
    // crates/bedrock-vmx/src/exits/pebs.rs. Decode them for diagnostics.
    fprintf(stderr, "PEBS registration failed (rax=0x%lx): ", result);
    if (result == UINT64_MAX) {
        fprintf(stderr, "host CPU does not support EPT-friendly PEBS "
                        "(IA32_PERF_CAPABILITIES.PEBS_BASELINE clear, "
                        "or running under a hypervisor that doesn't expose "
                        "PEBS to nested guests — common with KVM L1)\n");
    } else if (result == UINT64_MAX - 1) {
        fprintf(stderr, "page address not 4KB-aligned\n");
    } else if (result == UINT64_MAX - 2) {
        fprintf(stderr, "guest page-table walk failed\n");
    } else if (result == UINT64_MAX - 3) {
        fprintf(stderr, "EPT mapping missing — page not faulted in?\n");
    } else if (result == UINT64_MAX - 4) {
        fprintf(stderr, "PEBS page already registered\n");
    } else {
        fprintf(stderr, "unknown error (not running in bedrock VM?)\n");
    }
    return 1;
}
