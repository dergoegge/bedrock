<!-- SPDX-License-Identifier: GPL-2.0 -->
# flux

A clean-room, coverage-guided fuzzer for bedrock VMs — built directly on
the `bedrock-lab` API, with no external fuzzing framework.

flux drives a deterministic bedrock guest through `bedrock_lab`: it boots once
to a ready checkpoint, discovers the workload's drivers, then runs a
**shared-everything parallel campaign**. The corpus *is* the VM checkpoint
tree — each entry is a checkpoint plus the `RDRAND`/bash inputs that reached it.
A worker pulls an entry, mutates its input, rewinds to the earliest touched
point, replays the mutated suffix forward, and keeps the result if it found new
coverage. Any worker can build on a checkpoint discovered by any other.

## Usage

```sh
flux <vmlinux> <initramfs> [options]

# fuzz with 4 workers, serve the live dashboard
flux vmlinux rootfs --threads 4 --http 127.0.0.1:8080

# 60-second benchmark, one machine-readable result line
flux vmlinux rootfs --threads 4 --bench-secs 60
```

Key options: `--threads`, `--run-for-secs` / `--min-run-for-secs` (per-worker
run windows, exponentially spaced), `--max-dry-rounds` (per-pick early-stop),
`--burst` (IoInsert burst size), `--swarm {lineage,burst,off}`,
`--ignore-source`, `--exclude-from-partition`, `--no-log-feedback`,
`--no-quit-on-solution`, `--http`, `--bench-secs`, `--cov-dump`.

## Design

| Module | Responsibility |
| --- | --- |
| `rng` | A tiny `xoshiro256**` PRNG. |
| `input` | The `Input` type (RNG values + bash actions) and its replay `InputSource`. |
| `bytemut` | Length-preserving byte havoc over the recorded RNG stream. |
| `mutate` | Structured mutators (RNG byte havoc, IO insert / time-shift / driver-swap), the action vocabulary, and swarm-subset logic. |
| `shape` | Log-line shape normalization + FNV-1a hashing (the fallback signal). |
| `coverage` | Feedback-buffer bitmaps + log-shape novelty. |
| `corpus` | The `Node` (a corpus entry == a tree node) and the scheduler. |
| `campaign` | The parallel fuzzing loop: pick → havoc → run → merge → add. |
| `sink` | Dual-mode serial capture (stream on boot, buffer per-branch when fuzzing). |
| `http` / `views` | Read-only HTTP + SSE state API and its JSON DTOs. |
| `affinity` | CPU pinning. `ui` | Terminal styling. |

### Two novelty signals

1. **Feedback-buffer bitmaps** — instrumented workloads register coverage
   buffers; any byte going 0 → non-zero is a new edge.
2. **Log-line shapes** — for uninstrumented workloads, each serial line's
   *shape* (numbers and long hex/Base58 tokens normalized to `<*>`) is hashed;
   a first-seen shape is novelty.

### Scheduler

Each corpus node's selection weight is

```
(1 + ln(1+novelty)) / (1 + effort) / (1 + in_flight) · (1 + ln(1+rarity))
```

— favoring fertile, under-explored, rare-edge checkpoints while spreading
concurrent workers across distinct ones. See `corpus::node_weight`.

### Determinism

The guest's *boot* RNG is fixed (so every run reaches the same ready
checkpoint); only the fuzzer's *exploration* RNG is randomized per run. A
recorded input replays bit-for-bit through `InputCursor`.

## Relationship to `delorean`

flux is a from-scratch reimplementation of the `delorean` fuzzer's active
(custom-loop) architecture with no external fuzzing-framework dependency: the
mutators, PRNG, havoc stacking, and core-affinity are all implemented here as
small, self-contained modules.
