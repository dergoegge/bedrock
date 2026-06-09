// SPDX-License-Identifier: GPL-2.0

//! `flux` driver. Boots a Linux guest to its ready hypercall, discovers the
//! workload's drivers, then runs a shared-everything parallel [`Campaign`]: one
//! global corpus/tree + coverage, with N worker threads that just execute. Any
//! worker can build on any other worker's discovered checkpoint.

use std::collections::hash_map::RandomState;
use std::error::Error;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bedrock_lab::{Checkpoint, LabOpts, RngMode, VirtDuration, VirtTime};
use bedrock_vm::{boot::defaults, load_kernel, LinuxBootConfig, VmBuilder, DEFAULT_TSC_FREQUENCY};
use clap::{Parser, ValueEnum};

use flux::mutate::{Action, SwarmMode};
use flux::{ui, Campaign, Config, Sink};

#[derive(Parser, Debug)]
#[command(name = "flux")]
#[command(about = "Clean-room coverage-guided fuzzer for bedrock VMs")]
struct Args {
    /// Path to the uncompressed kernel image (vmlinux) to boot.
    vmlinux: PathBuf,

    /// Path to the initramfs image to boot (the workload initrd).
    initramfs: PathBuf,

    /// Guest RAM, in MiB.
    #[arg(short = 'm', long, default_value_t = 10240)]
    memory: usize,

    /// Maximum virtual time (seconds) to wait for the guest's ready hypercall
    /// during boot before aborting. Generous by default — a heavy
    /// container workload can take many hundreds of virtual seconds to settle.
    #[arg(long, default_value_t = 900.0)]
    ready_deadline_secs: f64,

    /// Longest per-worker run window, in virtual seconds (used by the last
    /// worker). Workers get exponentially-spaced windows from
    /// `--min-run-for-secs` up to this, so the fleet fuzzes a wide range of
    /// branch lengths at once. Inserted IO is scattered across a branch's
    /// window, so this also sets how densely it's driven.
    #[arg(long, default_value_t = 5.0)]
    run_for_secs: f64,

    /// Shortest per-worker run window, in virtual seconds (used by worker 0).
    /// Clamped to `(0, run_for_secs]`; ignored with a single worker.
    #[arg(long, default_value_t = 0.5)]
    min_run_for_secs: f64,

    /// Number of worker threads, each pinned round-robin to a core. All share
    /// one global corpus/tree, coverage map, and stats.
    #[arg(long, default_value_t = 1)]
    threads: usize,

    /// Per-pick havoc-stage early-stop: re-sample after this many *consecutive*
    /// rounds find nothing novel. A novel find resets the count, so a
    /// productive entry is mined until it goes quiet.
    #[arg(long, default_value_t = 32)]
    max_dry_rounds: usize,

    /// Upper bound on actions inserted per IoInsert burst (drawn `1..=burst`).
    #[arg(long, default_value_t = 40)]
    burst: usize,

    /// Swarm-testing mode for action selection.
    #[arg(long, value_enum, default_value_t = Swarm::Lineage)]
    swarm: Swarm,

    /// Containers to exclude from the fault-injection partition pool.
    #[arg(long = "exclude-from-partition")]
    exclude_from_partition: Vec<String>,

    /// Disable the `eventually_` invariant-check pass entirely. Any discovered
    /// `eventually_` drivers are ignored — never fired at the end of a branch and
    /// never added to the normal action vocabulary.
    #[arg(long = "no-eventually")]
    no_eventually: bool,

    /// Chance (percent) that a branch which covered *no* new edges still runs an
    /// `eventually_` invariant-check pass. Branches that did cover new edges
    /// always run one. Has no effect unless the workload ships `eventually_`
    /// drivers.
    #[arg(long = "eventually-pct", default_value_t = 10)]
    eventually_pct: u64,

    /// Virtual-time window for an `eventually_` pass (kill in-flight drivers,
    /// clear faults, then run the invariant checks). Must exceed the barrier
    /// plus the slowest `eventually_` driver — each driver fires ~a quarter of
    /// the way in, so the window needs to be roughly 4/3 of that driver's
    /// worst-case runtime. The default fits a ~60s convergence-poll driver.
    #[arg(long = "eventually-run-for-secs", default_value_t = 90.0)]
    eventually_run_for_secs: f64,

