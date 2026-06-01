// SPDX-License-Identifier: GPL-2.0

//! Boot a Linux guest until the ready hypercall fires, take that as a
//! checkpoint, and then drive random workload drivers off an
//! [`InputSource`] attached to a branch.
//!
//! Run with:
//!
//! ```text
//! cargo run -p bedrock-lab --example lab_random_drivers -- <vmlinux> <initramfs>
//! ```
//!
//! The guest is expected to load `bedrock-io.ko` and issue the ready
//! hypercall before the [`vt!`] boot deadline. After that, the example
//! queries the workload listing once (and re-checkpoints so the post-boot
//! discovery cost is paid only once), then forks a branch wired to a
//! [`RandomDriverSource`] that emits driver invocations with both the
//! pick and the inter-call delay drawn from independent seeded LCG
//! streams. Delays are uniform on `[0, --max-spacing-ms]`, so back-to-back
//! invocations at the same emulated TSC are possible — the lab serializes
//! them through the I/O channel one response at a time.

use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use bedrock_lab::{
    BashTarget, BranchId, Checkpoint, Event, EventSink, InputRecording, InputSource, IoInput,
    LabError, LabOpts, RngMode, RunOutcome, VirtDuration, VirtTime, WorkloadDriver,
};
use bedrock_vm::{boot::defaults, load_kernel, LinuxBootConfig, VmBuilder};
use clap::Parser;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

bedrock_lab::define_virt_time_macros!($, bedrock_vm::DEFAULT_TSC_FREQUENCY);

const MEMORY_MB: usize = 5120;
const BOOT_RNG_SEED: u64 = 0xbed0_0001;

