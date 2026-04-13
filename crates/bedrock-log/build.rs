// SPDX-License-Identifier: GPL-2.0

fn main() {
    // Declare kernel_log as a known cfg so Cargo doesn't warn about it.
    // This flag is set by the kernel build system (KBUILD_RUSTFLAGS += --cfg kernel_log)
    // to enable pr_* logging in kernel module builds.
    println!("cargo:rustc-check-cfg=cfg(kernel_log)");
}
