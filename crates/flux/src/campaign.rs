// SPDX-License-Identifier: GPL-2.0

//! The shared-everything parallel fuzzing campaign.
//!
//! One [`Campaign`] holds *all* global state — the corpus (which is the VM
//! checkpoint tree), the cumulative coverage, the retained rewind
//! intermediates, and the running stats. Worker threads are pure execution
//! engines: each pulls an entry from the shared corpus, mutates it, runs a
//! branch (the expensive part, fully parallel and lock-free), then merges
//! coverage and — if novel — appends to the shared corpus. Because the corpus
//! is global, any worker can build on a checkpoint discovered by any other.
//!
//! Locking is fine-grained: the only thing serialized is quick bookkeeping;
//! `branch.run_until` runs with no campaign lock held.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bedrock_assertions::Assertion;
use bedrock_lab::{
    Branch, Checkpoint, CheckpointId, InputRecording, LabError, RunOutcome, VirtDuration, VirtTime,
    WorkloadDriver,
};

use crate::corpus::{weighted_pick, ActiveBranch, Node, Solution};
use crate::coverage::{read_coverage, Coverage, RunBitmap};
use crate::input::{Input, IoAction, Reproduction, Target};
use crate::mutate::{drift_subset, random_subset, Action, MutationResult, Mutators, SwarmMode};
use crate::rng::Rng;
use crate::shape::strip_ansi;
use crate::sink::Sink;
use crate::views::*;

/// Per-subscriber SSE queue depth. Bounded so a stuck client can't grow memory
/// without limit; events overflow (drop) for that client only.
const SSE_CHANNEL_CAP: usize = 1024;

/// Worker mutation mix (percent): splice grafts another entry's action sequence
/// onto this one; extend appends a forward burst to build longer sequences; the
/// remainder is full havoc.
const SPLICE_PCT: usize = 20;
const EXTEND_PCT: usize = 40;

/// Static campaign configuration, assembled by the driver.
pub struct Config {
    /// Per-worker run window (its length is the worker count). Worker `tid`
    /// runs each branch forward by `run_fors[tid]` and scatters its inserted IO
    /// over the same span. Exponentially spaced so the fleet covers a wide
    /// range of branch lengths at once.
    pub run_fors: Vec<VirtDuration>,
    pub frequency: u64,
    /// The discovered action vocabulary (drivers + fault injection). Excludes
    /// `eventually_` drivers, which are fired by the eventually pass, not
    /// inserted as mid-branch actions.
    pub actions: Vec<Action>,
    /// `eventually_` invariant-check drivers, discovered but kept out of
    /// `actions`. Run alone at the end of a branch by [`Campaign::run_eventually_pass`]
    /// after the workload is quiesced. Empty disables the eventually pass.
    pub eventually: Vec<WorkloadDriver>,
    /// Chance (percent) a *non-novel* branch still runs an eventually pass; a
    /// branch that covered new edges always runs one.
    pub eventually_pct: u64,
    /// Virtual-time window for an eventually pass (kill + clear + invariant
    /// checks).
    pub eventually_run_for: VirtDuration,
    /// Upper bound on actions inserted per `IoInsert` burst.
    pub burst: usize,
    pub swarm: SwarmMode,
    /// Per-pick early-stop: consecutive barren rounds before re-sampling.
    pub max_dry_rounds: usize,
    pub print_tree: bool,
    pub quit_on_solution: bool,
    /// `Some(dur)` runs silently for a fixed wall window then exits (benchmark).
    pub bench_duration: Option<Duration>,
    /// Where to write a [`Reproduction`] (`crash-N.json`) plus the full
    /// root→bug serial (`crash-N.serial.log`) on each solution. `None` disables.
    /// Skipped in bench mode (a crash flood would stall throughput measurement).
    pub repro_dir: Option<PathBuf>,
    /// When set, the *full* serial of every corpus checkpoint is written to
    /// `<dir>/cp<checkpoint-id>.log` as it's added, and the HTTP corpus view
    /// serves serial from there. `None` (default) retains no serial in memory or
    /// on disk — it's used for objective detection then dropped. Opt-in because
    /// it's I/O- and disk-heavy; the on-disk logs are grep-able for analysis.
    pub serial_dir: Option<PathBuf>,
}

/// Aggregated campaign stats, read by the monitor thread.
#[derive(Default)]
struct Stats {
    branches: u64,
    total_vt_instructions: u64,
    corpus_adds: u64,
    solutions: u64,
}

/// Per-phase wall-time profiler. All fields are nanoseconds (or counts) summed
/// across every worker thread, added with `Relaxed` (we only want aggregate
/// magnitudes). Reported on the heartbeat as a `PROF:` line so we can see where
/// worker time actually goes (execution vs fork vs checkpoint vs lock waits)
/// instead of guessing.
#[derive(Default)]
struct Prof {
    iters: AtomicU64,            // run_one invocations
    progress_calls: AtomicU64,   // pump_branch on_progress callbacks (run_until roundtrips)
    branch_ns: AtomicU64,        // branch creation (the fork ioctl)
    pump_ns: AtomicU64,          // running the guest (incl. exit handling)
    cov_read_ns: AtomicU64,      // read_coverage off the branch
    checkpoint_ns: AtomicU64,    // branch.checkpoint()
    cov_merge_ns: AtomicU64,     // coverage merge_bitmap (held + wait)
    dedup_ns: AtomicU64,         // checkpoint deduplicate ioctl
    addnode_ns: AtomicU64,       // rest of add_node (register_node + corpus push)
    corpus_wait_ns: AtomicU64,   // time blocked acquiring the corpus lock
    coverage_wait_ns: AtomicU64, // time blocked acquiring the coverage lock
    active_wait_ns: AtomicU64,   // time blocked acquiring the active lock

    // VM exit breakdown, summed over runs (read off each branch after its run;
    // forked VMs start with fresh stats so each read reflects that run). Tells
    // us whether `pump` is exit-storm-bound (e.g. mtf single-stepping) or
    // genuinely executing the guest.
    ex_total: AtomicU64,
    ex_mtf: AtomicU64,
    ex_ept: AtomicU64,
    ex_extint: AtomicU64,
    ex_rdtsc: AtomicU64,
    ex_apic: AtomicU64,
    ex_msr: AtomicU64,
    ex_io: AtomicU64,
    ex_cpuid: AtomicU64,
    cyc_total: AtomicU64,      // total_run_cycles
    cyc_guest: AtomicU64,      // guest_cycles (actual non-root execution)
    cyc_vmexit_ovh: AtomicU64, // vmexit_overhead_cycles
}

pub struct Campaign {
    corpus: Mutex<Vec<Node>>,
    /// Retained rewind intermediates, keyed by id. Holding a strong handle
    /// keeps the intermediate's VM alive so it shows up in the tree.
    intermediates: Mutex<HashMap<CheckpointId, Checkpoint>>,
    coverage: Mutex<Coverage>,
    solutions: Mutex<Vec<Solution>>,
    /// Reasons (a failed `Always` assertion's message, or the unexpected-exit
    /// string) already reported as findings. A recurrence with a seen reason is
    /// counted but not re-printed or re-saved, so the same bug doesn't flood the
    /// log or repro dir. Its length is the unique-bug count shown on heartbeat.
    seen_reasons: Mutex<HashSet<String>>,
    active: Mutex<HashMap<u64, ActiveBranch>>,
    stats: Mutex<Stats>,
    subscribers: Mutex<Vec<SyncSender<Vec<u8>>>>,
    start: OnceLock<Instant>,
    sink: Arc<Sink>,
    /// Any live checkpoint; used to reach `tree()` for printing + the
    /// checkpoint count. `tree()` always returns the whole graph.
    tree_anchor: Checkpoint,
    cfg: Config,
    /// Cooperative shutdown flag, set on the first solution (when
    /// `quit_on_solution`) or by the bench timer.
    stop: AtomicBool,
    quit_on_solution: bool,
    /// Branches run since the last corpus find, reset to 0 on each add. Feeds
    /// the live spinner ("how long have we been dry").
    branches_since_add: AtomicU64,
    /// Wall-clock instant of the last corpus find (or run start). Feeds the
    /// spinner's "time since last find".
    last_add: Mutex<Instant>,
    /// Per-phase wall-time profiler (see [`Prof`]).
    prof: Prof,
}

