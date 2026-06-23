/* SPDX-License-Identifier: GPL-2.0 */
/*
 * Structures shared between the in-kernel BPF scheduler (main.bpf.c) and the
 * user-space loader (loader.c). The loader includes this header directly, so
 * it has to be valid both as BPF C (where the fixed width types come from
 * vmlinux.h) and as plain C for the loader (where we define them ourselves
 * below).
 */
#ifndef __INTF_H
#define __INTF_H

/*
 * vmlinux.h (included by the BPF program via scx/common.bpf.h) already defines
 * these. When this header is parsed on its own, e.g. by the user-space loader,
 * vmlinux.h is absent, so define them here.
 */
#ifndef __VMLINUX_H__
typedef unsigned char u8;
typedef unsigned short u16;
typedef unsigned int u32;
typedef unsigned long long u64;
#endif /* __VMLINUX_H__ */

/* Task comm is TASK_COMM_LEN (16) in the kernel. */
#define FUZZ_COMM_LEN 16

enum fuzz_event_type {
	FUZZ_EVENT_RUNNING = 0,
	FUZZ_EVENT_SLEEPING = 1,
};

/*
 * Runtime configuration, written by the loader while the scheduler is attached
 * (single element of an array map). target_tgid is the pid of the process
 * currently being fuzzed, 0 means none. generation is bumped on every new
 * iteration so per-task state is dropped, since pids are reused across the
 * restarts of the target.
 */
struct fuzz_config {
	u32 target_tgid;
	u32 generation;
};

/*
 * One run/sleep state transition, pushed to user space over a ring buffer so
 * the loader can print the log lines. This is purely diagnostic: the
 * scheduling decision itself never leaves the kernel.
 */
struct fuzz_event {
	u64 time_ns;	  /* bpf_ktime_get_ns() at the transition */
	u64 duration_ns;  /* how long the new state lasts */
	u32 pid;
	u32 event_type;	  /* enum fuzz_event_type */
	char comm[FUZZ_COMM_LEN];
};

#endif /* __INTF_H */
