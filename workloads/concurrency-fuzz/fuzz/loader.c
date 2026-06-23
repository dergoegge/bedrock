// SPDX-License-Identifier: GPL-2.0
//
// Static C loader + supervisor for the in-kernel concurrency-fuzz scheduler.
//
// This code:
//   - opens the BPF skeleton, writes the read-only config (run/sleep ranges,
//     slices, seed) into rodata, loads it, and attaches the sched_ext
//     struct_ops;
//   - fork/execs the target and writes the target's (global) tgid + generation
//     into the config map so the kernel knows which process to fuzz;
//   - drains the ring buffer of run/sleep events for an (optional) log, and
//     supervises the target until it exits.
//
// Determinism: all timestamps printed come from the kernel (bpf_ktime_get_ns),
// which derives from bedrock's deterministic emulated TSC. No wall-clock or
// host randomness is read here.
//
// Usage: fuzz-loader <seed> <target> [target-args...]
//   <seed> is decimal or 0x-prefixed hex.
//
// Exit status:
//   0 - target crashed (non-zero exit or terminating signal); the goal of a
//       successful fuzz run.
//   1 - target exited cleanly; no crash found this run.
//   2 - loader/setup error.
// The caller (run.sh) issues the shutdown VMCALL afterwards regardless.

#include <errno.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/wait.h>

#include <bpf/libbpf.h>
#include <bpf/bpf.h>

#include "intf.h"
#include "fuzz_bpf.skel.h"

// Fixed fuzzing flags: the workload runs one fixed configuration and the only
// knob that needs to vary (for the determinism negative control) is the seed.
#define RUN_MIN_NS	(1ULL * 1000000)	// 1ms
#define RUN_MAX_NS	(100ULL * 1000000)	// 100ms
#define SLEEP_MIN_NS	(10ULL * 1000000)	// 10ms
#define SLEEP_MAX_NS	(2000ULL * 1000000)	// 2000ms
#define SLICE_NS	(5ULL * 1000000)	// 5ms
#define SYSTEM_SLICE_NS (5ULL * 1000000)	// 5ms

static int handle_event(void *ctx, void *data, size_t size)
{
	const struct fuzz_event *e = data;
	(void)ctx;

	if (size < sizeof(*e))
		return 0;

	unsigned long long sec = e->time_ns / 1000000000ULL;
	unsigned long long ms = (e->time_ns % 1000000000ULL) / 1000000ULL;
	const char *verb = e->event_type == FUZZ_EVENT_RUNNING ? "running"
							       : "sleeping";

	printf("[%6llu.%03llu] %s is %s for %llums\n", sec, ms, e->comm, verb,
	       e->duration_ns / 1000000ULL);
	fflush(stdout);
	return 0;
}

static int parse_seed(const char *s, uint64_t *out)
{
	char *end = NULL;
	int base = 10;

	if (s[0] == '0' && (s[1] == 'x' || s[1] == 'X')) {
		base = 16;
		s += 2;
	}
	errno = 0;
	unsigned long long v = strtoull(s, &end, base);
	if (errno != 0 || end == s || *end != '\0')
		return -1;
	*out = (uint64_t)v;
	return 0;
}

// Set the runtime config (target pid + generation) in the single-entry map.
static int write_config(struct fuzz_bpf *skel, uint32_t target_tgid,
			uint32_t generation)
{
	struct fuzz_config cfg = {
		.target_tgid = target_tgid,
		.generation = generation,
	};
	uint32_t key = 0;

	return bpf_map__update_elem(skel->maps.config_map, &key, sizeof(key),
				    &cfg, sizeof(cfg), BPF_ANY);
}

int main(int argc, char **argv)
{
	if (argc < 3) {
		fprintf(stderr, "usage: %s <seed> <target> [args...]\n",
			argv[0]);
		return 2;
	}

	uint64_t seed;
	if (parse_seed(argv[1], &seed) != 0) {
		fprintf(stderr, "invalid seed: %s\n", argv[1]);
		return 2;
	}
	char **target_argv = &argv[2];

	struct fuzz_bpf *skel = fuzz_bpf__open();
	if (!skel) {
		fprintf(stderr, "failed to open BPF skeleton\n");
		return 2;
	}

	// Read-only config must be set before load.
	skel->rodata->run_min_ns = RUN_MIN_NS;
	skel->rodata->run_max_ns = RUN_MAX_NS;
	skel->rodata->sleep_min_ns = SLEEP_MIN_NS;
	skel->rodata->sleep_max_ns = SLEEP_MAX_NS;
	skel->rodata->slice_ns = SLICE_NS;
	skel->rodata->system_slice_ns = SYSTEM_SLICE_NS;
	skel->rodata->scale_slice = true;
	skel->rodata->logging = true;
	skel->rodata->seed = seed;

	int err = fuzz_bpf__load(skel);
	if (err) {
		fprintf(stderr, "failed to load BPF skeleton: %d\n", err);
		goto cleanup_skel;
	}

	// Attaching the sched_ext struct_ops makes our policy the system
	// scheduler. Hold the link; dropping it detaches.
	struct bpf_link *link =
		bpf_map__attach_struct_ops(skel->maps.fifo_fuzz_ops);
	if (!link) {
		fprintf(stderr, "failed to attach sched_ext struct_ops: %d\n",
			-errno);
		err = 2;
		goto cleanup_skel;
	}

	struct ring_buffer *rb =
		ring_buffer__new(bpf_map__fd(skel->maps.events), handle_event,
				 NULL, NULL);
	if (!rb) {
		fprintf(stderr, "failed to create ring buffer\n");
		err = 2;
		goto cleanup_link;
	}

	printf("fuzzing %s with seed %#llx\n", target_argv[0],
	       (unsigned long long)seed);
	fflush(stdout);

	// One iteration: launch the target, fuzz it until it exits. The
	// container shares the host PID namespace (compose `pid: host`), so the
	// child's pid IS the global tgid that the BPF reads from task_struct.
	pid_t child = fork();
	if (child < 0) {
		perror("fork");
		err = 2;
		goto cleanup_rb;
	}
	if (child == 0) {
		execv(target_argv[0], target_argv);
		perror("execv");
		_exit(127);
	}

	if (write_config(skel, (uint32_t)child, 1) != 0) {
		fprintf(stderr, "failed to set target in config map\n");
		// Keep going; the supervisor still reaps the child.
	}

	bool crashed = false;
	for (;;) {
		// Drain run/sleep log events; 100ms poll bounds latency.
		ring_buffer__poll(rb, 100 /* ms */);

		int status;
		pid_t r = waitpid(child, &status, WNOHANG);
		if (r == child) {
			if (WIFEXITED(status) && WEXITSTATUS(status) != 0) {
				crashed = true;
				printf("target crashed: exit %d\n",
				       WEXITSTATUS(status));
			} else if (WIFSIGNALED(status)) {
				crashed = true;
				printf("target crashed: signal %d\n",
				       WTERMSIG(status));
			} else {
				printf("target exited cleanly\n");
			}
			break;
		}
		if (r < 0 && errno != EINTR) {
			perror("waitpid");
			break;
		}
	}
	fflush(stdout);

	// Stop fuzzing the (now dead) pid before detaching.
	write_config(skel, 0, 1);
	err = crashed ? 0 : 1;

cleanup_rb:
	ring_buffer__free(rb);
cleanup_link:
	bpf_link__destroy(link);
cleanup_skel:
	fuzz_bpf__destroy(skel);
	return err;
}
