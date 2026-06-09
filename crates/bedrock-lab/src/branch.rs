// SPDX-License-Identifier: GPL-2.0

//! Branches — live lines of execution.

use std::collections::VecDeque;
use std::sync::Arc;

use bedrock_vm::{ExitKind, ExitStats, LogConfig, LogEntry, Vm, VmError};

use crate::bash::{self, ActionResponse, BashOutput, BashTarget, WorkloadDetails};
use crate::checkpoint::{Checkpoint, CheckpointId, CheckpointInner};
use crate::error::{LabError, Result};
use crate::event::{drain_serial_into_sink, emit_feedback_buffer_registered, Event, PartialLine};
use crate::inner::{BranchMeta, LabInner};
use crate::rng::{InputRecording, InputSource, IoInput, RandomInput, RngInput};
use crate::time::VirtTime;
use crate::tree::Tree;

/// A stable identifier for a branch within its tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BranchId(pub(crate) u64);

impl BranchId {
    /// The raw numeric id, for serialization or display by external tools.
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// The outcome of a [`Branch::run_until`] call.
///
/// Returned alongside the [`VirtTime`] at which the branch paused — see
/// [`Branch::run_until`]'s return signature.
#[derive(Debug, Clone)]
pub enum RunOutcome {
    /// The branch reached the requested virtual time.
    ReachedTime,
    /// The guest signaled it has finished boot/initialization and is ready
    /// for host-driven workload (VMCALL with the ready hypercall).
    Ready,
    /// A scheduled I/O action's response arrived. The branch is paused at
    /// the moment the response landed; call `run_until` again to keep going.
    ActionResponse { response: ActionResponse },
    /// The guest executed `RDRAND`/`RDSEED` and the attached [`InputSource`](crate::InputSource)
    /// returned `None` — out of randomness. The branch is paused on the trapping instruction;
    /// Calling `run_until` again will just re-trap on the same instruction.
    RngExhausted,
    /// The VM exited for a reason the lab did not handle internally.
    Yielded { kind: ExitKind },
}

/// A live line of execution descending from a [`Checkpoint`].
///
/// `Branch` is an owning, single-driver handle: it cannot be cloned, and
/// execution-advancing methods take `&mut self`. To preserve a moment in time
/// for later forking or rewinding, call [`Branch::checkpoint`] — that consumes
/// the branch and returns a [`Checkpoint`] you can branch off of again or
/// [`Checkpoint::rewind`] from.
pub struct Branch {
    id: BranchId,
    origin: Checkpoint,
    /// `Some` while the branch is live. `None` only during `checkpoint(self)`
    /// after the VM has been moved into the new checkpoint; the value is
    /// dropped at end of scope without `Drop for Branch` needing to do
    /// anything.
    vm: Option<Vm>,
    current_time: VirtTime,
    lab: Arc<LabInner>,
    /// Bytes of the current serial line not yet terminated by `\n`. Seeded
    /// from the origin checkpoint so a line that straddles
    /// `Branch::checkpoint` is emitted as a single `Event::SerialLine`.
    partial: PartialLine,
    /// This branch's private clone of the tree's userspace input source.
    /// `Some` only when the tree was built with an input source. Moves into
    /// the new checkpoint on [`Branch::checkpoint`] so descendant branches
    /// start from the post-consumption state.
    input_source: Option<Box<dyn InputSource>>,
    /// Next source-provided I/O action not yet queued because it is beyond
    /// the current run target or the VM queue was full.
    pending_input_io: Option<IoInput>,
    /// True once `input_source.next_io_input()` has returned `None`.
    input_io_exhausted: bool,
    /// Inputs consumed along this branch's path.
    input_recording: InputRecording,
    /// Last value passed to `vm.set_stop_at_tsc`. `None` means the VM's
    /// current stop_at_tsc setting is unknown (post-fork, or never set on
    /// this branch); the next `set_stop_at` call always sends an ioctl.
    last_stop_at: Option<Option<u64>>,
    /// Monotonically incrementing tag attached to every I/O request the
    /// branch submits. The kernel module echoes this tag back in the
    /// response so synchronous calls can pick their specific reply out
    /// of the I/O channel even when other workers complete out of order.
    next_request_id: u32,
    /// Responses that arrived while a synchronous `run_io_action` was
    /// waiting for a different `request_id`. Drained FIFO from
    /// `run_until` before the VM is run again, so external observers
    /// still see every response — just delivered through `run_until`
    /// rather than through the synchronous call that "skipped past"
    /// them.
    pending_responses: VecDeque<(VirtTime, ActionResponse)>,
}

impl Branch {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        id: BranchId,
        origin: Checkpoint,
        vm: Vm,
        current_time: VirtTime,
        lab: Arc<LabInner>,
        partial: PartialLine,
        input_source: Option<Box<dyn InputSource>>,
        pending_input_io: Option<IoInput>,
        input_io_exhausted: bool,
        input_recording: InputRecording,
    ) -> Self {
        lab.live_branches.lock().unwrap().insert(
            id,
            BranchMeta {
                id,
                origin: origin.id(),
                current_time,
            },
        );
        let origin_id = origin.id();
        let branch = Self {
            id,
            origin,
            vm: Some(vm),
            current_time,
            lab: lab.clone(),
            partial,
            input_source,
            pending_input_io,
            input_io_exhausted,
            input_recording,
            last_stop_at: None,
            next_request_id: 1,
            pending_responses: VecDeque::new(),
        };
        lab.sink.on_event(Event::BranchCreated {
            branch: id,
            origin: origin_id,
            at: current_time,
        });
        branch
    }