/// Upper bound on the per-iteration input count. Each iteration's
/// actual length is drawn uniformly from
/// `[MAX_CALLS_PER_ITER / 2, MAX_CALLS_PER_ITER]`, derived from the
/// iteration seed — so every iteration drives at least half the max,
/// which keeps short-input runs from dominating the distribution and
/// missing bugs that only show up after a few actions have stacked
/// state. Baked into the binary so that `--replay-seed` is the full
/// repro key — changing this constant invalidates prior repro
/// instructions.
const MAX_CALLS_PER_ITER: u32 = 30;

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let args = Args::parse();

    let mut vm = VmBuilder::new().memory_mb(MEMORY_MB).build()?;
    let kernel = fs::read(&args.vmlinux)?;
    let initramfs = fs::read(&args.initramfs)?;

    let (kernel_entry, kernel_end) = {
        let memory = vm.memory_mut()?;
        load_kernel(memory, &kernel)?
    };

    let mut boot = LinuxBootConfig::new(kernel_entry, kernel_end).cmdline(defaults::CMDLINE);
    boot = boot.initramfs(&initramfs);
    vm.setup_linux_boot(&boot)?;

    let sink = Arc::new(FuzzerSink::new());
    let ready_cp = Checkpoint::initial_when_ready_with(
        vm,
        vt!(120 s),
        LabOpts {
            sink: sink.clone(),
            rng: RngMode::Seeded(BOOT_RNG_SEED),
            ..Default::default()
        },
    )?;
    println!(
        "ready checkpoint {:?} at {:.3}s",
        ready_cp.id(),
        ready_cp.time().as_secs_f64()
    );

    // Discover containers and their drivers once on a throwaway branch,
    // then re-checkpoint there so the input-driven branch starts from a
    // state where all responses have already drained.
    let mut discover = ready_cp.branch()?;
    let details = discover.workload_details()?;
    if details.containers.is_empty() {
        return Err("no containers found in workload listing".into());
    }
    println!(
        "discovered {} containers, {} drivers:",
        details.containers.len(),
        details.drivers.len()
    );
    for c in &details.containers {
        println!("  container: {c}");
    }
    for d in &details.drivers {
        println!("  driver:    {}:{}", d.container, d.driver);
    }

    // Augment the per-container drivers with workload-agnostic
    // fault-injection actions targeted at the host: a `partition` per
    // container plus a `clear` that removes any active partition. The
    // picker enforces a state machine — `clear` is only eligible when a
    // partition is currently active — so the sequence is always
    // partition...partition clear partition... never clear clear.
    let mut actions: Vec<DriverAction> = details
        .drivers
        .into_iter()
        .map(DriverAction::Container)
        .collect();
    let excluded: std::collections::BTreeSet<&str> = args
        .exclude_from_partition
        .iter()
        .map(String::as_str)
        .collect();
    // Warn loudly if any excluded name doesn't exist in the discovery
    // list — almost always a typo and silently dropping it would hide
    // the mistake.
    for name in &excluded {
        if !details.containers.iter().any(|c| c == name) {
            eprintln!(
                "warning: --exclude-from-partition {name:?} doesn't match any \
                 discovered container; ignoring"
            );
        }
    }
    for c in &details.containers {
        if excluded.contains(c.as_str()) {
            continue;
        }
        actions.push(DriverAction::FaultPartition(c.clone()));
    }
    actions.push(DriverAction::FaultClear);
    println!(
        "total actions (drivers + fault-injector): {}{}",
        actions.len(),
        if excluded.is_empty() {
            String::new()
        } else {
            format!(" (partition excludes: {})", excluded.iter().copied().collect::<Vec<_>>().join(","))
        }
    );

    let discovery_cp = discover.checkpoint()?;
    println!(
        "discovery checkpoint {:?} at {:.3}s",
        discovery_cp.id(),
        discovery_cp.time().as_secs_f64()
    );

    let freq = discovery_cp.tsc_frequency();
    let max_spacing = VirtDuration::from_millis(args.max_spacing_ms, freq);

    // Liveness check: compare the post-iteration container list against
    // the baseline captured at discovery. Any difference (a container
    // exited, a new one appeared, anything) exits non-zero and prints
    // the diff to the journal, which the SerialSink relays.
    let expected = details.containers.join(",");
    let check_cmd = format!(
        "running=$(podman ps --format '{{{{.Names}}}}' | sort | paste -sd, -); \
         if [ \"$running\" != \"{expected}\" ]; then \
             echo \"LIVENESS FAIL: expected={expected} running=$running\"; \
             exit 1; \
         fi"
    );

    // Replay mode: if --replay-seed is set, skip the LCG-driven loop
    // and just run that one iteration. `num_calls` is derived from the
    // seed (see `derive_num_calls`), so the seed alone is the full
    // repro key — pass it back through here and you get bit-identical
    // inputs and bit-identical liveness verdict.
    let iterations: Vec<u64> = if let Some(rseed) = args.replay_seed {
        println!(
            "REPLAY mode: seed={rseed:#018x} num_calls={}",
            derive_num_calls(rseed)
        );
        vec![rseed]
    } else {
        // One ChaCha8 stream seeded by the user's top-level seed. Each
        // iteration consumes a fresh u64 for its iter_seed; num_calls
        // falls out of iter_seed deterministically.
        let mut top: ChaCha8Rng = ChaCha8Rng::seed_from_u64(args.seed);
        (0..args.iterations).map(|_| top.random::<u64>()).collect()
    };

    let driver_total = actions
        .iter()
        .filter(|a| matches!(a, DriverAction::Container(_)))
        .count();

    // Distribute iterations across `--threads` workers round-robin so
    // each worker sees a representative mix of seeds (rather than e.g.
    // worker 0 getting only the first-emitted ChaCha output which
    // tends to differ in distribution from the tail).
    let n_threads = args.threads.max(1) as usize;
    let n_cpus = online_cpu_count();
    if n_threads > n_cpus {
        return Err(format!(
            "--threads {n_threads} exceeds online CPU count {n_cpus}; \
             each worker is pinned to a distinct core, so more threads \
             than cores is rejected"
        )
        .into());
    }
    let mut chunks: Vec<Vec<(usize, u64)>> = (0..n_threads).map(|_| Vec::new()).collect();
    for (i, &seed) in iterations.iter().enumerate() {
        chunks[i % n_threads].push((i, seed));
    }
    println!(
        "spawning {n_threads} worker thread(s) (pinned to CPUs 0..{}) for {} iterations",
        n_threads - 1,
        iterations.len(),
    );

    // From here on, serial output is captured per-branch (one buffer
    // per concurrent iteration). Only iteration-completion lines and
    // the failure dump reach stdout. `--show-output` keeps the
    // pre-fuzz "print every line live" behaviour on top of capture, so
    // the user can watch a replay iteration unfold without losing the
    // post-failure dump.
    if !args.show_output {
        sink.enter_fuzz_mode();
    }

    let stop = AtomicBool::new(false);
    let completed = AtomicU32::new(0);
    // Running totals across all worker threads, in nanoseconds. We
    // accumulate u64 ns rather than seconds because atomics can't
    // safely sum floats and ns has enough range (2^64 ns ≈ 584 years)
    // for any realistic fuzz session. Worker threads `fetch_add` their
    // per-iteration contribution; the running average is
    // `total / completed` recomputed on each completion line.
    let total_vt_ns = AtomicU64::new(0);
    let total_wall_ns = AtomicU64::new(0);
    let first_failure: Mutex<Option<FailureReport>> = Mutex::new(None);
    let iterations_total = iterations.len();
    let fuzz_start = Instant::now();

    type WorkerErr = Box<dyn Error + Send + Sync>;
    let total_survived = thread::scope(|s| -> Result<u32, WorkerErr> {
        let handles: Vec<_> = chunks
            .into_iter()
            .enumerate()
            .map(|(tid, seeds)| {
                let discovery_cp = &discovery_cp;
                let actions = &actions[..];
                let check_cmd = check_cmd.as_str();
                let sink = &*sink;
                let stop = &stop;
                let completed = &completed;
                let total_vt_ns = &total_vt_ns;
                let total_wall_ns = &total_wall_ns;
                let first_failure = &first_failure;
                s.spawn(move || {
                    worker(
                        tid,
                        seeds,
                        discovery_cp,
                        actions,
                        check_cmd,
                        sink,
                        max_spacing,
                        freq,
                        driver_total,
                        iterations_total,
                        stop,
                        completed,
                        total_vt_ns,
                        total_wall_ns,
                        first_failure,
                    )
                })
            })
            .collect();
        let mut total = 0u32;
        for h in handles {
            total += h.join().expect("worker thread panicked")?;
        }
        Ok(total)
    })?;

    // If any worker found a liveness failure, the first one stored its
    // report. Print full details + replay instructions and exit non-zero.
    if let Some(report) = first_failure.lock().unwrap().take() {
        println!();
        println!("--- full iteration serial log (t{}) ---", report.tid);
        for line in &report.capture {
            println!("{line}");
        }
        println!("--- end iteration serial log ---");
        println!();
        println!(
            "‼ iter {idx} (worker t{tid}): liveness check failed \
             (status={s} exit={ec}) — a container exited during this \
             input sequence (num_calls={n}, drivers={d}/{dt})",
            idx = report.iter_index,
            tid = report.tid,
            s = report.status,
            ec = report.exit_code,
            n = report.num_calls,
            d = report.driver_subset_size,
            dt = report.driver_total,
        );
        println!();
        println!("Input recording (replay-faithful):");
        for (i, input) in report.input_recording.io_inputs().iter().enumerate() {
            println!(
                "  {i:04}: vt {:>8.3} target {:?} command {:?}",
                input.at.as_secs_f64(),
                input.target,
                input.command,
            );
        }
        println!();
        println!("To reproduce this iteration, re-run with:");
        let excludes: String = args
            .exclude_from_partition
            .iter()
            .map(|c| format!(" \\\n    --exclude-from-partition {c}"))
            .collect();
        println!(
            "  cargo run -p bedrock-lab --example lab_random_drivers -- \\\n    \
             {vmlinux} {initramfs} \\\n    \
             --replay-seed {seed:#018x} \\\n    \
             --max-spacing-ms {spacing}{excludes}",
            vmlinux = args.vmlinux,
            initramfs = args.initramfs,
            seed = report.iter_seed,
            spacing = args.max_spacing_ms,
        );
        std::process::exit(1);
    }

    let tv = total_vt_ns.load(Ordering::Relaxed);
    let tw = total_wall_ns.load(Ordering::Relaxed);
    let vt_per_wall = if tw > 0 { tv as f64 / tw as f64 } else { 0.0 };
    let total_wall = fuzz_start.elapsed().as_secs_f64();
    println!(
        "done: {total_survived}/{iterations_total} iterations survived in {total_wall:.2}s \
         (vt/wall={vt_per_wall:.2}x across all branches)",
    );
    Ok(())
}