impl Campaign {
    /// Build a campaign seeded with the empty input bound to `seed_cp`.
    /// `tree_anchor` is any live checkpoint (typically the boot/ready
    /// checkpoint) used for tree rendering and the checkpoint count.
    pub fn new(
        seed_cp: Checkpoint,
        tree_anchor: Checkpoint,
        sink: Arc<Sink>,
        cfg: Config,
    ) -> Arc<Self> {
        // Bench mode accumulates bugs over the whole window, so never
        // short-circuit on the first one regardless of the flag.
        let quit_on_solution = cfg.quit_on_solution && cfg.bench_duration.is_none();
        // Ensure the output dirs exist up front (best-effort).
        for dir in [&cfg.repro_dir, &cfg.serial_dir].into_iter().flatten() {
            let _ = std::fs::create_dir_all(dir);
        }
        // The seed is the root corpus entry (kept for the whole campaign), so
        // deduplicate it up front like any other retained checkpoint.
        let _ = seed_cp.deduplicate();
        Arc::new(Self {
            corpus: Mutex::new(vec![Node::seed(seed_cp)]),
            intermediates: Mutex::new(HashMap::new()),
            coverage: Mutex::new(Coverage::default()),
            solutions: Mutex::new(Vec::new()),
            seen_reasons: Mutex::new(HashSet::new()),
            active: Mutex::new(HashMap::new()),
            stats: Mutex::new(Stats::default()),
            subscribers: Mutex::new(Vec::new()),
            start: OnceLock::new(),
            sink,
            tree_anchor,
            cfg,
            stop: AtomicBool::new(false),
            quit_on_solution,
            branches_since_add: AtomicU64::new(0),
            last_add: Mutex::new(Instant::now()),
            prof: Prof::default(),
        })
    }