    fn vm_mut(&mut self) -> &mut Vm {
        self.vm.as_mut().expect("Branch.vm taken")
    }

    fn vm(&self) -> &Vm {
        self.vm.as_ref().expect("Branch.vm taken")
    }

    pub fn id(&self) -> BranchId {
        self.id
    }

    /// The branch's current virtual time (the emulated TSC of its VM).
    pub fn current_time(&self) -> VirtTime {
        self.current_time
    }

    pub fn tsc_frequency(&self) -> u64 {
        self.lab.tsc_frequency
    }

    /// Per-exit-reason counts and guest/overhead cycle totals accumulated by
    /// this branch's VM so far. Forked VMs start with fresh stats, so for a
    /// branch read after a run this reflects that run. Diagnostic only.
    pub fn exit_stats(&self) -> std::io::Result<ExitStats> {
        self.vm().get_exit_stats()
    }

    /// The checkpoint this branch was forked from. Fixed for the lifetime of
    /// the branch.
    pub fn origin(&self) -> &Checkpoint {
        &self.origin
    }

    /// Configure deterministic exit logging for this branch.
    ///
    /// When enabled, every covered VM exit is captured as a [`LogEntry`]
    /// (guest registers + device state hashes) and forwarded to the tree's
    /// [`EventSink`](crate::EventSink) as [`Event::ExitLogged`]. Diffing two
    /// runs' exit streams pinpoints where they diverged, which is the main
    /// non-determinism debugging primitive.
    ///
    /// Forked VMs start with logging disabled regardless of the parent's
    /// setting, so each branch must enable logging explicitly. See
    /// [`LogConfig`] for the available modes ([`LogMode::AllExits`](crate::LogMode::AllExits),
    /// [`AtTsc`](crate::LogMode::AtTsc), [`Checkpoints`](crate::LogMode::Checkpoints),
    /// [`TscRange`](crate::LogMode::TscRange)).
    pub fn set_log_config(&mut self, config: LogConfig) -> Result<()> {
        self.vm_mut().set_log_config(&config).map_err(|source| {
            LabError::Vm(VmError::Ioctl {
                operation: "SET_LOG_CONFIG",
                source,
            })
        })?;
        Ok(())
    }

    /// Disable exit logging on this branch and release the kernel log
    /// buffer. Equivalent to `set_log_config(LogConfig::disabled())`.
    pub fn disable_logging(&mut self) -> Result<()> {
        self.set_log_config(LogConfig::disabled())
    }