/// Worker thread body. Pulls iterations from its assigned chunk,
/// branches each from the shared `discovery_cp`, runs it, runs the
/// liveness check, and reports.
///
/// Returns the number of iterations this worker survived. A liveness
/// failure is recorded in `first_failure` (only the first worker to
/// reach the mutex wins) and signals `stop` so siblings short-circuit.
/// Internal `LabError`s (lab/VM bugs, not workload failures) propagate
/// up through `?`.
#[allow(clippy::too_many_arguments)]
fn worker(
    tid: usize,
    seeds: Vec<(usize, u64)>,
    discovery_cp: &Checkpoint,
    actions: &[DriverAction],
    check_cmd: &str,
    sink: &FuzzerSink,
    max_spacing: VirtDuration,
    freq: u64,
    driver_total: usize,
    iterations_total: usize,
    stop: &AtomicBool,
    completed: &AtomicU32,
    total_vt_ns: &AtomicU64,
    total_wall_ns: &AtomicU64,
    first_failure: &Mutex<Option<FailureReport>>,
) -> Result<u32, Box<dyn Error + Send + Sync>> {
    // Pin to the CPU whose index matches our tid. Caller validates
    // tid < online_cpu_count() so this always points at a real core.
    pin_current_thread_to_cpu(tid).map_err(|e| -> Box<dyn Error + Send + Sync> {
        format!("worker t{tid}: failed to pin to CPU {tid}: {e}").into()
    })?;
    let mut survived = 0u32;
    for (iter_index, iter_seed) in seeds {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let num_calls = derive_num_calls(iter_seed);
        let iter_actions = select_subset(actions, iter_seed);
        let driver_subset_size = iter_actions
            .iter()
            .filter(|a| matches!(a, DriverAction::Container(_)))
            .count();

        // Each iteration branches fresh from the same discovery
        // checkpoint, so faults applied in one iteration never leak
        // into the next — the workload's clean baseline is the branch
        // point, not whatever state the previous iteration left.
        let first_at = discovery_cp.time() + max_spacing;
        let source = RandomDriverSource::new(
            iter_actions,
            iter_seed,
            num_calls,
            first_at,
            max_spacing,
        );
        let iter_wall_start = Instant::now();
        let mut branch = discovery_cp.branch_with_input_source(source)?;
        let bid = branch.id();
        sink.start_capture(bid);

        let span =
            VirtDuration::from_instructions(max_spacing.instructions() * num_calls as u64, freq);
        let deadline = first_at + span + vt_dur!(2 s);

        // Drain inputs. We don't care about per-action exit codes —
        // they're noisy and the only signal we want is whether the
        // workload as a whole survives, which is what the liveness
        // check below answers.
        let drain_result: Result<(), LabError> = loop {
            match branch.run_until(deadline) {
                Ok((_, RunOutcome::ReachedTime)) => break Ok(()),
                Ok((_, RunOutcome::ActionResponse { .. })) => continue,
                Ok((_, RunOutcome::Ready)) => continue,
                Ok((_, RunOutcome::RngExhausted)) => break Ok(()),
                Ok((at, RunOutcome::Yielded { kind })) => {
                    break Err(LabError::UnexpectedExit { at, kind });
                }
                Err(e) => break Err(e),
            }
        };

        // Synchronous liveness check. The bash() call submits with a
        // fresh request_id and waits for the response with that exact
        // id — any responses still in flight from the input sequence
        // get stashed and replayed through later run_until calls
        // rather than being mis-read as the liveness check's reply.
        let liveness = drain_result.and_then(|()| branch.bash(BashTarget::Host, check_cmd));
        // Capture timings before the branch is dropped: the branch's
        // current_time advanced during all the run_until + bash work,
        // so the difference from first_at is the iteration's emulated
        // virtual-time span. iter_wall_start was taken just before
        // branching, so its elapsed gives wall-clock time spent on
        // exactly the same work.
        let vt_secs = (branch.current_time() - first_at).as_secs_f64();
        let wall_dur = iter_wall_start.elapsed();
        let capture = sink.take_capture(bid);
        let recording = branch.input_recording().clone();
        drop(branch);

        let out = liveness?;
        let failed = out.status != 0 || out.exit_code != 0;
        if failed {
            // Race to be the first to record the failure. Whichever
            // worker grabs the mutex first wins; others see `Some` and
            // skip. Setting `stop` then short-circuits siblings still
            // mid-iteration on their next loop check.
            let mut slot = first_failure.lock().unwrap();
            if slot.is_none() {
                *slot = Some(FailureReport {
                    tid,
                    iter_index,
                    iter_seed,
                    num_calls,
                    driver_subset_size,
                    driver_total,
                    capture,
                    input_recording: recording,
                    status: out.status,
                    exit_code: out.exit_code,
                });
                stop.store(true, Ordering::Relaxed);
            }
            return Ok(survived);
        }

        survived += 1;
        // Fold this iteration's vt/wall durations into the global
        // running totals. `as u64` saturates negative or NaN values
        // to 0, which is the safe rounding for time accounting.
        let vt_ns = (vt_secs * 1e9) as u64;
        let wall_ns = wall_dur.as_nanos() as u64;
        let total_vt = total_vt_ns.fetch_add(vt_ns, Ordering::Relaxed) + vt_ns;
        let total_wall = total_wall_ns.fetch_add(wall_ns, Ordering::Relaxed) + wall_ns;
        // Globally monotonic progress count — the Nth iteration to
        // *complete*, across all worker threads. Decoupled from the
        // per-worker chunk index (which would otherwise look erratic
        // because workers progress through their chunks at different
        // rates depending on each iteration's num_calls).
        let progress = completed.fetch_add(1, Ordering::Relaxed) + 1;
        // Global ratio: emulated virtual time covered per wall-clock
        // second, summed across every branch from every thread. With
        // N pinned cores this approaches N (or higher if vt advances
        // faster than wall inside a single branch, e.g. when the
        // guest is mostly idle).
        let vt_per_wall = if total_wall > 0 {
            total_vt as f64 / total_wall as f64
        } else {
            0.0
        };
        println!(
            "[t{tid}] {progress:>5}/{iterations_total} seed={iter_seed:#018x} \
             num_calls={num_calls:>3} drivers={driver_subset_size}/{driver_total} \
             vt/wall={vt_per_wall:>5.2}x survived",
        );
    }
    Ok(survived)
}