    /// Acquire the corpus lock, charging blocked time to the profiler.
    fn lock_corpus(&self) -> std::sync::MutexGuard<'_, Vec<Node>> {
        let t = Instant::now();
        let g = self.corpus.lock().unwrap();
        self.prof
            .corpus_wait_ns
            .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        g
    }

    /// Acquire the coverage lock, charging blocked time to the profiler.
    fn lock_coverage(&self) -> std::sync::MutexGuard<'_, Coverage> {
        let t = Instant::now();
        let g = self.coverage.lock().unwrap();
        self.prof
            .coverage_wait_ns
            .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        g
    }

    /// Acquire the active-branches lock, charging blocked time to the profiler.
    fn lock_active(&self) -> std::sync::MutexGuard<'_, HashMap<u64, ActiveBranch>> {
        let t = Instant::now();
        let g = self.active.lock().unwrap();
        self.prof
            .active_wait_ns
            .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        g
    }

    /// Run one worker per entry in `run_fors`, pinned round-robin to `cores`,
    /// plus monitor/ticker threads (or a bench timer). Returns when the first
    /// solution is found (unless `quit_on_solution` was cleared) or the bench
    /// window elapses. When `http_addr` is set, an HTTP/SSE server is spawned.
    pub fn run(self: &Arc<Self>, fuzz_seed: u64, cores: &[usize], http_addr: Option<String>) {
        let _ = self.start.set(Instant::now());
        *self.last_add.lock().unwrap() = Instant::now();

        if let Some(addr) = http_addr {
            let campaign = Arc::clone(self);
            std::thread::spawn(move || crate::http::serve(campaign, addr));
        }

        // The always-on shimmering spinner (interactive only); the periodic
        // heartbeat stats print above it. No-op in bench/piped mode.
        crate::ui::spinner_start();

        std::thread::scope(|s| {
            match self.cfg.bench_duration {
                Some(dur) => {
                    s.spawn(move || {
                        if self.sleep_unless_stopped(dur) {
                            self.stop.store(true, Ordering::Relaxed);
                        }
                    });
                }
                None => {
                    s.spawn(|| self.monitor());
                    s.spawn(|| self.stream_ticker());
                }
            }
            for tid in 0..self.cfg.run_fors.len() {
                let core = (!cores.is_empty()).then(|| cores[tid % cores.len()]);
                s.spawn(move || {
                    if let Some(core) = core {
                        crate::affinity::pin_to_core(core);
                    }
                    self.worker(tid, fuzz_seed.wrapping_add(tid as u64));
                });
            }
        });

        crate::ui::spinner_stop();
    }

    fn worker(&self, tid: usize, seed: u64) {
        let mut rng = Rng::new(seed);
        let n_actions = self.cfg.actions.len();
        let run_for = self.cfg.run_fors[tid];
        // Effort one branch run by this worker charges, normalized so the
        // longest-window worker's run costs 1.0.
        let max_window = self.cfg.run_fors.iter().copied().max().unwrap_or(run_for);
        let effort_per_round = if max_window.instructions() == 0 {
            1.0
        } else {
            run_for.instructions() as f64 / max_window.instructions() as f64
        };
        let mutators = Mutators::new(
            self.cfg.actions.clone(),
            run_for.instructions(),
            self.cfg.burst,
            self.cfg.swarm,
        );

        let max_dry = self.cfg.max_dry_rounds.max(1);

        loop {
            if self.stop.load(Ordering::Relaxed) {
                break;
            }

            // 1. Pick one entry by the fertility/rarity/underexplored weighting.
            let (parent_idx, base_input, cp, parent_swarm) = {
                let mut corpus = self.lock_corpus();
                let idx = weighted_pick(&corpus, &mut rng);
                corpus[idx].scheduled += 1;
                (
                    idx,
                    corpus[idx].input.clone(),
                    corpus[idx].checkpoint.clone(),
                    corpus[idx].swarm.clone(),
                )
            };
            let lineage = (!parent_swarm.is_empty()).then(|| parent_swarm.clone());

            // Reserve the entry as in-flight for the whole havoc stage so other
            // workers deprioritize it.
            self.lock_corpus()[parent_idx].in_flight += 1;

            // 2. Havoc stage: mutate-and-run rounds against this one entry until
            //    `max_dry` consecutive rounds turn up nothing novel.
            let mut dry = 0;
            while dry < max_dry {
                if self.stop.load(Ordering::Relaxed) {
                    break;
                }
                dry += 1;

                // Mutate; retry until at least one sub-mutation applies (the
                // seed's empty input only accepts an insert/splice). Mix per
                // attempt: splice (graft another entry's action sequence —
                // combines building blocks a single mutation can't), extend
                // (append a forward burst to build longer sequences), or full
                // havoc (rewrite / retime / re-arg, may rewind for breadth).
                let mut input = base_input.clone();
                input.mutated_at = None;
                let mut mutated = false;
                for _ in 0..8 {
                    let r = match rng.below(100) {
                        k if k < SPLICE_PCT => {
                            let donor = {
                                let corpus = self.lock_corpus();
                                if corpus.len() < 2 {
                                    Vec::new()
                                } else {
                                    corpus[weighted_pick(&corpus, &mut rng)].input.io.clone()
                                }
                            };
                            mutators.splice(&mut rng, &mut input, &donor)
                        }
                        k if k < SPLICE_PCT + EXTEND_PCT => {
                            mutators.extend_forward(&mut rng, &mut input, lineage.as_deref())
                        }
                        _ => mutators.havoc(&mut rng, &mut input, lineage.as_deref()),
                    };
                    if r == MutationResult::Mutated {
                        mutated = true;
                        break;
                    }
                }
                if !mutated {
                    continue;
                }
                let Some(mutated_at) = input.mutated_at else {
                    continue;
                };

                // 3. Run the branch (unlocked, parallel); charge effort.
                self.lock_corpus()[parent_idx].effort += effort_per_round;
                let fresh = rng.next_u64();
                let Some(res) = self.run_one(&cp, parent_idx, &input, mutated_at, fresh, run_for)
                else {
                    continue;
                };

                // 4. Stats.
                {
                    let mut st = self.stats.lock().unwrap();
                    st.branches += 1;
                    st.total_vt_instructions = st.total_vt_instructions.saturating_add(res.vt);
                }
                self.branches_since_add.fetch_add(1, Ordering::Relaxed);

                // 5. Evaluate the branch: merge coverage + report any objective
                //    (a new edge OR a new unique `Always` failure = novel; a VM
                //    crash is terminal). Don't add to the corpus yet — whether we
                //    keep this `end_cp` can also depend on the eventually pass.
                let crashed = res.crashed;
                let end_cp = res.new_cp.clone();
                let (main_novel, mut stop, main_edges) = self.evaluate(parent_idx, &res);
                // Keep a novel main branch eagerly so its eventually forks (live
                // branches included) attach to its node instead of floating.
                let mut res_opt = Some(res);
                let mut main_id = if !crashed && main_novel {
                    Some(self.add_node(
                        parent_idx,
                        &parent_swarm,
                        res_opt.take().unwrap(),
                        &mut rng,
                        n_actions,
                        main_edges,
                    ))
                } else {
                    None
                };

                // 6. Eventually pass from this (non-crashed) branch's end state.
                //    Its branches fork from `end_cp`; if any is kept (new coverage
                //    or a fresh invariant violation) the pass also keeps `end_cp`
                //    itself — lazily adding it when the main branch wasn't novel —
                //    so the kept node attaches to its real parent rather than
                //    floating past `parent_idx`'s bar.
                if !crashed
                    && !stop
                    && !self.cfg.eventually.is_empty()
                    && (main_novel || rng.chance(self.cfg.eventually_pct))
                {
                    let (mid, ev_stop) = self.run_eventually_round(
                        parent_idx,
                        &parent_swarm,
                        end_cp,
                        main_id,
                        res_opt.take(),
                        main_edges,
                        n_actions,
                        &mut rng,
                    );
                    main_id = mid;
                    stop |= ev_stop;
                }

                // 7. A find (here or in the eventually pass) resets the dry
                //    streak so a productive entry keeps being mined.
                if main_id.is_some() {
                    dry = 0;
                }
                if stop {
                    crate::ui::info(
                        "quitting on first solution; pass --no-quit-on-solution to keep fuzzing",
                    );
                    self.stop.store(true, Ordering::Relaxed);
                    break;
                }
            }

            // Stage done: release the entry.
            {
                let mut corpus = self.lock_corpus();
                corpus[parent_idx].in_flight = corpus[parent_idx].in_flight.saturating_sub(1);
            }
        }
    }

    /// Evaluate a finished branch: merge its edge coverage and report any
    /// objective, **without** adding it to the corpus. Returns
    /// `(novel, stop, new_edges)`:
    ///  - `novel` if it's worth keeping — new **edge coverage** OR a new unique
    ///    **`Always` assertion failure** (objective-as-coverage), so the fuzzer
    ///    breeds around what it finds. The caller adds it (see [`Self::add_node`])
    ///    once it has decided the parent.
    ///  - `stop` if a solution fired under `quit_on_solution`.
    ///  - `new_edges` to hand to `add_node` for the "+N edges" record.
    ///
    /// A VM-level crash (`res.crashed`) is the exception: the guest yielded on an
    /// unexpected exit, which both dead-ends the branch and — because a crashed
    /// workload container restarts and re-registers its instrumented feedback
    /// buffer under the same build-id — would fold a slab of bogus edges into the
    /// cumulative map. So a crash is reported but never merged and never novel.
    fn evaluate(&self, parent_idx: usize, res: &RunResult) -> (bool, bool, usize) {
        if res.crashed {
            self.report_solution(
                parent_idx,
                res,
                "guest yielded on unexpected exit".to_string(),
            );
            return (false, self.quit_on_solution, 0);
        }
        let t_cov = Instant::now();
        let new_edges = self.lock_coverage().merge_bitmap(&res.coverage);
        self.prof
            .cov_merge_ns
            .fetch_add(t_cov.elapsed().as_nanos() as u64, Ordering::Relaxed);
        let mut novel = new_edges > 0;
        let mut stop = false;
        if let Some(reason) = assertion_failure_reason(&res.serial) {
            // A new unique assertion failure is itself a novelty signal.
            if self.report_solution(parent_idx, res, reason) {
                novel = true;
            }
            stop = self.quit_on_solution;
        }
        (novel, stop, new_edges)
    }

    /// Record a solution. A solution is a distinct **finding** the first time
    /// its `reason` is seen; later recurrences of the same reason are just
    /// counted. For a failed `Always` assertion the reason is its message, so
    /// findings are keyed by message — the same violated invariant dedups to one
    /// finding regardless of which branch/run hit it. Only findings get the full
    /// treatment — serial retained, printed, and a reproducer written — so
    /// keep-fuzzing runs don't flood the repro dir or memory with duplicates.
    ///
    /// Returns `true` iff this was a **new unique finding** (its reason hadn't
    /// been seen before) — the caller uses that as a novelty signal, breeding
    /// from bug-adjacent states by keeping the branch in the corpus.
    fn report_solution(&self, parent_idx: usize, res: &RunResult, reason: String) -> bool {
        let serial_lines = res.serial.len();
        let is_finding = self.seen_reasons.lock().unwrap().insert(reason.clone());
        let id = {
            let mut sols = self.solutions.lock().unwrap();
            let id = sols.len();
            sols.push(Solution {
                parent: Some(parent_idx),
                checkpoint: res.new_cp.clone(),
                serial: if is_finding {
                    res.serial.clone()
                } else {
                    Vec::new()
                },
                reason: reason.clone(),
            });
            self.stats.lock().unwrap().solutions += 1;
            id
        };
        if !is_finding {
            // Recurrence of a known reason: counted in the heartbeat total, but
            // not printed or re-saved.
            return false;
        }
        crate::ui::solution(&format!(
            "SOLUTION #{id} (from corpus entry {parent_idx}) — {reason}"
        ));
        for line in &res.serial {
            crate::ui::detail(line);
        }
        // Persist a deterministic reproduction (input + full root→bug serial).
        self.save_reproduction(id, res, &reason);
        self.publish(
            "solution",
            &json_string(&SolutionView {
                id,
                parent: Some(parent_idx),
                reason,
                serial_lines,
            }),
        );
        true
    }

    /// Write `crash-<id>.json` (a [`Reproduction`] replayable by `--reproduce`)
    /// and `crash-<id>.serial.log` (the full guest serial from the fuzzing root
    /// to the bug) under the configured directory. Replays the recorded input
    /// from the seed in a single branch to capture the complete serial — which
    /// also self-checks that the saved input actually reproduces the crash.
    fn save_reproduction(&self, id: usize, res: &RunResult, reason: &str) {
        let Some(dir) = self.cfg.repro_dir.clone() else {
            return;
        };
        // Bench mode optimizes for throughput; don't stall it replaying crashes.
        if self.cfg.bench_duration.is_some() {
            return;
        }
        let seed_cp = self.corpus.lock().unwrap()[0].checkpoint.clone();
        let bug_instr = res.new_cp.time().instructions();
        let repro = Reproduction {
            frequency: self.cfg.frequency,
            root_instr: seed_cp.time().instructions(),
            bug_instr,
            reason: reason.to_string(),
            input: recording_to_input(&res.recording, bug_instr),
        };
        let base = format!("{}/crash-{id}", dir.display());
        match serde_json::to_string_pretty(&repro) {
            Ok(j) => {
                if let Err(e) = std::fs::write(format!("{base}.json"), j) {
                    crate::ui::warn(&format!("could not write {base}.json: {e}"));
                    return;
                }
            }
            Err(e) => {
                crate::ui::warn(&format!("could not serialize reproduction: {e}"));
                return;
            }
        }
        // Replay from the seed to capture the full root→bug serial (and confirm
        // the saved input reproduces).
        match replay(&seed_cp, &repro, &self.sink) {
            Some(out) => {
                let _ = std::fs::write(format!("{base}.serial.log"), out.serial.join("\n"));
                let reproduced = out.crashed || assertion_failure_reason(&out.serial).is_some();
                crate::ui::good(&format!(
                    "saved reproduction {base}.json + .serial.log — replay {}",
                    if reproduced {
                        "reproduced the crash ✓"
                    } else {
                        "did NOT reproduce ✗ (nondeterministic?)"
                    }
                ));
            }
            None => crate::ui::warn(&format!(
                "saved {base}.json but replay failed to start a branch"
            )),
        }
    }

    /// Append a novel branch to the shared corpus as a child of `parent_idx`,
    /// returning its new corpus id. `parent_idx` must be the entry the branch
    /// actually forked from (so the web tree's fork dot lands on that node's
    /// bar, not floating past an unrelated ancestor's).
    #[allow(clippy::too_many_arguments)]
    fn add_node(
        &self,
        parent_idx: usize,
        parent_swarm: &[usize],
        res: RunResult,
        rng: &mut Rng,
        n_actions: usize,
        new_edges: usize,
    ) -> usize {
        let t_add = Instant::now();
        // A find resets the "dry streak" the spinner reports.
        self.branches_since_add.store(0, Ordering::Relaxed);
        *self.last_add.lock().unwrap() = Instant::now();
        if let Some(im) = res.intermediate {
            self.intermediates.lock().unwrap().insert(im.id(), im);
        }
        // Rarity at discovery for the scheduler's rare-edge bias: register this
        // node as one owner of each edge it covered and score how rare that
        // territory is. The coverage lock is taken and released here, never held
        // alongside the corpus lock below.
        let rarity = self.lock_coverage().register_node(&res.coverage);
        let runtime_secs = if self.cfg.frequency == 0 {
            0.0
        } else {
            res.vt as f64 / self.cfg.frequency as f64
        };
        let serial_lines = res.serial.len();
        // A failed `Always` assertion on this branch's serial makes it a "bug"
        // node — kept for breeding around the bug and drawn red (not white) in
        // the dashboard.
        let bug = assertion_failure_reason(&res.serial).is_some();
        let time_secs = res.new_cp.time().as_secs_f64();
        let checkpoint = res.new_cp.id().as_u64();
        // No serial is retained in memory. When a serial dir is configured, dump
        // this branch's full captured serial to disk keyed by checkpoint id (so
        // it can be served by the HTTP view and grepped for analysis); otherwise
        // it's dropped here — objective detection already ran on the live copy.
        if let Some(dir) = &self.cfg.serial_dir {
            let _ = std::fs::write(
                format!("{}/cp{checkpoint}.log", dir.display()),
                res.serial.join("\n"),
            );
        }
        // Child lineage subset: the root seeds a fresh regime; deeper nodes
        // inherit their parent's with a small drift.
        let child_swarm = if parent_swarm.is_empty() {
            random_subset(n_actions, rng)
        } else {
            drift_subset(parent_swarm, n_actions, 30, rng)
        };
        // This checkpoint is being retained in the corpus, so it's worth
        // deduplicating its COW pages now (immutable from here, no children
        // yet). We only do this for kept checkpoints — the many transient
        // checkpoints from non-novel runs are dropped without dedup. Best-effort.
        let t_dedup = Instant::now();
        let _ = res.new_cp.deduplicate();
        self.prof
            .dedup_ns
            .fetch_add(t_dedup.elapsed().as_nanos() as u64, Ordering::Relaxed);
        let node = Node {
            input: recording_to_input(&res.recording, res.new_cp.time().instructions()),
            checkpoint: res.new_cp,
            parent: Some(parent_idx),
            serial_lines,
            scheduled: 0,
            effort: 0.0,
            novelty: 0,
            runtime_secs,
            in_flight: 0,
            swarm: child_swarm,
            rarity,
            bug,
        };
        let new_id = {
            let mut corpus = self.lock_corpus();
            corpus[parent_idx].novelty += 1;
            corpus.push(node);
            let new_id = corpus.len() - 1;
            self.stats.lock().unwrap().corpus_adds += 1;
            if self.cfg.print_tree {
                println!("--- vm tree ---\n{}", self.tree_anchor.tree().ascii());
            }
            new_id
        };
        // Concise progress signal: one line per novel find, with how many new
        // edges it covered and where it came from.
        crate::ui::good(&format!(
            "corpus #{new_id} (+{new_edges} edges) · {time_secs:.1}s vt · from #{parent_idx}"
        ));
        self.publish(
            "corpus_add",
            &json_string(&CorpusEntryView {
                id: new_id,
                parent: Some(parent_idx),
                checkpoint,
                time_secs,
                runtime_secs,
                scheduled: 0,
                novelty: 0,
                serial_lines,
                bug,
            }),
        );
        self.prof
            .addnode_ns
            .fetch_add(t_add.elapsed().as_nanos() as u64, Ordering::Relaxed);
        new_id
    }

    /// Rewind/branch/run one mutated input. Returns `None` if there's nothing
    /// to do (no rewindable ancestor, or a lab error we skip rather than abort).
    fn run_one(
        &self,
        cp: &Checkpoint,
        parent_idx: usize,
        input: &Input,
        mutated_at: u64,
        fresh_seed: u64,
        run_for: VirtDuration,
    ) -> Option<RunResult> {
        let target = VirtTime::from_instructions(mutated_at, self.cfg.frequency);
        let (start_cp, intermediate) = if target < cp.time() {
            match cp.rewind(cp.time() - target) {
                Ok(earlier) => (earlier.clone(), Some(earlier)),
                Err(LabError::NoCheckpointBefore { .. }) => return None,
                Err(_) => return None,
            }
        } else {
            (cp.clone(), None)
        };

        let source = input.source_from(mutated_at, self.cfg.frequency, fresh_seed);
        let t_branch = Instant::now();
        let mut branch = start_cp.branch_with_input_source(source).ok()?;
        self.prof
            .branch_ns
            .fetch_add(t_branch.elapsed().as_nanos() as u64, Ordering::Relaxed);
        self.prof.iters.fetch_add(1, Ordering::Relaxed);
        let branch_id = branch.id();
        self.sink.start_capture(branch_id);

        let bid = branch_id.as_u64();
        let start_secs = start_cp.time().as_secs_f64();
        self.lock_active().insert(
            bid,
            ActiveBranch {
                parent: parent_idx,
                start_secs,
                current_secs: start_secs,
            },
        );

        let run_target = start_cp.time() + run_for;
        let t_pump = Instant::now();
        let crashed = pump_branch(&mut branch, run_target, |at| {
            self.prof.progress_calls.fetch_add(1, Ordering::Relaxed);
            if let Some(a) = self.lock_active().get_mut(&bid) {
                a.current_secs = at.as_secs_f64();
            }
        });
        self.prof
            .pump_ns
            .fetch_add(t_pump.elapsed().as_nanos() as u64, Ordering::Relaxed);

        // Exit breakdown for this run (fresh stats per fork). Diagnostic.
        let st = branch.exit_stats().unwrap_or_default();
        let add = |c: &AtomicU64, v: u64| {
            c.fetch_add(v, Ordering::Relaxed);
        };
        add(&self.prof.ex_total, st.total_exit_count());
        add(&self.prof.ex_mtf, st.mtf.count);
        add(&self.prof.ex_ept, st.ept_violation.count);
        add(&self.prof.ex_extint, st.external_interrupt.count);
        add(&self.prof.ex_rdtsc, st.rdtsc.count + st.rdtscp.count);
        add(&self.prof.ex_apic, st.apic_access.count);
        add(&self.prof.ex_msr, st.msr_read.count + st.msr_write.count);
        add(&self.prof.ex_io, st.io_instruction.count);
        add(&self.prof.ex_cpuid, st.cpuid.count);
        add(&self.prof.cyc_total, st.total_run_cycles);
        add(&self.prof.cyc_guest, st.guest_cycles);
        add(&self.prof.cyc_vmexit_ovh, st.vmexit_overhead_cycles);

        self.lock_active().remove(&bid);

        let vt = branch
            .current_time()
            .instructions()
            .saturating_sub(start_cp.time().instructions());
        let t_cov = Instant::now();
        let coverage = read_coverage(&mut branch);
        self.prof
            .cov_read_ns
            .fetch_add(t_cov.elapsed().as_nanos() as u64, Ordering::Relaxed);
        let recording = branch.input_recording().clone();
        let t_cp = Instant::now();
        let new_cp = branch.checkpoint().ok()?;
        self.prof
            .checkpoint_ns
            .fetch_add(t_cp.elapsed().as_nanos() as u64, Ordering::Relaxed);
        let serial = self.sink.take_capture(branch_id);

        Some(RunResult {
            crashed,
            new_cp,
            intermediate,
            recording,
            serial,
            coverage,
            vt,
        })
    }

    /// Quiesce the workload at `end_cp`, then run every `eventually_` invariant
    /// check — each in its own branch off the single quiesced checkpoint —
    /// keeping interesting results *and the ancestor checkpoints they hang off*.
    /// Returns `(main_id, stop)`: `main_id` is the corpus id of `end_cp` if it
    /// ended up kept (so the worker tracks it), `stop` if a solution fired under
    /// `quit_on_solution`.
    ///
    /// Two stages:
    /// 1. **Quiesce**: a forward branch off `end_cp` that kills all in-flight
    ///    drivers and clears faults, then checkpoints the quiesced state.
    /// 2. **Check**: for each eventually driver, a *separate* branch off that one
    ///    quiesced checkpoint running just that driver — so the invariant drivers
    ///    never overlap (each other or anything else) and each checks the *same*
    ///    quiesced state, not one a prior driver mutated.
    ///
    /// Branches are evaluated (coverage merged, objectives reported) but added to
    /// the corpus lazily, **bottom-up**: a node is kept if it is itself novel
    /// (new edges or a fresh `Always` failure) *or* a descendant is kept — so an
    /// interesting eventually check drags in the quiesce checkpoint and `end_cp`
    /// it forked from, and they're added top-down with real parent links (the web
    /// tree then attaches each fork dot on its parent's bar). `end_cp`'s own
    /// ancestors are already retained: `main_id` is a corpus node when set, and
    /// `add_node` keeps the main branch's rewind intermediate.
    ///
    /// `main_id` is `Some` if the worker already kept the (novel) main branch for
    /// `end_cp`; otherwise `main_res` carries it for a lazy add here.
    #[allow(clippy::too_many_arguments)]
    fn run_eventually_round(
        &self,
        parent_idx: usize,
        parent_swarm: &[usize],
        end_cp: Checkpoint,
        mut main_id: Option<usize>,
        main_res: Option<RunResult>,
        main_edges: usize,
        n_actions: usize,
        rng: &mut Rng,
    ) -> (Option<usize>, bool) {
        let window = self.cfg.eventually_run_for;
        // The corpus entry the live quiesce/driver branches reference until their
        // real parent node exists: the kept main node if we have one, else the
        // originating entry (a transient cosmetic for live branches only).
        let live_parent = main_id.unwrap_or(parent_idx);

        // Stage 1: kill in-flight drivers, then clear faults; checkpoint the
        // quiesced state. Evaluate but hold for a lazy, bottom-up add.
        let q_anchor = end_cp.time().instructions();
        let mut q_input = Input::new(q_anchor);
        q_input.io = quiesce_io(q_anchor, window.instructions());
        q_input.mutated_at = Some(q_anchor);
        let mut stop = false;
        let mut q_novel = false;
        let mut q_hold: Option<(RunResult, usize)> = None;
        let mut quiesced_cp: Option<Checkpoint> = None;
        if let Some(q_res) = self.run_one(
            &end_cp,
            live_parent,
            &q_input,
            q_anchor,
            rng.next_u64(),
            window,
        ) {
            let q_crashed = q_res.crashed;
            let (nv, st, ed) = self.evaluate(live_parent, &q_res);
            q_novel = nv;
            stop = st;
            // A crashed quiesce is terminal — don't keep it or run checks off it
            // (barrier SIGKILLs exit 137 and are suppressed, so nothing spurious
            // surfaces here).
            if !q_crashed {
                quiesced_cp = Some(q_res.new_cp.clone());
                q_hold = Some((q_res, ed));
            }
        }

        // Stage 2: each eventually driver alone, off the quiesced checkpoint.
        let mut kept_drivers: Vec<(RunResult, usize)> = Vec::new();
        if let (Some(qcp), false) = (&quiesced_cp, stop) {
            let d_anchor = qcp.time().instructions();
            for d in &self.cfg.eventually {
                let mut d_input = Input::new(d_anchor);
                d_input.io = eventually_driver_io(d_anchor, window.instructions(), d);
                d_input.mutated_at = Some(d_anchor);
                let Some(d_res) =
                    self.run_one(qcp, live_parent, &d_input, d_anchor, rng.next_u64(), window)
                else {
                    continue;
                };
                let (d_novel, d_stop, d_edges) = self.evaluate(live_parent, &d_res);
                if d_novel {
                    kept_drivers.push((d_res, d_edges));
                }
                if d_stop {
                    stop = true;
                    break;
                }
            }
        }

        // Decide keeps bottom-up: keep the quiesce node if it's novel or hosts a
        // kept driver; keep `end_cp` if it's novel or hosts the kept quiesce.
        let keep_quiesce = q_hold.is_some() && (q_novel || !kept_drivers.is_empty());
        if (keep_quiesce || !kept_drivers.is_empty()) && main_id.is_none() {
            // An interesting eventually result hangs off `end_cp`; keep it so the
            // result attaches to its real parent. (`main_res` is `Some` here —
            // the worker only defers the add when the main branch wasn't novel.)
            if let Some(mres) = main_res {
                main_id =
                    Some(self.add_node(parent_idx, parent_swarm, mres, rng, n_actions, main_edges));
            }
        }

        // Add the kept eventually nodes top-down with real parent links.
        let q_parent = main_id.unwrap_or(parent_idx);
        let q_id = if keep_quiesce {
            let (q_res, q_edges) = q_hold.expect("keep_quiesce implies q_hold is Some");
            Some(self.add_node(q_parent, parent_swarm, q_res, rng, n_actions, q_edges))
        } else {
            None
        };
        let d_parent = q_id.unwrap_or(q_parent);
        for (d_res, d_edges) in kept_drivers {
            self.add_node(d_parent, parent_swarm, d_res, rng, n_actions, d_edges);
        }
        (main_id, stop)
    }

    /// Sleep up to `dur`, waking early if `stop` is set. Returns `true` if the
    /// full nap elapsed (do periodic work), `false` if stop was observed (exit).
    fn sleep_unless_stopped(&self, dur: Duration) -> bool {
        let step = Duration::from_millis(100);
        let mut slept = Duration::ZERO;
        while slept < dur {
            if self.stop.load(Ordering::Relaxed) {
                return false;
            }
            let nap = step.min(dur - slept);
            std::thread::sleep(nap);
            slept += nap;
        }
        !self.stop.load(Ordering::Relaxed)
    }

    /// One-line profiler dump: per-run wall-time spent in each phase and per-run
    /// time blocked on each shared lock, plus cumulative lock-wait seconds
    /// (summed across all worker threads). Lets us see the bottleneck rather
    /// than guess. Note `active` lock-wait is mostly incurred inside `pump` (the
    /// per-run-roundtrip progress callback), so it overlaps the `pump` figure.
    fn prof_line(&self, wall_secs: f64) -> String {
        let p = &self.prof;
        let iters = p.iters.load(Ordering::Relaxed).max(1);
        // Per-run average in microseconds.
        let us = |c: &AtomicU64| (c.load(Ordering::Relaxed) as f64 / iters as f64) / 1000.0;
        // Cumulative thread-seconds across all workers.
        let s = |c: &AtomicU64| c.load(Ordering::Relaxed) as f64 / 1e9;
        format!(
            "PROF: {iters} runs ({:.0}/s) · {:.1} roundtrips/run · per-run us [branch {:.0} pump {:.0} covrd {:.0} ckpt {:.0} covmrg {:.0} dedup {:.0} addnode {:.0}] · lock-wait us/run [corpus {:.0} cov {:.0} active {:.0}] · lock-wait total s [corpus {:.0} cov {:.0} active {:.0}]",
            iters as f64 / wall_secs.max(1.0),
            p.progress_calls.load(Ordering::Relaxed) as f64 / iters as f64,
            us(&p.branch_ns),
            us(&p.pump_ns),
            us(&p.cov_read_ns),
            us(&p.checkpoint_ns),
            us(&p.cov_merge_ns),
            us(&p.dedup_ns),
            us(&p.addnode_ns),
            us(&p.corpus_wait_ns),
            us(&p.coverage_wait_ns),
            us(&p.active_wait_ns),
            s(&p.corpus_wait_ns),
            s(&p.coverage_wait_ns),
            s(&p.active_wait_ns),
        )
    }

    /// Per-run VM-exit breakdown + guest-vs-overhead cycle split. If `mtf`/run
    /// is huge it's single-stepping; if guest% is tiny the run is exit-bound;
    /// if guest% is high the guest genuinely executes that much.
    fn exit_line(&self) -> String {
        let p = &self.prof;
        let iters = p.iters.load(Ordering::Relaxed).max(1);
        let per = |c: &AtomicU64| c.load(Ordering::Relaxed) / iters;
        let ct = p.cyc_total.load(Ordering::Relaxed).max(1) as f64;
        let guest_pct = 100.0 * p.cyc_guest.load(Ordering::Relaxed) as f64 / ct;
        let ovh_pct = 100.0 * p.cyc_vmexit_ovh.load(Ordering::Relaxed) as f64 / ct;
        format!(
            "EXITS: {}/run [mtf {} ept {} extint {} rdtsc {} apic {} msr {} io {} cpuid {}] · cycles guest {guest_pct:.1}% vmexit-ovh {ovh_pct:.1}%",
            per(&p.ex_total),
            per(&p.ex_mtf),
            per(&p.ex_ept),
            per(&p.ex_extint),
            per(&p.ex_rdtsc),
            per(&p.ex_apic),
            per(&p.ex_msr),
            per(&p.ex_io),
            per(&p.ex_cpuid),
        )
    }

    fn monitor(&self) {
        while self.sleep_unless_stopped(Duration::from_secs(15)) {
            let view = self.stats_view();
            crate::ui::heartbeat(&format!(
                "wall {:.0}s · vt {:.0}s ({:.1}x) · {} branches · {} corpus (+{}) · {} checkpoints · {} bugs ({} unique)",
                view.wall_secs, view.vt_secs, view.vt_per_wall, view.branches,
                view.corpus, view.adds, view.checkpoints, view.solutions, view.unique_solutions,
            ));
            crate::ui::heartbeat(&self.prof_line(view.wall_secs));
            crate::ui::heartbeat(&self.exit_line());
            let edges: usize = view.coverage.iter().map(|c| c.seen).sum();
            let total: usize = view.coverage.iter().map(|c| c.total).sum();
            if total > 0 {
                let pct = 100.0 * edges as f64 / total as f64;
                crate::ui::detail(&format!(
                    "edges {edges}/{total} ({pct:.1}%) across {} buffers",
                    view.coverage.len()
                ));
            }
        }
    }

    /// Stream live state a few times a second: the in-flight branch frontier
    /// plus a fresh `stats` snapshot. Also prunes disconnected subscribers.
    fn stream_ticker(&self) {
        while self.sleep_unless_stopped(Duration::from_millis(250)) {
            let list: Vec<BranchLiveView> = {
                let a = self.active.lock().unwrap();
                a.iter()
                    .map(|(id, b)| BranchLiveView {
                        id: format!("br{id}"),
                        parent: b.parent,
                        start_secs: b.start_secs,
                        current_secs: b.current_secs,
                    })
                    .collect()
            };
            self.publish("branches", &json_string(&list));
            self.publish("stats", &json_string(&self.stats_view()));

            // Refresh the live spinner's parenthetical: how long, and how many
            // branches, since the last corpus find.
            let since = self.last_add.lock().unwrap().elapsed().as_secs_f64();
            let dry = self.branches_since_add.load(Ordering::Relaxed);
            crate::ui::set_spinner_status(format!("{since:.0}s, {dry} branches"));
        }
    }
}