    /// Enable single-step (MTF) execution within the half-open virtual
    /// time range `[start, end)` and log every exit that fires inside it.
    ///
    /// The kernel sets the VMCS Monitor-Trap-Flag whenever
    /// `emulated_tsc ∈ [start, end)`, so the guest exits after every
    /// retired instruction in that window. Combined with
    /// [`LogMode::TscRange`](crate::LogMode::TscRange) logging, this gives
    /// an instruction-by-instruction trace of guest state — the highest-
    /// resolution divergence-debugging tool available.
    ///
    /// Single-stepping is expensive (~1 vmexit per guest instruction);
    /// pick the smallest range that brackets the suspected divergence
    /// point. Disable with [`Self::disable_single_step`] when done.
    pub fn single_step(&mut self, start: VirtTime, end: VirtTime) -> Result<()> {
        self.check_freq(start.frequency())?;
        self.check_freq(end.frequency())?;
        if end < start {
            return Err(LabError::TargetInPast {
                current: start,
                target: end,
            });
        }
        self.vm_mut()
            .set_single_step_range(start.instructions(), end.instructions())
            .map_err(|source| {
                LabError::Vm(VmError::Ioctl {
                    operation: "SET_SINGLE_STEP",
                    source,
                })
            })?;
        // Memory hashing on every single-stepped instruction would dominate
        // run time and adds no signal — register state already pins down
        // divergence at instruction granularity.
        self.set_log_config(LogConfig::tsc_range().with_no_memory_hash())
    }

    /// Disable single-step execution and release the log buffer.
    pub fn disable_single_step(&mut self) -> Result<()> {
        self.vm_mut().disable_single_step().map_err(|source| {
            LabError::Vm(VmError::Ioctl {
                operation: "SET_SINGLE_STEP",
                source,
            })
        })?;
        self.disable_logging()
    }

    fn check_freq(&self, freq: u64) -> Result<()> {
        if freq != self.lab.tsc_frequency {
            return Err(LabError::FrequencyMismatch {
                lhs: freq,
                rhs: self.lab.tsc_frequency,
            });
        }
        Ok(())
    }

    /// Wrap `vm.set_stop_at_tsc` with a cache so we skip the ioctl when the
    /// value hasn't changed. Branch::run_until calls this every loop
    /// iteration; without the cache that's one extra ioctl per VM exit.
    fn set_stop_at(&mut self, value: Option<u64>) -> Result<()> {
        if self.last_stop_at == Some(value) {
            return Ok(());
        }
        self.vm_mut().set_stop_at_tsc(value).map_err(|source| {
            LabError::Vm(VmError::Ioctl {
                operation: "SET_STOP_TSC",
                source,
            })
        })?;
        self.last_stop_at = Some(value);
        Ok(())
    }

    /// Update self.current_time and mirror it into the lab's live-branch map
    /// so tree views stay in sync.
    fn advance_time(&mut self, t: VirtTime) {
        self.current_time = t;
        if let Some(m) = self.lab.live_branches.lock().unwrap().get_mut(&self.id) {
            m.current_time = t;
        }
    }

    fn drain_serial(&mut self, serial_len: usize, exit_at: VirtTime) {
        drain_serial_into_sink(
            self.vm.as_ref().expect("Branch.vm taken"),
            serial_len,
            exit_at,
            self.id,
            self.lab.sink.as_ref(),
            &mut self.partial,
        );
    }

    /// Forward newly-written exit-log entries to the sink as
    /// [`Event::ExitLogged`]. `count` is `VmExit::log_entry_count` from the
    /// just-returned `vm.run()` ioctl.
    ///
    /// The kernel resets `log_entry_count` to 0 at the start of every
    /// `vm.run()` ioctl (`handlers.rs` `log_clear`), so `count` is
    /// *per-call*, not cumulative — emit `entries[0..count]` and trust the
    /// next ioctl to reset.
    fn drain_log_entries(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        let vm = self.vm.as_ref().expect("Branch.vm taken");
        let Some(buffer) = vm.log_buffer() else {
            return;
        };
        for entry in LogEntry::from_buffer(buffer, count) {
            self.lab.sink.on_event(Event::ExitLogged {
                branch: self.id,
                entry,
            });
        }
    }