#[derive(Parser, Debug)]
#[command(name = "lab_random_drivers")]
#[command(about = "Dumb fuzzer: branch from a baseline checkpoint, run a random-length \
                   sequence of random driver / fault-injector inputs, check container \
                   liveness, repeat with a fresh seed.")]
struct Args {
    /// Path to the vmlinux ELF image.
    vmlinux: String,

    /// Path to an initramfs/initrd image.
    initramfs: String,

    /// Number of fuzz iterations. Each iteration branches from the
    /// discovery checkpoint, runs a randomly-sized input sequence, and
    /// checks that no container has exited.
    #[arg(long, default_value_t = 50)]
    iterations: u32,

    /// Number of worker threads. Each holds its own live `Branch` from
    /// the shared discovery checkpoint, so memory cost scales roughly
    /// linearly with thread count (one VM image + its writable CoW
    /// working set per concurrent branch).
    #[arg(long, default_value_t = 1)]
    threads: u32,

    /// Upper bound on the random inter-call delay, in milliseconds. The
    /// actual delay is drawn uniformly from `[0, max-spacing-ms]`, so 0
    /// is the "as tight as the lab allows" case.
    #[arg(long, default_value_t = 500)]
    max_spacing_ms: u64,

    /// Top-level seed. A separate LCG over this seed derives one
    /// `iter_seed` per iteration, so the whole fuzz session is
    /// reproducible run-to-run.
    #[arg(long, default_value_t = 0xbeef_cafe_dead_face_u64)]
    seed: u64,