    /// Keep fuzzing after the first solution instead of quitting.
    #[arg(long = "no-quit-on-solution")]
    no_quit_on_solution: bool,

    /// Print the full VM tree on every novel corpus add (O(tree), under lock).
    #[arg(long = "print-tree")]
    print_tree: bool,

    /// Serve the read-only HTTP/SSE state API on this address (e.g.
    /// `127.0.0.1:8080`). Off when unset.
    #[arg(long)]
    http: Option<String>,

    /// Benchmark mode: fuzz for this many wall-clock seconds with output
    /// suppressed, then print one `BENCH_RESULT <json>` line and exit. Forces
    /// `--no-quit-on-solution`. Boot/discovery are excluded from the window.
    #[arg(long)]
    bench_secs: Option<f64>,

    /// Dump the cumulative coverage bitmap per id to `<dir>/<id>.cov` on exit.
    #[arg(long)]
    cov_dump: Option<String>,

    /// Directory to write a crash reproduction (`crash-N.json` replayable input
    /// + `crash-N.serial.log` full root→bug serial) on each solution.
    #[arg(long, default_value = ".")]
    repro_dir: PathBuf,

    /// Reproduce a saved crash: boot to the discovery checkpoint, replay the
    /// input recorded in this `crash-N.json`, print the full serial, and report
    /// whether the crash reproduced. Skips fuzzing.
    #[arg(long)]
    reproduce: Option<PathBuf>,