    /// Read the guest GPRs after a successful `HYPERCALL_REGISTER_FEEDBACK_BUFFER`
    /// exit and emit an [`Event::FeedbackBufferRegistered`]. The run loop
    /// transparently continues after this — registrations are surfaced only
    /// as events, never as a [`RunOutcome`].
    ///
    /// The kernel-side handler only returns this exit when registration
    /// succeeds; failure cases are swallowed as `Continue` (see
    /// `crates/bedrock-vmx/src/exits/vmcall.rs`).
    fn on_feedback_buffer_registered(&mut self, at: VirtTime) -> Result<()> {
        emit_feedback_buffer_registered(
            self.vm.as_ref().expect("Branch.vm taken"),
            at,
            self.id,
            self.lab.sink.as_ref(),
        )?;
        Ok(())
    }

    /// Read every feedback buffer this branch's VM has registered under
    /// `id`. Returns one `&[u8]` per matching slot, in ascending slot
    /// order. Empty result if no registration matches.
    ///
    /// IDs are not unique by design (see [`Event::FeedbackBufferRegistered`](crate::Event)
    /// docs): multiple guest processes can register coverage maps under the
    /// same id (typically a build-id) and the caller is responsible for
    /// merging — usually a byte-wise OR — the resulting slices.
    ///
    /// Each backing slot is lazily mmapped on first read and the mapping is
    /// cached for the branch's lifetime. The slices stay valid until the
    /// branch is dropped or consumed by [`Branch::checkpoint`]. Forked
    /// branches see their own copy-on-write view of every buffer, so reads
    /// from sibling branches are independent.
    ///
    /// # Errors
    ///
    /// - The mmap or info-query ioctl fails
    pub fn feedback_buffers(&mut self, id: &[u8]) -> Result<Vec<&[u8]>> {
        let vm = self.vm.as_mut().expect("Branch.vm taken");
        let slots = vm.feedback_buffer_slots_for_id(id)?;
        for &slot in &slots {
            if vm.feedback_buffer_at(slot).is_none() {
                vm.map_feedback_buffer_at(slot)?;
            }
        }
        // Re-borrow to get the slices now that all mappings exist. Done in a
        // second loop so the mutable borrow above is released before we hand
        // out shared references.
        let mut out = Vec::with_capacity(slots.len());
        for &slot in &slots {
            if let Some(bytes) = vm.feedback_buffer_at(slot) {
                out.push(bytes);
            }
        }
        Ok(out)
    }

    /// Convenience: read every feedback buffer matching `id` into owned
    /// `Vec`s. Useful when the caller needs to hold the bytes across other
    /// `&mut self` operations on the branch.
    pub fn feedback_buffers_to_vec(&mut self, id: &[u8]) -> Result<Vec<Vec<u8>>> {
        Ok(self
            .feedback_buffers(id)?
            .into_iter()
            .map(|s| s.to_vec())
            .collect())
    }

    /// Return every distinct identifier currently registered on this
    /// branch's VM, in slot-ascending order (first time each id is seen).
    ///
    /// Issues one info-query ioctl per slot. Cheap but not free; cache the
    /// result if you call it on a hot path.
    pub fn feedback_buffer_ids(&self) -> Result<Vec<Vec<u8>>> {
        let vm = self.vm.as_ref().expect("Branch.vm taken");
        let mut seen = std::collections::HashSet::new();
        let mut ids = Vec::new();
        for slot in 0..bedrock_vm::MAX_FEEDBACK_BUFFERS {
            if let Some(info) = vm.get_feedback_buffer_info_at(slot)? {
                let id = info.id_bytes().to_vec();
                if seen.insert(id.clone()) {
                    ids.push(id);
                }
            }
        }
        Ok(ids)
    }

