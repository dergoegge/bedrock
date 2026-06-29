#!/bin/sh
# Entrypoint for the concurrency-fuzz workload.
#
#   1. Signal the VM ready (takes the boot checkpoint).
#   2. Run the producer/consumer sample until it crashes (the expected outcome).
#   3. Shut the VM down so the run terminates deterministically.
#
# There is no scheduler setup here: the in-kernel fuzzing scheduler is loaded by
# the guest at boot (scx-init), and crun-shim — podman's OCI runtime — has
# already switched this container's processes into SCHED_EXT, so `queue` (and
# anything else this container runs) is governed by the fuzzing scheduler
# automatically. Under bedrock's single vCPU + emulated TSC the schedule is a
# pure function of the getrandom stream bedrock serves; vary that stream to prove
# the crash time depends on it (determinism negative control).
set -eu

bedrock-vmcall --ready

# queue aborts (non-zero / SIGABRT) once the fuzzing scheduler starves its
# consumer long enough for an item to go stale — the success condition for a
# fuzz run. Don't let `set -e` abort before we issue the shutdown.
/usr/local/bin/queue || true

bedrock-vmcall