// ---- HTTP/SSE state API ----------------------------------------------------

impl Campaign {
    /// Register a new SSE subscriber, returning the receiver the HTTP server
    /// streams to one client.
    pub fn subscribe(&self) -> Receiver<Vec<u8>> {
        let (tx, rx) = sync_channel(SSE_CHANNEL_CAP);
        self.subscribers.lock().unwrap().push(tx);
        rx
    }

    /// Fan one SSE event out to every subscriber, pruning disconnected ones.
    /// Non-blocking: a full (slow) client drops the event.
    fn publish(&self, event: &str, data: &str) {
        let frame = format!("event: {event}\ndata: {data}\n\n").into_bytes();
        let mut subs = self.subscribers.lock().unwrap();
        subs.retain(|tx| {
            !matches!(
                tx.try_send(frame.clone()),
                Err(TrySendError::Disconnected(_))
            )
        });
    }

    fn stats_view(&self) -> StatsView {
        let wall = self.start.get().map_or(0.0, |s| s.elapsed().as_secs_f64());
        let (branches, vt_instr, adds, solutions) = {
            let s = self.stats.lock().unwrap();
            (
                s.branches,
                s.total_vt_instructions,
                s.corpus_adds,
                s.solutions,
            )
        };
        let vt = if self.cfg.frequency == 0 {
            0.0
        } else {
            vt_instr as f64 / self.cfg.frequency as f64
        };
        let vt_per_wall = if wall > 0.0 { vt / wall } else { 0.0 };
        let unique_solutions = self.seen_reasons.lock().unwrap().len() as u64;
        let corpus = self.corpus.lock().unwrap().len();
        let checkpoints = self.tree_anchor.tree().checkpoints().len();
        let coverage = {
            let cov = self.coverage.lock().unwrap();
            cov.summary()
                .into_iter()
                .map(|(id, seen, total)| CoverageView { id, seen, total })
                .collect()
        };
        StatsView {
            wall_secs: wall,
            vt_secs: vt,
            vt_per_wall,
            branches,
            checkpoints,
            corpus,
            adds,
            solutions,
            unique_solutions,
            coverage,
        }
    }

