/* SPDX-License-Identifier: GPL-2.0 */
/*
 * Concurrency fuzzing scheduler, in-kernel BPF version.
 *
 * A sched_ext scheduler that deliberately stops and starts the threads of a
 * target process at random, to manufacture the rare interleavings that surface
 * concurrency bugs. The whole run/sleep decision runs here in the kernel, so
 * that under a deterministic hypervisor (bedrock: single vCPU + emulated TSC) a
 * run is fully reproducible from a seed.
 *
 * Behavior:
 *   - A task that belongs to the fuzzed process alternates between a "running"
 *     window (a random length from the run range) during which it competes for
 *     the CPU normally, and a "sleeping" window (from the sleep range) during
 *     which it is held off the CPU entirely.
 *   - Tasks unrelated to the target run normally with a fixed system slice.
 *   - Randomness comes from a seeded xorshift PRNG (NOT bpf_get_prandom_u32,
 *     which is not reproducible), so the same seed yields the same schedule.
 *
 * Mechanism: every queued task lives in one shared DSQ ordered by the time at
 * which it next becomes eligible to run (its vtime). A "running" task is
 * inserted eligible-now; a "sleeping" task is inserted eligible-at-wakeup, so
 * it sits in the queue but dispatch refuses to run it until its time arrives.
 * A periodic timer kicks the CPUs so dispatch re-checks eligibility even when
 * the system would otherwise idle.
 */
#include <scx/common.bpf.h>
#include "intf.h"

char _license[] SEC("license") = "GPL";

/* The single shared dispatch queue, ordered by per-task eligibility time. */
#define SHARED_DSQ_ID 0

/* How often the timer wakes the CPUs to re-check eligibility. 1ms; the fuzzer
 * injects ms-scale delays, so this jitter is irrelevant. */
#define TICK_NS 1000000ULL

/* Bound for the kick loop in the timer callback. */
#define MAX_CPUS 1024

/*
 * Read-only configuration, set by the loader before the program is loaded.
 * "const volatile" is how sched_ext schedulers expose rodata to user space.
 */
const volatile u64 run_min_ns;
const volatile u64 run_max_ns;
const volatile u64 sleep_min_ns;
const volatile u64 sleep_max_ns;
const volatile u64 slice_ns;
const volatile u64 system_slice_ns;
const volatile bool scale_slice;
const volatile bool logging;
const volatile u64 seed;

/*
 * Global PRNG state. Seeded from "seed" in init. A single global stream is
 * reproducible on a single CPU (where the enqueue order is itself
 * deterministic); on a multi-CPU host concurrent enqueues race on it, which is
 * irrelevant on the single-vCPU bedrock target.
 */
static u64 rng_state;

enum task_state {
	TASK_START = 0,
	TASK_RUNNING = 1,
	TASK_SLEEPING = 2,
};

/* Per-task fuzzing state. "deadline" is when the current state ends. "gen" is
 * the generation this state belongs to, for the reset-on-new-iteration rule. */
struct task_ctx {
	u64 state;
	u64 deadline;
	u32 gen;
};

struct {
	__uint(type, BPF_MAP_TYPE_TASK_STORAGE);
	__uint(map_flags, BPF_F_NO_PREALLOC);
	__type(key, int);
	__type(value, struct task_ctx);
} task_ctx_stor SEC(".maps");

/* Single-entry config map written by the loader while attached. */
struct {
	__uint(type, BPF_MAP_TYPE_ARRAY);
	__uint(max_entries, 1);
	__type(key, u32);
	__type(value, struct fuzz_config);
} config_map SEC(".maps");

static __always_inline struct fuzz_config *get_config(void)
{
	u32 zero = 0;

	return bpf_map_lookup_elem(&config_map, &zero);
}

/* Wrapper so the timer can live in an array map (bpf_timer needs map storage). */
struct timer_wrap {
	struct bpf_timer timer;
};

struct {
	__uint(type, BPF_MAP_TYPE_ARRAY);
	__uint(max_entries, 1);
	__type(key, u32);
	__type(value, struct timer_wrap);
} timer_map SEC(".maps");

/* Ring buffer carrying fuzz_event records up to the loader. 256 KiB. */
struct {
	__uint(type, BPF_MAP_TYPE_RINGBUF);
	__uint(max_entries, 256 * 1024);
} events SEC(".maps");