    /// Replay mode: re-run exactly one iteration with this `iter_seed`
    /// (as printed by a previous fuzz session's failure report). When
    /// set, `--iterations` and `--seed` are ignored. `num_calls` is
    /// derived from the seed via `derive_num_calls`, so this is the
    /// full repro key by itself.
    #[arg(long, value_parser = parse_u64_hex_or_dec)]
    replay_seed: Option<u64>,

    /// Stream every guest serial line straight to stdout for the duration
    /// of the run, instead of buffering each branch's output and dumping
    /// only on liveness failure. Most useful in replay mode
    /// (`--replay-seed`) where you want to watch the single iteration
    /// unfold live; also handy for debugging individual workers when
    /// `--threads 1` is set. Has no effect on the failure report — the
    /// capture is still recorded so the post-failure dump still works.
    #[arg(long = "show-output")]
    show_output: bool,

    /// Container names to exclude from the partition pool. Repeatable.
    /// The named containers still appear in the driver/discovery list
    /// and can be invoked normally; only `fault-injector partition
    /// <name>` actions targeting them are dropped. Useful when one
    /// container is known to legitimately exit on isolation (e.g. a
    /// stress client whose `redis:6379` connection times out and
    /// triggers a wrapper-script exit) and you want to fuzz the other
    /// containers without that noise.
    #[arg(long = "exclude-from-partition")]
    exclude_from_partition: Vec<String>,
}