    /// If set, write each corpus checkpoint's full serial to
    /// `<dir>/cp<id>.log` (grep-able offline; served by the HTTP corpus view).
    /// Off by default — no serial is retained in memory or on disk.
    #[arg(long)]
    serial_dir: Option<PathBuf>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Swarm {
    Lineage,
    Burst,
    Off,
}

impl From<Swarm> for SwarmMode {
    fn from(s: Swarm) -> Self {
        match s {
            Swarm::Lineage => SwarmMode::Lineage,
            Swarm::Burst => SwarmMode::Burst,
            Swarm::Off => SwarmMode::Off,
        }
    }
}

/// Fixed seed for the guest's boot-time RDRAND/RDSEED, so every run boots from
/// the same deterministic stream and reaches the same ready checkpoint. Only
/// the fuzzer's own exploration RNG is randomized per run.
const BOOT_RNG_SEED: u64 = 0xbed0_0001;

/// A fresh, non-deterministic seed from OS entropy for the per-worker mutation
/// RNGs, so two runs explore differently.
fn random_seed() -> u64 {
    RandomState::new().build_hasher().finish()
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let bench_duration = args.bench_secs.map(|s| Duration::from_secs_f64(s.max(0.0)));
    let quiet = bench_duration.is_some();
    ui::init(quiet);

    ui::banner("coverage-guided fuzzer for bedrock VMs");

    let mut vm = VmBuilder::new().memory_mb(args.memory).build()?;
    let kernel = fs::read(&args.vmlinux)?;
    let initramfs = fs::read(&args.initramfs)?;
    let (entry, end) = {
        let memory = vm.memory_mut()?;
        load_kernel(memory, &kernel)?
    };
    let boot = LinuxBootConfig::new(entry, end)
        .cmdline(defaults::CMDLINE)
        .initramfs(&initramfs);
    vm.setup_linux_boot(&boot)?;
    ui::info(&format!(
        "booting {} MiB guest — kernel {}, initramfs {}",
        args.memory,
        args.vmlinux.display(),
        args.initramfs.display()
    ));

    let freq = DEFAULT_TSC_FREQUENCY;
    let deadline = VirtTime::from_secs_f64(args.ready_deadline_secs, freq);
    let sink: Arc<Sink> = Arc::new(Sink::new());
    // From here until `enter_fuzz_mode`, the guest's boot + discovery serial
    // streams into the sink's fixed-height panel — so nothing else should print
    // to stdout in this window, or it would scribble over the panel. All the
    // summary lines below are deferred until after the panel is sealed.
    let ready_cp = Checkpoint::initial_when_ready_with(
        vm,
        deadline,
        LabOpts {
            sink: sink.clone(),
            rng: RngMode::Seeded(BOOT_RNG_SEED),
            tsc_frequency: freq,
        },
    )?;

    // Discover the workload's drivers by forking a throwaway branch, querying
    // the I/O channel, and re-checkpointing — so the fuzzer's first iteration
    // starts from a state where the discovery response has drained.
    let mut discover = ready_cp.branch()?;
    let details = discover.workload_details()?;
    // Split discovered drivers by the `eventually_` filename prefix. Eventually
    // drivers check system-wide invariants: they're kept out of the normal
    // action vocabulary (never inserted mid-branch) and instead fired alone, at
    // the end of a branch, after quiescing the workload (see `run_eventually_pass`).
    let (mut eventually, normal): (Vec<_>, Vec<_>) = details
        .drivers
        .iter()
        .cloned()
        .partition(|d| is_eventually_driver(&d.driver));
    // `--no-eventually` ignores them entirely: dropped from the eventually list
    // (an empty list disables the pass — see `Config::eventually`) and never in
    // `normal`, so they're not added to the action vocabulary either.
    if args.no_eventually {
        eventually.clear();
    }
    let actions = Action::vocabulary(normal, &details.containers, &args.exclude_from_partition);
    let discovery_cp = discover.checkpoint()?;

    // Seal the boot panel; from here serial is captured per-branch and the
    // summary of what we found prints below the panel.
    sink.enter_fuzz_mode();
    ui::good(&format!(
        "ready checkpoint {:?} at {:.3}s",
        ready_cp.id(),
        ready_cp.time().as_secs_f64()
    ));

    // Reproduction mode: replay a saved crash from the discovery checkpoint and
    // report whether it reproduces. No fuzzing.
    if let Some(path) = &args.reproduce {
        let data = fs::read_to_string(path)?;
        let repro: flux::Reproduction = serde_json::from_str(&data)?;
        ui::good(&format!(
            "reproducing {} — {} rng + {} io, replaying to vt {:.1}s ({})",
            path.display(),
            repro.input.rng.len(),
            repro.input.io.len(),
            repro.bug_instr as f64 / repro.frequency.max(1) as f64,
            repro.reason,
        ));
        match flux::replay(&discovery_cp, &repro, &sink) {
            Some(out) => {
                for line in &out.serial {
                    ui::detail(line);
                }
                let reason = if out.crashed {
                    Some("guest yielded on unexpected exit".to_string())
                } else {
                    flux::assertion_failure_reason(&out.serial)
                };
                match reason {
                    Some(r) => ui::solution(&format!("REPRODUCED \u{2713} — {r}")),
                    None => ui::warn(&format!(
                        "did NOT reproduce — no crash by vt {:.1}s ({} serial lines captured)",
                        out.end_instr as f64 / repro.frequency.max(1) as f64,
                        out.serial.len()
                    )),
                }
            }
            None => ui::err("replay failed to start a branch from the discovery checkpoint"),
        }
        return Ok(());
    }
    ui::good(&format!(
        "discovered {} containers, {} drivers ({} eventually)",
        details.containers.len(),
        details.drivers.len(),
        eventually.len(),
    ));
    for d in &details.drivers {
        let kind = if is_eventually_driver(&d.driver) {
            if args.no_eventually {
                " [eventually, disabled]"
            } else {
                " [eventually]"
            }
        } else {
            ""
        };
        ui::detail(&format!("driver {}:{}{kind}", d.container, d.driver));
    }
    ui::info(&format!(
        "{} actions (drivers + fault-injector){}",
        actions.len(),
        if args.exclude_from_partition.is_empty() {
            String::new()
        } else {
            format!(
                " — partition excludes: {}",
                args.exclude_from_partition.join(",")
            )
        }
    ));

    let n_threads = args.threads.max(1);
    let cores: Vec<usize> = (0..flux::affinity::core_count()).collect();
    let run_fors = exponential_run_fors(args.min_run_for_secs, args.run_for_secs, n_threads, freq);
    ui::info(&format!(
        "{n_threads} worker(s) over {} core(s) — run windows {:.2}s … {:.2}s",
        cores.len(),
        run_fors.first().map(|d| d.as_secs_f64()).unwrap_or(0.0),
        run_fors.last().map(|d| d.as_secs_f64()).unwrap_or(0.0),
    ));
    if let Some(d) = &bench_duration {
        ui::info(&format!(
            "benchmark mode — fuzzing {:.0}s, output suppressed",
            d.as_secs_f64()
        ));
    }

    let cfg = Config {
        run_fors,
        frequency: freq,
        actions,
        eventually,
        eventually_pct: args.eventually_pct,
        eventually_run_for: VirtDuration::from_secs_f64(args.eventually_run_for_secs, freq),
        burst: args.burst,
        swarm: args.swarm.into(),
        max_dry_rounds: args.max_dry_rounds,
        print_tree: args.print_tree,
        quit_on_solution: !args.no_quit_on_solution,
        bench_duration,
        repro_dir: Some(args.repro_dir),
        serial_dir: args.serial_dir,
    };
    let campaign = Campaign::new(discovery_cp, ready_cp, sink, cfg);
    campaign.run(random_seed(), &cores, args.http);

    if quiet {
        println!("BENCH_RESULT {}", campaign.stats_json());
    } else {
        ui::good("campaign finished");
    }
    if let Some(dir) = args.cov_dump {
        campaign.dump_coverage(&dir);
    }
    Ok(())
}

/// Whether a discovered driver is an `eventually_` invariant-check driver,
/// identified by its filename (basename) beginning with `eventually_`. `path`
/// is the full in-container driver path (e.g. `/opt/bedrock/drivers/eventually_x`).
fn is_eventually_driver(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .starts_with("eventually_")
}

/// Per-worker run windows, geometrically spaced from `min_secs` (worker 0) up
/// to `max_secs` (worker n-1): worker `i` gets `min * (max/min)^(i/(n-1))`, so
/// windows ramp by a constant *ratio* — low cores cluster near the short end
/// (cheap, fast branches), only the top cores reach the full window. `min_secs`
/// is clamped into `[max * 1e-9, max]`. A single worker gets `max_secs`.
fn exponential_run_fors(min_secs: f64, max_secs: f64, n: usize, freq: u64) -> Vec<VirtDuration> {
    let n = n.max(1);
    let max = max_secs.max(f64::MIN_POSITIVE);
    let min = min_secs.clamp(max * 1e-9, max);
    (0..n)
        .map(|i| {
            let secs = if n == 1 {
                max
            } else {
                let t = i as f64 / (n - 1) as f64;
                min * (max / min).powf(t)
            };
            VirtDuration::from_secs_f64(secs, freq)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FREQ: u64 = DEFAULT_TSC_FREQUENCY;

    fn secs(d: &[VirtDuration]) -> Vec<f64> {
        d.iter().map(VirtDuration::as_secs_f64).collect()
    }

    #[test]
    fn eventually_prefix_matches_basename_only() {
        assert!(is_eventually_driver(
            "/opt/bedrock/drivers/eventually_check"
        ));
        assert!(is_eventually_driver("eventually_x"));
        // Prefix must be on the filename, not anywhere in the path.
        assert!(!is_eventually_driver(
            "/opt/eventually_dir/drivers/mine-blocks"
        ));
        assert!(!is_eventually_driver("/opt/bedrock/drivers/mine-blocks"));
        assert!(!is_eventually_driver(
            "/opt/bedrock/drivers/check_eventually"
        ));
    }

    #[test]
    fn single_worker_gets_full_window() {
        let s = secs(&exponential_run_fors(0.5, 5.0, 1, FREQ));
        assert_eq!(s.len(), 1);
        assert!((s[0] - 5.0).abs() < 1e-3, "{s:?}");
    }

    #[test]
    fn endpoints_are_min_and_max_with_geometric_ramp() {
        let n = 6;
        let (min, max) = (0.25, 8.0);
        let s = secs(&exponential_run_fors(min, max, n, FREQ));
        assert!((s[0] - min).abs() < 1e-3, "{s:?}");
        assert!((s[n - 1] - max).abs() < 1e-3, "{s:?}");
        let expected = (max / min).powf(1.0 / (n - 1) as f64);
        for i in 1..n {
            assert!(s[i] > s[i - 1], "increasing: {s:?}");
            assert!(
                (s[i] / s[i - 1] - expected).abs() < 1e-2,
                "ratio at {i}: {s:?}"
            );
        }
    }

    #[test]
    fn min_above_max_clamps_to_max() {
        for v in secs(&exponential_run_fors(10.0, 2.0, 4, FREQ)) {
            assert!((v - 2.0).abs() < 1e-3);
        }
    }

    #[test]
    fn zero_min_stays_finite() {
        let s = secs(&exponential_run_fors(0.0, 5.0, 4, FREQ));
        assert!(s.iter().all(|v| v.is_finite()), "{s:?}");
        assert!((s[s.len() - 1] - 5.0).abs() < 1e-3);
    }
}