/* xorshift64. */
static __always_inline u64 rng_next(void)
{
	u64 x = rng_state;

	x ^= x << 13;
	x ^= x >> 7;
	x ^= x << 17;
	rng_state = x;
	return x;
}

/* Random value in the half-open range [min, max). */
static __always_inline u64 rng_range(u64 min, u64 max)
{
	u64 r;

	if (min == max)
		return min;
	r = (u32)rng_next();
	return min + (r * 31) % (max - min);
}

/*
 * Whether a task belongs to the fuzzed process. Read task_struct directly via
 * CO-RE. A task is related when its thread group is the target, or its parent's
 * thread group is (covering both the "ppid is the target" and "parent's tgid is
 * the target" cases).
 */
static __always_inline bool is_related(struct task_struct *p, u32 target_tgid)
{
	struct task_struct *parent;

	if (target_tgid == 0)
		return false;
	if (BPF_CORE_READ(p, tgid) == (pid_t)target_tgid)
		return true;
	parent = BPF_CORE_READ(p, real_parent);
	if (parent && BPF_CORE_READ(parent, tgid) == (pid_t)target_tgid)
		return true;
	return false;
}

static __always_inline void log_event(struct task_struct *p, u32 type, u64 now,
				       u64 duration_ns)
{
	struct fuzz_event *e;

	if (!logging)
		return;
	e = bpf_ringbuf_reserve(&events, sizeof(*e), 0);
	if (!e)
		return;
	e->time_ns = now;
	e->duration_ns = duration_ns;
	e->pid = BPF_CORE_READ(p, pid);
	e->event_type = type;
	bpf_probe_read_kernel_str(e->comm, sizeof(e->comm), BPF_CORE_READ(p, comm));
	bpf_ringbuf_submit(e, 0);
}

static __always_inline void begin_running(struct task_struct *p,
					  struct task_ctx *c, u64 now)
{
	u64 dur = rng_range(run_min_ns, run_max_ns);

	c->state = TASK_RUNNING;
	c->deadline = now + dur;
	log_event(p, FUZZ_EVENT_RUNNING, now, dur);
}

static __always_inline void begin_sleeping(struct task_struct *p,
					   struct task_ctx *c, u64 now)
{
	u64 dur = rng_range(sleep_min_ns, sleep_max_ns);

	c->state = TASK_SLEEPING;
	c->deadline = now + dur;
	log_event(p, FUZZ_EVENT_SLEEPING, now, dur);
}

/*
 * Advance the run/sleep state machine and return the time at which the task is
 * next eligible to run (now for a running task, the wakeup time for a sleeping
 * one).
 */
static __always_inline u64 advance_state(struct task_struct *p,
					 struct task_ctx *c, u64 now,
					 u32 generation)
{
	if (c->gen != generation) {
		c->gen = generation;
		c->state = TASK_START;
	}

	switch (c->state) {
	case TASK_START:
		/* First sight: flip a coin between starting to run or sleep. */
		if (rng_range(0, 2) == 0) {
			begin_sleeping(p, c, now);
			return c->deadline;
		}
		begin_running(p, c, now);
		return now;
	case TASK_RUNNING:
		if (now >= c->deadline) {
			begin_sleeping(p, c, now);
			return c->deadline;
		}
		return now;
	case TASK_SLEEPING:
	default:
		if (now >= c->deadline) {
			begin_running(p, c, now);
			return now;
		}
		return c->deadline;
	}
}

static __always_inline u64 pick_slice(bool related)
{
	u64 slice = related ? slice_ns : system_slice_ns;

	if (scale_slice) {
		/* Shrink the slice as the run queue grows. */
		s32 nr = scx_bpf_dsq_nr_queued(SHARED_DSQ_ID);

		if (nr > 0)
			slice /= (u64)nr + 1;
		if (slice == 0)
			slice = 1;
	}
	return slice;
}

s32 BPF_STRUCT_OPS(fifo_fuzz_select_cpu, struct task_struct *p, s32 prev_cpu,
		   u64 wake_flags)
{
	/*
	 * Do not direct-dispatch here. Returning prev_cpu without inserting the
	 * task forces every wakeup through enqueue(), so the run/sleep policy
	 * sees it.
	 */
	return prev_cpu;
}

