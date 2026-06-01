// SPDX-License-Identifier: GPL-2.0
// Issues bedrock's HYPERCALL_READY (7) VMCALL and exits. Shell can't
// emit inline asm, so workload.sh execs this helper once setup is done.

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