    /// Run the branch forward until its virtual time reaches `target`.
    ///
    /// Returns the [`VirtTime`] at which the branch is now paused, together
    /// with the [`RunOutcome`] that describes *why* it paused.
    ///
    /// Errors with [`LabError::TargetInPast`] if `target` is earlier than
    /// [`Branch::current_time`]. To move backward, take a [`Checkpoint`] via
    /// [`Branch::checkpoint`] and call [`Checkpoint::rewind`] on it.
    pub fn run_until(&mut self, target: VirtTime) -> Result<(VirtTime, RunOutcome)> {
        self.check_freq(target.frequency())?;
        if target < self.current_time {
            return Err(LabError::TargetInPast {
                current: self.current_time,
                target,
            });
        }
        // Deliver any responses that arrived while a synchronous call
        // was waiting for a different `request_id`. Order is preserved
        // — these are the responses the VM produced before the caller
        // started this `run_until`.
        if let Some((at, response)) = self.pending_responses.pop_front() {
            return Ok((at, RunOutcome::ActionResponse { response }));
        }
        if target == self.current_time {
            return Ok((target, RunOutcome::ReachedTime));
        }

        loop {
            let stop_at = self.prepare_next_io_input(target)?;
            self.set_stop_at(Some(stop_at.instructions()))?;
            let exit = self.vm_mut().run().map_err(|source| {
                LabError::Vm(VmError::Ioctl {
                    operation: "RUN",
                    source,
                })
            })?;
            let at = VirtTime::from_instructions(exit.emulated_tsc, self.lab.tsc_frequency);
            self.advance_time(at);
            self.drain_serial(exit.serial_len as usize, at);
            self.drain_log_entries(exit.log_entry_count as usize);
            match exit.kind() {
                ExitKind::StopTscReached => {
                    if at >= target {
                        return Ok((at, RunOutcome::ReachedTime));
                    }
                    continue;
                }
                ExitKind::VmcallReady => return Ok((at, RunOutcome::Ready)),
                ExitKind::IoResponse => {
                    let bytes = self.vm_mut().drain_io_response().map_err(|source| {
                        LabError::Vm(VmError::Ioctl {
                            operation: "DRAIN_IO_RESPONSE",
                            source,
                        })
                    })?;
                    let (_request_id, response) =
                        bash::decode_response(&bytes).map_err(LabError::BadResponse)?;
                    return Ok((at, RunOutcome::ActionResponse { response }));
                }
                ExitKind::FeedbackBufferRegistered => {
                    self.on_feedback_buffer_registered(at)?;
                    continue;
                }
                ExitKind::Rdrand | ExitKind::Rdseed => match self.feed_rng()? {
                    FeedRng::Fed => continue,
                    FeedRng::Exhausted => return Ok((at, RunOutcome::RngExhausted)),
                    FeedRng::NoSource => {
                        return Ok((at, RunOutcome::Yielded { kind: exit.kind() }))
                    }
                },
                ExitKind::VmcallGetRandom => match self.feed_random()? {
                    FeedRng::Fed => continue,
                    FeedRng::Exhausted => return Ok((at, RunOutcome::RngExhausted)),
                    FeedRng::NoSource => {
                        return Ok((at, RunOutcome::Yielded { kind: exit.kind() }))
                    }
                },
                ExitKind::Continue | ExitKind::LogBufferFull => continue,
                kind => return Ok((at, RunOutcome::Yielded { kind })),
            }
        }
    }

    /// Allocate the next request tag and advance the counter. Wraps at
    /// 2^32; in practice we never queue 4 billion outstanding actions.
    fn next_request_id(&mut self) -> u32 {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        id
    }