    /// `/stats`
    pub fn stats_json(&self) -> String {
        json_string(&self.stats_view())
    }

    /// Dump the cumulative coverage bitmap per id to `<dir>/<id>.cov` (one line
    /// per covered edge index). Used offline to map edges to source files.
    pub fn dump_coverage(&self, dir: &str) {
        let cov = self.coverage.lock().unwrap();
        for (id, bm) in cov.bitmaps().iter() {
            let name = String::from_utf8_lossy(id);
            let mut out = String::new();
            for (i, &b) in bm.iter().enumerate() {
                if b != 0 {
                    out.push_str(&i.to_string());
                    out.push('\n');
                }
            }
            let _ = std::fs::write(format!("{dir}/{name}.cov"), out);
        }
    }

    /// `/tree` — the live checkpoint + branch genealogy as JSON.
    pub fn tree_json(&self) -> String {
        let tree = self.tree_anchor.tree();
        let mut nodes: Vec<TreeNodeView> = tree
            .checkpoints()
            .iter()
            .map(|cp| TreeNodeView {
                id: format!("cp{}", cp.id().as_u64()),
                parent: cp
                    .closest_live_ancestor()
                    .map(|p| format!("cp{}", p.id().as_u64())),
                time_secs: cp.time().as_secs_f64(),
                kind: "checkpoint",
            })
            .collect();
        nodes.extend(tree.branches().iter().map(|b| TreeNodeView {
            id: format!("br{}", b.id.as_u64()),
            parent: Some(format!("cp{}", b.origin.as_u64())),
            time_secs: b.current_time.as_secs_f64(),
            kind: "branch",
        }));
        json_string(&TreeView { nodes })
    }