void BPF_STRUCT_OPS(fifo_fuzz_enqueue, struct task_struct *p, u64 enq_flags)
{
	u64 now = bpf_ktime_get_ns();
	struct fuzz_config *cfg = get_config();
	u32 target_tgid = cfg ? cfg->target_tgid : 0;
	u32 generation = cfg ? cfg->generation : 0;
	bool related = is_related(p, target_tgid);
	u64 slice = pick_slice(related);
	u64 vtime;

	if (related) {
		struct task_ctx *c;

		c = bpf_task_storage_get(&task_ctx_stor, p, 0,
					 BPF_LOCAL_STORAGE_GET_F_CREATE);
		if (c)
			vtime = advance_state(p, c, now, generation);
		else
			vtime = now; /* out of storage: just run it */
	} else {
		vtime = now;
	}

	/*
	 * Everything goes into one DSQ ordered by eligibility time. A sleeping
	 * task is inserted with a future vtime, so it stays queued (in the
	 * kernel's custody, not stalled) but dispatch will not run it until its
	 * time comes.
	 */
	scx_bpf_dsq_insert_vtime(p, SHARED_DSQ_ID, slice, vtime, enq_flags);
}

void BPF_STRUCT_OPS(fifo_fuzz_dispatch, s32 cpu, struct task_struct *prev)
{
	u64 now = bpf_ktime_get_ns();
	struct task_struct *p;
	u64 head_vtime = 0;
	bool have_head = false;

	/*
	 * Peek the head. The DSQ is vtime-ordered, so the head has the earliest
	 * eligibility time; if it is not yet eligible, nothing else is either.
	 */
	bpf_for_each(scx_dsq, p, SHARED_DSQ_ID, 0) {
		head_vtime = p->scx.dsq_vtime;
		have_head = true;
		break;
	}

	if (have_head && head_vtime <= now)
		scx_bpf_dsq_move_to_local(SHARED_DSQ_ID);
	/* else: leave it; the timer will kick us to re-check at the next tick. */
}

static int timer_cb(void *map, int *key, struct timer_wrap *tw)
{
	u32 nr = scx_bpf_nr_cpu_ids();
	u32 i;

	/*
	 * Kick the CPUs so dispatch runs again and can release tasks whose sleep
	 * has just elapsed, even if the system would otherwise idle. On the
	 * single-vCPU target this is just CPU 0; the bounded loop keeps it
	 * correct on multi-CPU hosts too and keeps the verifier happy.
	 */
	for (i = 0; i < MAX_CPUS; i++) {
		if (i >= nr)
			break;
		scx_bpf_kick_cpu(i, 0);
	}

	bpf_timer_start(&tw->timer, TICK_NS, 0);
	return 0;
}

s32 BPF_STRUCT_OPS_SLEEPABLE(fifo_fuzz_init)
{
	struct timer_wrap *tw;
	u32 zero = 0;
	s32 ret;

	ret = scx_bpf_create_dsq(SHARED_DSQ_ID, -1);
	if (ret)
		return ret;

	/* Seed the PRNG. "| 1" avoids the all-zero xorshift fixed point. */
	rng_state = seed | 1ULL;

	tw = bpf_map_lookup_elem(&timer_map, &zero);
	if (!tw)
		return -1;
	bpf_timer_init(&tw->timer, &timer_map, CLOCK_MONOTONIC);
	bpf_timer_set_callback(&tw->timer, timer_cb);
	bpf_timer_start(&tw->timer, TICK_NS, 0);

	return 0;
}

SEC(".struct_ops.link")
struct sched_ext_ops fifo_fuzz_ops = {
	.select_cpu = (void *)fifo_fuzz_select_cpu,
	.enqueue    = (void *)fifo_fuzz_enqueue,
	.dispatch   = (void *)fifo_fuzz_dispatch,
	.init       = (void *)fifo_fuzz_init,
	/* SCX_OPS_ENQ_LAST: keep getting enqueue() for the last runnable task so
	 * a lone target thread still cycles through the state machine. */
	.flags      = SCX_OPS_ENQ_LAST,
	.timeout_ms = 5000,
	.name       = "fifo_fuzz",
};
