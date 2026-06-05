// SPDX-License-Identifier: GPL-2.0

//! # flux
//!
//! A clean-room, coverage-guided fuzzer for bedrock VMs.
//!
//! flux drives a deterministic bedrock guest through the [`bedrock_lab`] API:
//! it boots once to a ready checkpoint, discovers the workload's drivers, then
//! runs a **shared-everything parallel campaign**. The corpus *is* the VM
//! checkpoint tree — each entry is a checkpoint plus the `RDRAND`/bash inputs
//! that reached it. Workers pull an entry, mutate its input, rewind to the
//! earliest touched point, replay the mutated suffix forward, and keep the
//! result if it found new coverage.
//!
//! ## Module map
//!
//! - [`rng`] — a tiny fast deterministic PRNG.
//! - [`input`] — the [`Input`](input::Input) type and its replay [`InputSource`].
//! - [`mutate`] — the structured mutators (RNG byte havoc + IO insert/shift/swap)
//!   and the action vocabulary.
//! - [`bytemut`] — length-preserving byte havoc over the RNG / argument bytes.
//! - [`shape`] — ANSI stripping for serial lines (used by crash detection).
//! - [`coverage`] — feedback-buffer edge-coverage bitmaps.
//! - [`corpus`] — the [`Node`](corpus::Node) and the scheduler.
//! - [`campaign`] — the parallel fuzzing loop + crash reproduction/replay.
//! - [`sink`] — serial-output capture.
//! - [`http`] / [`views`] — the read-only HTTP/SSE state API.
//! - [`affinity`] — CPU pinning. [`ui`] — terminal styling.
//!
//! [`InputSource`]: bedrock_lab::InputSource

pub mod affinity;
pub mod bytemut;
pub mod campaign;
pub mod corpus;
pub mod coverage;
pub mod http;
pub mod input;
pub mod mutate;
pub mod rng;
pub mod shape;
pub mod sink;
pub mod ui;
pub mod views;

pub use campaign::{assertion_failure_reason, replay, Campaign, Config, ReplayOutcome};
pub use input::{Input, Reproduction};
pub use mutate::{Action, SwarmMode};
pub use rng::Rng;
pub use sink::Sink;