    /// `/tree?format=ascii`
    pub fn tree_ascii(&self) -> String {
        self.tree_anchor.tree().ascii()
    }

    /// `/tree?format=dot`
    pub fn tree_dot(&self) -> String {
        self.tree_anchor.tree().dot()
    }

    /// `/corpus[?since=N]`
    pub fn corpus_json(&self, since: usize) -> String {
        let corpus = self.corpus.lock().unwrap();
        let views: Vec<CorpusEntryView> = corpus
            .iter()
            .enumerate()
            .skip(since)
            .map(|(id, n)| CorpusEntryView {
                id,
                parent: n.parent,
                checkpoint: n.checkpoint.id().as_u64(),
                time_secs: n.checkpoint.time().as_secs_f64(),
                runtime_secs: n.runtime_secs,
                scheduled: n.scheduled,
                novelty: n.novelty,
                serial_lines: n.serial_lines,
                bug: n.bug,
            })
            .collect();
        json_string(&views)
    }

    /// `/corpus/{id}` — serial is read from disk (`--serial-dir`) on demand, or
    /// a one-line note if none was retained.
    pub fn corpus_entry_json(&self, id: usize) -> Option<String> {
        let (parent, checkpoint, time_secs, runtime_secs, scheduled, novelty) = {
            let corpus = self.corpus.lock().unwrap();
            let n = corpus.get(id)?;
            (
                n.parent,
                n.checkpoint.id().as_u64(),
                n.checkpoint.time().as_secs_f64(),
                n.runtime_secs,
                n.scheduled,
                n.novelty,
            )
        };
        Some(json_string(&CorpusDetail {
            id,
            parent,
            checkpoint,
            time_secs,
            runtime_secs,
            scheduled,
            novelty,
            serial: self.checkpoint_serial(checkpoint),
        }))
    }

