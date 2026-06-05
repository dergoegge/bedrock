//! Bedrock workload monitor.
//!
//! A long-lived service that runs in the guest's host namespace (outside the
//! workload containers) and streams container lifecycle events from podman.
//! Started once at guest boot, it spawns `podman events --format json` and
//! tails the stream, printing each newline-delimited JSON event as it arrives.
//!
//! On top of surfacing the raw stream, it documents an invariant about the
//! workloads: whenever a container dies, its exit code should always be greater
//! than zero (a clean exit means the workload stopped on its own, which we don't
//! expect under test). Each death appends an [`Assertion`] recording the
//! observed exit code — serialized as one line of JSON — to the sink at
//! [`ASSERTIONS_PATH`], where a downstream collector can aggregate the results.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use bedrock_assertions::always_gt;
use serde::Deserialize;

/// Default assertion sink: an append-only JSONL file, one assertion per line.
/// Override with the `BEDROCK_ASSERTIONS_PATH` environment variable (used by
/// tests/local runs).
const ASSERTIONS_PATH: &str = "/bedrock/assertions.jsonl";

/// A single `podman events --format json` record. Only the fields we act on are
/// declared; everything else in the line is ignored. Field names match podman's
/// JSON marshaling (capitalized), and all are optional so a record shape we
/// don't recognize parses rather than aborting the stream.
#[derive(Deserialize)]
struct Event {
    #[serde(rename = "Type")]
    type_: Option<String>,
    /// The event action, e.g. `"died"`, `"start"`, `"create"`.
    #[serde(rename = "Status")]
    status: Option<String>,
    /// Process exit code, populated by podman on `"died"` container events.
    #[serde(rename = "ContainerExitCode")]
    exit_code: Option<i64>,
}

impl Event {
    /// The exit code if this record is a container-death event, else `None`.
    fn container_death_exit_code(&self) -> Option<i64> {
        let is_container_death =
            self.type_.as_deref() == Some("container") && self.status.as_deref() == Some("died");
        is_container_death.then(|| self.exit_code.unwrap_or(0))
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    // Open the shared assertion sink up front. Append mode + whole-line writes
    // keep concurrent writers (this monitor plus the containers that mount the
    // same file) from interleaving. A failure here is non-fatal: we still want
    // the event stream below, we just can't record assertions.
    let path = std::env::var("BEDROCK_ASSERTIONS_PATH").unwrap_or_else(|_| ASSERTIONS_PATH.into());
    let mut sink = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => Some(file),
        Err(e) => {
            eprintln!("cannot open assertion sink {path}: {e}; assertions will be dropped");
            None
        }
    };

    // Stream events as they happen. `--stream` keeps the process attached and
    // emitting; without a `--filter` we receive every event podman reports.
    // `--format json` emits one self-contained JSON object per line (newline-
    // delimited), so each line we print is a complete, parseable record.
    let mut child = Command::new("podman")
        .args(["events", "--stream", "--format", "json"])
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn podman events: {e}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or("podman events produced no stdout")?;

    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|e| format!("reading podman events: {e}"))?;
        println!("{line}");

        // Best-effort: a line we can't parse is still surfaced above, we just
        // can't assert on it.
        if let Ok(event) = serde_json::from_str::<Event>(&line) {
            if let Some(exit_code) = event.container_death_exit_code() {
                record_exit_code_assertion(sink.as_mut(), exit_code);
            }
        }
    }

    let status = child
        .wait()
        .map_err(|e| format!("waiting on podman events: {e}"))?;
    Err(format!("podman events exited: {status}"))
}

/// Record the "container exit code is always > 0" invariant for an observed
/// death by appending one line of serialized JSON to the assertion sink.
fn record_exit_code_assertion(sink: Option<&mut File>, exit_code: i64) {
    let Some(file) = sink else { return };

    let assertion = always_gt!(exit_code, 0);
    let mut line = match serde_json::to_string(&assertion) {
        Ok(json) => json,
        Err(e) => {
            eprintln!("failed to serialize exit-code assertion: {e}");
            return;
        }
    };
    // One write of a single sub-PIPE_BUF line keeps appends atomic across the
    // file's concurrent writers.
    line.push('\n');
    if let Err(e) = file.write_all(line.as_bytes()) {
        eprintln!("failed to append assertion to sink: {e}");
    }
}