/// Number of online CPU cores reported by the kernel. Used to cap
/// `--threads` (we pin each worker to a distinct core).
fn online_cpu_count() -> usize {
    let n = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    if n < 1 {
        1
    } else {
        n as usize
    }
}

/// Pin the calling thread to a single CPU. Subsequent execution
/// (including the long `Vm::run` ioctls that dominate worker time)
/// stays on that core, avoiding cross-core migration and the cache
/// thrash it causes when many workers contend for the kernel
/// scheduler.
fn pin_current_thread_to_cpu(cpu: usize) -> std::io::Result<()> {
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(cpu, &mut set);
        // pid=0 means "current thread" on Linux.
        let rc = libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Per-iteration input count derived from the iteration seed via a
/// fresh ChaCha8 stream. Length is uniform in `[1, MAX_CALLS_PER_ITER]`
/// (unbiased — `gen_range` uses rejection sampling). Pure function of
/// the seed so `--replay-seed` alone reproduces the iteration.
fn derive_num_calls(iter_seed: u64) -> u32 {
    let mut rng = ChaCha8Rng::seed_from_u64(iter_seed ^ 0x9e37_79b9_7f4a_7c15);
    // Floor is MAX/2 so every iteration drives at least half the max
    // — see MAX_CALLS_PER_ITER's doc for why short-input dominance is
    // bad. Integer division rounds down; with MAX=30 the range is
    // [15, 30].
    rng.random_range(MAX_CALLS_PER_ITER / 2..=MAX_CALLS_PER_ITER)
}

/// Per-iteration driver subset. Each `Container` driver gets an
/// independent 50% Bernoulli draw off a seed-derived ChaCha8 stream,
/// so different iterations exercise different driver mixes (and
/// replay reproduces the same mix). Fault-injection actions
/// (`FaultPartition` / `FaultClear`) are always kept — filtering them
/// would break the partition→clear state machine.
fn select_subset(actions: &[DriverAction], iter_seed: u64) -> Vec<DriverAction> {
    let mut rng = ChaCha8Rng::seed_from_u64(iter_seed ^ 0xdead_beef_cafe_f00d);
    actions
        .iter()
        .filter_map(|a| match a {
            DriverAction::Container(_) => rng.random_bool(0.5).then(|| a.clone()),
            DriverAction::FaultPartition(_) | DriverAction::FaultClear => Some(a.clone()),
        })
        .collect()
}

/// Accept either `0xABCD...` or plain decimal for replay seeds.
fn parse_u64_hex_or_dec(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).map_err(|e| format!("bad hex u64: {e}"))
    } else {
        s.parse::<u64>().map_err(|e| format!("bad decimal u64: {e}"))
    }
}

