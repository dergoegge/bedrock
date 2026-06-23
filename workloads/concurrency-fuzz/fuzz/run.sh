#!/bin/sh
# Entrypoint for the concurrency-fuzz workload.
#
#   1. Signal the VM ready (takes the boot checkpoint).
#   2. Run the in-kernel fuzzing scheduler against the queue sample with a
#      FIXED seed until the sample crashes (the expected outcome). The seed is
#      what makes the run reproducible under bedrock's single vCPU + emulated
#      TSC; vary FUZZ_SEED to prove the schedule actually depends on it
#      (determinism negative control).
#   3. Shut the VM down so the run terminates deterministically.
set -eu

SEED="${FUZZ_SEED:-0x1337}"

bedrock-vmcall --ready

# The loader returns 0 when the target crashed (success for a fuzz run), so
# don't let `set -e` abort before we issue the shutdown.
/usr/local/bin/fuzz-loader "$SEED" /usr/local/bin/queue || true

bedrock-vmcall