    /// Queue an I/O action tagged with `expected_id` and pump the VM
    /// until the matching response arrives. Returns the raw response
    /// bytes for the caller to decode. Responses for other in-flight
    /// requests that arrive in the meantime are stashed in
    /// `pending_responses` and delivered through subsequent
    /// `run_until` calls — no response is dropped, just routed past
    /// this synchronous wait.
    fn run_io_action(&mut self, request: &[u8], expected_id: u32) -> Result<Vec<u8>> {
        // Run unbounded — any leftover stop_at_tsc from a previous run_until
        // could otherwise fire before the I/O response lands.
        self.set_stop_at(None)?;
        self.vm_mut()
            .queue_io_action(request, 0)
            .map_err(|source| {
                LabError::Vm(VmError::Ioctl {
                    operation: "QUEUE_IO_ACTION",
                    source,
                })
            })?;
        loop {
            let exit = self.vm_mut().run().map_err(|source| {
                LabError::Vm(VmError::Ioctl {
                    operation: "RUN",
                    source,
                })
            })?;
            let at = VirtTime::from_instructions(exit.emulated_tsc, self.lab.tsc_frequency);
            self.advance_time(at);
            self.drain_serial(exit.serial_len as usize, at);
            self.drain_log_entries(exit.log_entry_count as usize);
            match exit.kind() {
                ExitKind::IoResponse => {
                    let bytes = self.vm_mut().drain_io_response().map_err(|source| {
                        LabError::Vm(VmError::Ioctl {
                            operation: "DRAIN_IO_RESPONSE",
                            source,
                        })
                    })?;
                    let (rid, response) =
                        bash::decode_response(&bytes).map_err(LabError::BadResponse)?;
                    if rid == expected_id {
                        return Ok(bytes);
                    }
                    // A response for a different in-flight request
                    // landed first (workers complete out of order on
                    // the I/O channel). Stash it for later delivery
                    // through `run_until` and keep pumping.
                    self.pending_responses.push_back((at, response));
                    continue;
                }
                ExitKind::FeedbackBufferRegistered => {
                    self.on_feedback_buffer_registered(at)?;
                    continue;
                }
                ExitKind::Rdrand | ExitKind::Rdseed => match self.feed_rng()? {
                    FeedRng::Fed => continue,
                    FeedRng::Exhausted | FeedRng::NoSource => {
                        return Err(LabError::UnexpectedExit {
                            at,
                            kind: exit.kind(),
                        })
                    }
                },
                ExitKind::VmcallGetRandom => match self.feed_random()? {
                    FeedRng::Fed => continue,
                    FeedRng::Exhausted | FeedRng::NoSource => {
                        return Err(LabError::UnexpectedExit {
                            at,
                            kind: exit.kind(),
                        })
                    }
                },
                ExitKind::Continue | ExitKind::LogBufferFull | ExitKind::VmcallReady => continue,
                kind => return Err(LabError::UnexpectedExit { at, kind }),
            }
        }
    }

    /// If this branch has a userspace input source, pull the next RNG `u64`
    /// from it and feed it to the VM via `SET_RDRAND_VALUE` so the next
    /// `vm.run()` re-executes the trapped `RDRAND`/`RDSEED` with that
    /// value. See [`FeedRng`] for the three possible outcomes.
    fn feed_rng(&mut self) -> Result<FeedRng> {
        let Some(source) = self.input_source.as_mut() else {
            return Ok(FeedRng::NoSource);
        };
        let Some(value) = source.next_rng_u64() else {
            return Ok(FeedRng::Exhausted);
        };
        self.vm_mut().set_rdrand_value(value).map_err(|source| {
            LabError::Vm(VmError::Ioctl {
                operation: "SET_RDRAND_VALUE",
                source,
            })
        })?;
        self.input_recording.push_rng(RngInput {
            at: self.current_time,
            value,
        });
        Ok(FeedRng::Fed)
    }

    /// Service a `HYPERCALL_GET_RANDOM` exit: read the pending request (PID +
    /// length), pull that many bytes from the input source, hand them to the
    /// guest via `SET_RANDOM_BYTES`, and record them for replay. This is the
    /// `/dev/urandom` / `getrandom()` analogue of [`Self::feed_rng`] — but the
    /// bytes are served whole per request (the guest tells us the exact size),
    /// not one `u64` at a time. Returns [`FeedRng::NoSource`] when the branch
    /// has no input source (e.g. seeded mode never exits here, so this is only
    /// a defensive fallback).
    fn feed_random(&mut self) -> Result<FeedRng> {
        let req = self.vm_mut().random_request().map_err(|source| {
            LabError::Vm(VmError::Ioctl {
                operation: "GET_RANDOM_REQUEST",
                source,
            })
        })?;
        let len = req.len as usize;
        let pid = req.pid;

        // Scope the source borrow so we can take `&mut self.vm` afterwards.
        let bytes = {
            let Some(source) = self.input_source.as_mut() else {
                return Ok(FeedRng::NoSource);
            };
            source.next_random(len, pid)
        };

        self.vm_mut().set_random_bytes(&bytes).map_err(|source| {
            LabError::Vm(VmError::Ioctl {
                operation: "SET_RANDOM_BYTES",
                source,
            })
        })?;
        self.input_recording.push_random(RandomInput {
            at: self.current_time,
            pid,
            bytes,
        });
        Ok(FeedRng::Fed)
    }

