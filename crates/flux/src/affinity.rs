// SPDX-License-Identifier: GPL-2.0

//! Minimal CPU-affinity helpers.
//!
//! Workers are CPU-bound on VM execution, so pinning each to a distinct core
//! keeps the VM's per-CPU state warm and avoids the scheduler bouncing them.

/// Number of online CPUs (at least 1).
pub fn core_count() -> usize {
    let n = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    if n < 1 {
        1
    } else {
        n as usize
    }
}

/// Pin the calling thread to `core` (taken modulo the online CPU count).
/// Best-effort: a failure is silently ignored (pinning is an optimization).
pub fn pin_to_core(core: usize) {
    let n = core_count();
    let core = core % n;
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(core, &mut set);
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
    }
}