/// One picker entry. Either a per-container workload driver
/// (discovered via `workload_details`), a fault-injector partition
/// targeting a specific container, or the fault-injector clear.
#[derive(Clone)]
enum DriverAction {
    Container(WorkloadDriver),
    FaultPartition(String),
    FaultClear,
}

/// [`InputSource`] that fires driver invocations at randomly-spaced
/// virtual times, picking from a discovered action list with three
/// independent ChaCha8 streams (picker / delay / rng) all seeded from
/// one user-supplied iter_seed. ChaCha8 is uniform across all bit
/// positions (the prior LCG had a period-2 low bit, which biased
/// `state & 1` and `state % small_power_of_two` toward alternating
/// patterns), and `gen_range` uses rejection sampling so modular
/// picks aren't skewed when the range doesn't divide 2^64.
///
/// `fault_active` tracks the partition state machine: `FaultClear` is
/// only eligible when a partition is currently applied, so the picker
/// never produces `clear clear` sequences (it does allow back-to-back
/// `partition partition`, which the fault-injector layers under one
/// nft table).
#[derive(Clone)]
struct RandomDriverSource {
    actions: Vec<DriverAction>,
    rng: ChaCha8Rng,
    picker: ChaCha8Rng,
    delay: ChaCha8Rng,
    remaining: u32,
    next_at: VirtTime,
    max_spacing: VirtDuration,
    fault_active: bool,
}

impl RandomDriverSource {
    fn new(
        actions: Vec<DriverAction>,
        seed: u64,
        num_calls: u32,
        first_at: VirtTime,
        max_spacing: VirtDuration,
    ) -> Self {
        // XOR-derive independent seeds so the three streams advance
        // separately. The XOR constants are arbitrary high-entropy
        // values, used here only to decorrelate the three streams.
        Self {
            actions,
            rng: ChaCha8Rng::seed_from_u64(seed ^ 0xa55a_5aa5_a55a_5aa5),
            picker: ChaCha8Rng::seed_from_u64(seed),
            delay: ChaCha8Rng::seed_from_u64(seed ^ 0x9e37_79b9_7f4a_7c15),
            remaining: num_calls,
            next_at: first_at,
            max_spacing,
            fault_active: false,
        }
    }
}

impl InputSource for RandomDriverSource {
    fn next_rng_u64(&mut self) -> Option<u64> {
        Some(self.rng.random())
    }

    fn next_io_input(&mut self) -> Option<IoInput> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;

        // Filter eligible actions by current fault state. `FaultClear`
        // drops out when no partition is active; everything else is
        // always eligible.
        let eligible: Vec<&DriverAction> = self
            .actions
            .iter()
            .filter(|a| self.fault_active || !matches!(a, DriverAction::FaultClear))
            .collect();
        let pick = self.picker.random_range(0..eligible.len());
        let action = eligible[pick];

        let (target, command) = match action {
            DriverAction::Container(d) => {
                (BashTarget::container(&d.container), d.driver.clone())
            }
            DriverAction::FaultPartition(c) => {
                self.fault_active = true;
                (BashTarget::Host, format!("fault-injector partition {c}"))
            }
            DriverAction::FaultClear => {
                self.fault_active = false;
                (BashTarget::Host, "fault-injector clear".into())
            }
        };