    /// Read a checkpoint's serial from `--serial-dir`, or a single note line
    /// explaining none is available.
    fn checkpoint_serial(&self, checkpoint: u64) -> Vec<String> {
        let Some(dir) = &self.cfg.serial_dir else {
            return vec!["(no serial retained — run flux with --serial-dir to capture per-checkpoint serial)".to_string()];
        };
        match std::fs::read_to_string(format!("{}/cp{checkpoint}.log", dir.display())) {
            Ok(s) => s.lines().map(str::to_string).collect(),
            Err(_) => vec![format!("(no serial on disk for cp{checkpoint})")],
        }
    }

    /// `/solutions[?since=N]`
    pub fn solutions_json(&self, since: usize) -> String {
        let sols = self.solutions.lock().unwrap();
        let views: Vec<SolutionView> = sols
            .iter()
            .enumerate()
            .skip(since)
            .map(|(id, s)| SolutionView {
                id,
                parent: s.parent,
                reason: s.reason.clone(),
                serial_lines: s.serial.len(),
            })
            .collect();
        json_string(&views)
    }

    /// `/solutions/{id}`
    pub fn solution_json(&self, id: usize) -> Option<String> {
        let sols = self.solutions.lock().unwrap();
        let s = sols.get(id)?;
        Some(json_string(&SolutionDetail {
            id,
            parent: s.parent,
            reason: s.reason.clone(),
            checkpoint: s.checkpoint.id().as_u64(),
            time_secs: s.checkpoint.time().as_secs_f64(),
            serial: s.serial.clone(),
        }))
    }
}

/// The product of one branch run, consumed by the worker's novelty/objective
/// checks and corpus-add.
struct RunResult {
    crashed: bool,
    new_cp: Checkpoint,
    intermediate: Option<Checkpoint>,
    recording: InputRecording,
    serial: Vec<String>,
    coverage: RunBitmap,
    vt: u64,
}

/// Convert the lab's `InputRecording` into an [`Input`] anchored at the new
/// checkpoint's time, so future mutations operate on what the guest consumed.
fn recording_to_input(recording: &InputRecording, anchor_instr: u64) -> Input {
    Input {
        rng: recording
            .rng_inputs()
            .iter()
            .map(|r| crate::input::RngVal {
                at: r.at.instructions(),
                value: r.value,
            })
            .collect(),
        io: recording
            .io_inputs()
            .iter()
            .map(|i| IoAction {
                at: i.at.instructions(),
                target: Target::from(&i.target),
                command: i.command.clone(),
            })
            .collect(),
        anchor_at: anchor_instr,
        mutated_at: None,
    }
}

/// Scheduled IO for the quiesce stage anchored at virtual time `anchor`: a
/// kill-drivers barrier, then a `fault-injector clear`. Spaced a third of
/// `window` apart so the kill's drain-wait completes before the clear fires
/// (with a final third of headroom before the checkpoint). `at`s are strictly
/// increasing and `>= anchor`.
fn quiesce_io(anchor: u64, window: u64) -> Vec<IoAction> {
    let gap = (window / 3).max(1);
    vec![
        IoAction {
            at: anchor.saturating_add(gap),
            target: Target::KillDrivers,
            command: String::new(),
        },
        IoAction {
            at: anchor.saturating_add(gap.saturating_mul(2)),
            target: Target::Host,
            command: "fault-injector clear".to_string(),
        },
    ]
}

/// Scheduled IO for one eventually driver, anchored at virtual time `anchor`:
/// fire it a quarter into `window` so it has the rest of the branch to run to
/// completion before the checkpoint.
fn eventually_driver_io(anchor: u64, window: u64, d: &WorkloadDriver) -> Vec<IoAction> {
    vec![IoAction {
        at: anchor.saturating_add((window / 4).max(1)),
        target: Target::Container(d.container.clone()),
        command: d.driver.clone(),
    }]
}