    /// Pull one source-provided bash action, stop at its virtual time, and
    /// queue it as an immediate I/O action once that time is reached.
    ///
    /// After queueing, peek the *next* source input (without consuming) and
    /// return its virtual time as the VM run-loop stop hint. The next
    /// `StopTscReached` exit then re-enters this method so the next action
    /// can be queued. Two actions at the same virtual time therefore both
    /// enter the kernel worker pool: the first iteration queues A and sets
    /// stop_at to B.at; the second iteration fires immediately (since
    /// B.at == current_time) and queues B.
    fn prepare_next_io_input(&mut self, target: VirtTime) -> Result<VirtTime> {
        if self.input_io_exhausted {
            return Ok(target);
        }

        if self.pending_input_io.is_none() {
            let Some(source) = self.input_source.as_mut() else {
                return Ok(target);
            };
            self.pending_input_io = source.next_io_input();
            if self.pending_input_io.is_none() {
                self.input_io_exhausted = true;
                return Ok(target);
            }
        }

        let input = self
            .pending_input_io
            .as_ref()
            .expect("pending_input_io was set above");
        self.check_freq(input.at.frequency())?;
        if input.at > target {
            return Ok(target);
        }
        if input.at > self.current_time {
            return Ok(input.at);
        }

        let input = self
            .pending_input_io
            .take()
            .expect("pending_input_io was checked above");
        // Scheduled actions get a fresh request_id like any other
        // submission; nothing on the lab side correlates against it
        // here (responses flow through `run_until` as `ActionResponse`
        // regardless), but it has to be unique so a coincident
        // synchronous `bash`/`workload_details` doesn't claim this
        // response as its own.
        let request_id = self.next_request_id();
        let request = bash::encode_bash_request(input.target.clone(), request_id, &input.command);
        match self.vm().queue_io_action(&request, 0) {
            Ok(()) => {
                self.input_recording.push_io(input);
            }
            Err(source) if source.kind() == std::io::ErrorKind::ResourceBusy => {
                self.pending_input_io = Some(input);
                return Ok(target);
            }
            Err(source) => {
                return Err(LabError::QueueInputIo {
                    at: input.at,
                    target: input.target,
                    command: input.command,
                    source,
                })
            }
        }

        // Peek the next input to set the VM's stop_at: when its start vt
        // arrives we'll be re-entered via StopTscReached to queue it.
        let Some(source) = self.input_source.as_mut() else {
            return Ok(target);
        };
        self.pending_input_io = source.next_io_input();
        match self.pending_input_io.as_ref() {
            None => {
                self.input_io_exhausted = true;
                Ok(target)
            }
            Some(next) => {
                self.check_freq(next.at.frequency())?;
                Ok(next.at.min(target))
            }
        }
    }

    /// Pull the next deterministic I/O input from this branch's source, if
    /// one is attached.
    ///
    /// The returned input is not queued automatically; callers can inspect
    /// or transform it before deciding whether to pass it to
    /// [`Self::sched_bash`].
    pub fn next_io_input(&mut self) -> Option<IoInput> {
        self.input_source.as_mut()?.next_io_input()
    }

    /// Inputs consumed by this branch so far.
    pub fn input_recording(&self) -> &InputRecording {
        &self.input_recording
    }

    /// Clone this branch's consumed-input recording for replay elsewhere.
    pub fn input_recording_to_source(&self) -> crate::RecordedInputSource {
        crate::RecordedInputSource::new(self.input_recording.clone())
    }

    /// Inject a bash command and block until *its* response arrives.
    ///
    /// The `target` selects whether the command runs on the guest host
    /// (outside any container) or inside a named container.
    ///
    /// Drives the VM forward through any intervening exits — the branch's
    /// virtual time advances by however long the guest takes to execute the
    /// command and reply.
    ///
    /// Requires the guest to have `bedrock-io.ko` loaded and registered.
    ///
    /// Each request carries a unique `request_id` the kernel module echoes
    /// in the matching response, so this call returns specifically its own
    /// reply even when other in-flight I/O actions complete first. Any
    /// such interleaved responses are queued internally and delivered
    /// in order through subsequent [`Branch::run_until`] calls.
    pub fn bash(&mut self, target: BashTarget, cmd: &str) -> Result<BashOutput> {
        let id = self.next_request_id();
        let request = bash::encode_bash_request(target, id, cmd);
        let bytes = self.run_io_action(&request, id)?;
        let (_rid, response) = bash::decode_response(&bytes).map_err(LabError::BadResponse)?;
        match response {
            ActionResponse::Bash(out) => Ok(out),
            other => Err(LabError::BadResponse(format!(
                "expected bash response, got {other:?}"
            ))),
        }
    }

