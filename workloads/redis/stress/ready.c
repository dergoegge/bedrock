// SPDX-License-Identifier: GPL-2.0
// Issues bedrock's HYPERCALL_READY (7) VMCALL and exits. Python can't emit
// inline asm, so stress.py execs this helper once the server is reachable.

int main(void) {
    __asm__ volatile(
        "mov $7, %%rax\n\t"
        "vmcall\n\t"
        :
        :
        : "rax"
    );
    return 0;
}