/// Run the branch until it reaches `target_time` or yields with an unexpected
/// exit. Returns `true` if it crashed. `on_progress` is called with the
/// branch's virtual time after each step for the live frontier view.
fn pump_branch(
    branch: &mut Branch,
    target_time: VirtTime,
    mut on_progress: impl FnMut(VirtTime),
) -> bool {
    loop {
        let (at, outcome) = match branch.run_until(target_time) {
            Ok(x) => x,
            Err(_) => return false,
        };
        on_progress(at);
        match outcome {
            RunOutcome::ReachedTime | RunOutcome::RngExhausted => return false,
            RunOutcome::ActionResponse { .. } | RunOutcome::Ready => continue,
            RunOutcome::Yielded { .. } => return true,
        }
    }
}

/// Inspect captured serial for the one bug signal flux acts on: a failed
/// `Always` assertion. Each assertion is emitted on a serial line rendered
/// `[assertions] | {json}`; this returns the message of the first `Always`
/// record whose `result` is false. `Sometimes` records and passing
/// (`result: true`) records are ignored. The message is used verbatim as the
/// solution reason, so findings dedup by message. Shared by the live objective
/// check and `--reproduce`.
pub fn assertion_failure_reason(serial: &[String]) -> Option<String> {
    serial.iter().find_map(|line| failed_always_message(line))
}

/// The message of a failed `Always` assertion carried by one serial line, if
/// any. The trailing JSON object on the line is parsed as an [`Assertion`];
/// returns `None` for lines without a parseable assertion record, for
/// `Sometimes` records, and for assertions that held.
fn failed_always_message(line: &str) -> Option<String> {
    let stripped = strip_ansi(line);
    // The assertion record is the trailing JSON object; the formatter prefix
    // (`[assertions] | `) and flux's branch/vt prefix contain no `{`.
    let json = stripped.get(stripped.find('{')?..)?;
    match serde_json::from_str::<Assertion>(json).ok()? {
        Assertion::Always(data) if !data.result => Some(data.message),
        _ => None,
    }
}

/// Outcome of replaying a [`Reproduction`].
pub struct ReplayOutcome {
    /// Full guest serial captured from the fuzzing root to the bug.
    pub serial: Vec<String>,
    /// Whether the guest yielded on an unexpected exit during replay.
    pub crashed: bool,
    /// Virtual time (instructions) the replay branch reached.
    pub end_instr: u64,
}

/// Replay a [`Reproduction`] from `seed_cp` (the discovery checkpoint) in a
/// single branch: serve the recorded input suffix from the fuzzing root and run
/// to just past the bug, capturing the complete serial. Deterministic — the
/// recorded `RDRAND`/IO stream drives the guest down the exact same path. The
/// `sink` must be in fuzz mode so the branch's serial is captured.
pub fn replay(seed_cp: &Checkpoint, repro: &Reproduction, sink: &Sink) -> Option<ReplayOutcome> {
    let source = repro
        .input
        .source_from(repro.root_instr, repro.frequency, 0);
    let mut branch = seed_cp.branch_with_input_source(source).ok()?;
    let bid = branch.id();
    sink.start_capture(bid);
    // Run a little past the bug checkpoint so a `container died` event that
    // trails the crash by a moment still lands in the capture.
    let margin = repro.frequency.saturating_mul(3);
    let target =
        VirtTime::from_instructions(repro.bug_instr.saturating_add(margin), repro.frequency);
    let crashed = pump_branch(&mut branch, target, |_| {});
    let serial = sink.take_capture(bid);
    let end_instr = branch.current_time().instructions();
    Some(ReplayOutcome {
        serial,
        crashed,
        end_instr,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        assertion_failure_reason, eventually_driver_io, failed_always_message, quiesce_io, Target,
    };
    use bedrock_lab::WorkloadDriver;

    #[test]
    fn quiesce_io_is_kill_then_clear_within_window() {
        let anchor = 1_000;
        let window = 3_000_000;
        let io = quiesce_io(anchor, window);
        assert_eq!(io.len(), 2);
        // Kill first (payload-less control action), then clear.
        assert_eq!(io[0].target, Target::KillDrivers);
        assert!(io[0].command.is_empty());
        assert_eq!(io[1].target, Target::Host);
        assert_eq!(io[1].command, "fault-injector clear");
        // Strictly increasing, at/after the anchor, with headroom before window end.
        assert!(io[0].at >= anchor && io[0].at < io[1].at);
        assert!(
            io[1].at < anchor + window,
            "clear leaves headroom before the checkpoint"
        );
    }

    #[test]
    fn eventually_driver_io_fires_one_driver_early() {
        let d = WorkloadDriver {
            container: "lnd1".into(),
            driver: "/opt/bedrock/drivers/eventually_b".into(),
        };
        let anchor = 500;
        let window = 4_000_000;
        let io = eventually_driver_io(anchor, window, &d);
        assert_eq!(io.len(), 1);
        assert_eq!(io[0].target, Target::Container("lnd1".into()));
        assert_eq!(io[0].command, "/opt/bedrock/drivers/eventually_b");
        // Fires after the anchor but with most of the window left to complete.
        assert!(io[0].at > anchor && io[0].at < anchor + window / 2);
    }

    #[test]
    fn quiesce_io_handles_zero_window_without_panicking() {
        // Defensive: a tiny/zero window still yields kill+clear with a sane
        // (>=1) gap and strictly increasing times.
        let io = quiesce_io(0, 0);
        assert_eq!(io.len(), 2);
        assert!(io[0].at < io[1].at);
    }

    /// A serial line as flux captures it: branch/vt prefix plus the
    /// `[assertions]` formatter output carrying one assertion record.
    fn line(json: &str) -> String {
        format!("[br BranchId(7) vt  12.345] [assertions] | {json}")
    }

    const FAILED_CONTAINER: &str = r#"{"Always":{"condition":{"Eq":{"x":137,"y":0}},"result":false,"message":"container btcd1 exit code is zero","location":{"file":"m.rs","line":1,"column":1}}}"#;

    #[test]
    fn failed_always_reports_its_message() {
        assert_eq!(
            failed_always_message(&line(FAILED_CONTAINER)).as_deref(),
            Some("container btcd1 exit code is zero")
        );
    }

    #[test]
    fn passing_always_is_not_a_bug() {
        let l = line(
            r#"{"Always":{"condition":{"Eq":{"x":0,"y":0}},"result":true,"message":"container btcd1 exit code is zero","location":{"file":"m.rs","line":1,"column":1}}}"#,
        );
        assert_eq!(failed_always_message(&l), None);
    }

    #[test]
    fn failed_sometimes_is_not_a_bug() {
        let l = line(
            r#"{"Sometimes":{"condition":{"Eq":{"x":1,"y":0}},"result":false,"message":"x reached zero","location":{"file":"m.rs","line":1,"column":1}}}"#,
        );
        assert_eq!(failed_always_message(&l), None);
    }

    #[test]
    fn non_assertion_lines_are_ignored() {
        assert_eq!(
            failed_always_message("[br BranchId(1) vt 1.0] [btcd1] | starting node {ok}"),
            None
        );
    }

    #[test]
    fn scan_returns_first_failure_message_as_reason() {
        let serial = vec![
            line(
                r#"{"Always":{"condition":{"Eq":{"x":0,"y":0}},"result":true,"message":"ok","location":{"file":"m.rs","line":1,"column":1}}}"#,
            ),
            line(
                r#"{"Always":{"condition":{"Eq":{"x":2,"y":0}},"result":false,"message":"container lnd1 exit code is zero","location":{"file":"m.rs","line":1,"column":1}}}"#,
            ),
        ];
        // The reason is the message verbatim — that is the per-message dedup key.
        assert_eq!(
            assertion_failure_reason(&serial).as_deref(),
            Some("container lnd1 exit code is zero")
        );
    }
}