        let at = self.next_at;
        // Uniform delay in `[0, max_spacing]` instructions. With max=0
        // (or when the rare 0 draw happens), this input lands at the
        // same virtual time as the previous one — the lab queues them
        // serially as the prior response drains.
        let span = self.max_spacing.instructions();
        let delay_inst = if span > 0 {
            self.delay.random_range(0..=span)
        } else {
            0
        };
        let delay = VirtDuration::from_instructions(delay_inst, self.max_spacing.frequency());
        self.next_at = self.next_at + delay;

        Some(IoInput {
            at,
            target,
            command,
        })
    }

    fn clone_box(&self) -> Box<dyn InputSource> {
        Box::new(self.clone())
    }
}

/// Multi-thread-aware serial-line sink with two modes:
///
/// - **`print_all = true`** (boot + discovery): print every serial line
///   to stdout immediately, matching the original `SerialSink`
///   behaviour. Used while the main thread sets up the baseline VM
///   and discovers the workload.
/// - **`print_all = false`** (fuzzing): events for branches registered
///   via `start_capture` are buffered into per-branch `Vec<String>`
///   buffers; events for unregistered branches are silently dropped.
///   This way concurrent worker threads don't interleave their guest
///   output on screen — the only screen output during fuzzing is the
///   atomic per-iteration completion line each worker prints itself.
///
/// All state behind a single `Mutex` because `EventSink::on_event`
/// takes `&self`. Critical sections are very short (push one string),
/// so contention across threads is negligible.
struct FuzzerSink {
    inner: Mutex<FuzzerSinkInner>,
}

struct FuzzerSinkInner {
    print_all: bool,
    captures: HashMap<BranchId, Vec<String>>,
}

impl FuzzerSink {
    fn new() -> Self {
        Self {
            inner: Mutex::new(FuzzerSinkInner {
                print_all: true,
                captures: HashMap::new(),
            }),
        }
    }

    /// Switch from "print every serial line" to "capture only registered
    /// branches". Call once after boot + discovery and before spawning
    /// fuzz workers.
    fn enter_fuzz_mode(&self) {
        self.inner.lock().unwrap().print_all = false;
    }

    /// Begin capturing events for `branch`. Subsequent events tagged
    /// with this branch ID go into a per-branch buffer instead of
    /// stdout.
    fn start_capture(&self, branch: BranchId) {
        self.inner
            .lock()
            .unwrap()
            .captures
            .insert(branch, Vec::new());
    }

    /// Stop capturing for `branch` and return its accumulated lines.
    fn take_capture(&self, branch: BranchId) -> Vec<String> {
        self.inner
            .lock()
            .unwrap()
            .captures
            .remove(&branch)
            .unwrap_or_default()
    }
}

impl EventSink for FuzzerSink {
    fn on_event(&self, event: Event<'_>) {
        let Event::SerialLine { branch, at, line } = event else {
            return;
        };
        let formatted = format!(
            "[br {branch:?} vt {:>8.3}] {}",
            at.as_secs_f64(),
            String::from_utf8_lossy(line).trim_end_matches('\n'),
        );
        let mut s = self.inner.lock().unwrap();
        if let Some(buf) = s.captures.get_mut(&branch) {
            buf.push(formatted.clone());
        }
        // Print to stdout whenever `print_all` is set, regardless of
        // whether the line was also captured. This lets `--show-output`
        // stream every branch's serial line live AND still preserve the
        // capture for the post-failure dump. Outside `--show-output`,
        // `print_all` flips false in `enter_fuzz_mode` and captured
        // lines stay in the buffer until taken.
        let print = s.print_all;
        // Release the mutex before printing — stdout takes its own
        // lock and we don't want to hold ours across IO.
        drop(s);
        if print {
            println!("{formatted}");
        }
    }
}

/// First-failure report produced when one worker hits a liveness
/// check failure. The lab side serializes on `Mutex<Option<...>>` so
/// only the first failing worker fills the slot; the rest see it
/// already populated and exit through the `stop` flag.
struct FailureReport {
    tid: usize,
    iter_index: usize,
    iter_seed: u64,
    num_calls: u32,
    driver_subset_size: usize,
    driver_total: usize,
    capture: Vec<String>,
    input_recording: InputRecording,
    status: i32,
    exit_code: i32,
}