    /// Schedule a bash command to fire at virtual time `at`.
    ///
    /// Returns immediately; the response is delivered asynchronously when
    /// [`Branch::run_until`] reaches the I/O response exit and yields
    /// [`RunOutcome::ActionResponse`].
    ///
    /// `at.instructions() == 0` is the special "fire as soon as the guest is
    /// interruptible" value the hypervisor's I/O channel honors. For non-zero
    /// values the action lands at exactly that emulated-TSC.
    pub fn sched_bash(&mut self, at: VirtTime, target: BashTarget, cmd: &str) -> Result<()> {
        self.check_freq(at.frequency())?;
        let request_id = self.next_request_id();
        let request = bash::encode_bash_request(target, request_id, cmd);
        self.vm_mut().queue_io_action(&request, at.instructions())?;
        Ok(())
    }

    /// Query the guest's workload listing — the set of containers and their
    /// invocable drivers — and block until the response arrives.
    ///
    /// Requires the guest to have `bedrock-io.ko` loaded and registered.
    /// See [`Branch::bash`] for the request-tagging guarantee — this call
    /// also returns specifically its own reply.
    pub fn workload_details(&mut self) -> Result<WorkloadDetails> {
        let id = self.next_request_id();
        let request = bash::encode_workload_details_request(id);
        let bytes = self.run_io_action(&request, id)?;
        let (_rid, response) = bash::decode_response(&bytes).map_err(LabError::BadResponse)?;
        match response {
            ActionResponse::WorkloadDetails(details) => Ok(details),
            other => Err(LabError::BadResponse(format!(
                "expected workload-details response, got {other:?}"
            ))),
        }
    }

    /// Carve out an immutable [`Checkpoint`] at the current point, consuming
    /// this branch.
    ///
    /// The branch's VM becomes the checkpoint's frozen fork source. To
    /// continue execution from this point, call [`Checkpoint::branch`] on the
    /// returned checkpoint.
    pub fn checkpoint(mut self) -> Result<Checkpoint> {
        let vm = self.vm.take().expect("Branch.vm taken");
        let id = CheckpointId(self.lab.next_checkpoint_id());
        let time = self.current_time;
        let parent_id = self.origin.id();
        let from_branch = self.id;
        let inner = Arc::new(CheckpointInner {
            id,
            time,
            vm,
            _vm_parent: Some(Arc::downgrade(&self.origin.inner)),
            lab: self.lab.clone(),
            partial_line: core::mem::take(&mut self.partial),
            input_source: self.input_source.take(),
            pending_input_io: self.pending_input_io.take(),
            input_io_exhausted: self.input_io_exhausted,
            input_recording: core::mem::take(&mut self.input_recording),
        });
        self.lab
            .graph
            .lock()
            .unwrap()
            .register_checkpoint(&inner, Some(parent_id));
        self.lab.sink.on_event(Event::CheckpointCreated {
            checkpoint: id,
            from_branch: Some(from_branch),
            parent: Some(parent_id),
            at: time,
        });
        Ok(Checkpoint { inner })
        // self drops here, removing this branch from lab.live_branches.
    }

    /// Take a read-only snapshot of the entire tree this branch belongs to.
    pub fn tree(&self) -> Tree {
        Tree::from_lab(&self.lab)
    }
}

/// Outcome of [`Branch::feed_rng`]. Internal — branches translate this into
/// either a `continue` or one of the public surfacing variants of
/// [`RunOutcome`].
enum FeedRng {
    /// Value fed; caller should `continue` the run loop.
    Fed,
    /// Branch has no userspace source attached (kernel-side RDRAND mode).
    NoSource,
    /// Source returned `None` — no more randomness available.
    Exhausted,
}

impl Drop for Branch {
    fn drop(&mut self) {
        if let Ok(mut live) = self.lab.live_branches.lock() {
            live.remove(&self.id);
        }
    }
}

impl std::fmt::Debug for Branch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Branch")
            .field("id", &self.id)
            .field("current_time", &self.current_time)
            .finish()
    }
}
