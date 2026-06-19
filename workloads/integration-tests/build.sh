#!/usr/bin/env bash
# Build the integration-tests workload's container image and pack it into a
# docker-archive tarball at workloads/integration-tests/images.tar. That file
# and compose.yaml are served to the guest at runtime over the file-transmission
# hypercall (the guest's generic initrd downloads them at boot) — the
# integration-tests harness/app serves them via BEDROCK_COMPOSE / BEDROCK_IMAGES.
# See nix/podman-initrd.nix.
#
# Usage:  ./build.sh
#
# Requires a working `docker` daemon (or `podman` with a `docker` shim).

set -euo pipefail
cd "$(dirname "$0")"

DOCKER="${DOCKER:-docker}"

# Stage the shared guest libraries into each Docker build context. Docker's
# COPY can't reach files outside the context, so the single sources under
# guest/ are copied in here and removed on exit — keeping one source of truth
# instead of committed dups. The `ready` image needs only the header-only
# libvmcall.h; the coverage images also need the libfeedback backend plus the
# matching frontend (libpcguard for trace-pc-guard, libvoidstar for the Go SDK).
GUEST=../../guest
trap 'rm -f ready/libvmcall.h \
            coverage-pcguard/libvmcall.h coverage-pcguard/libfeedback.h \
            coverage-pcguard/libfeedback.c coverage-pcguard/libpcguard.c \
            coverage-go/libvmcall.h coverage-go/libfeedback.h \
            coverage-go/libfeedback.c coverage-go/libvoidstar.c' EXIT
cp "$GUEST/libvmcall.h" ready/libvmcall.h
cp "$GUEST/libvmcall.h" "$GUEST/libfeedback.h" "$GUEST/libfeedback.c" \
   "$GUEST/libpcguard.c" coverage-pcguard/
cp "$GUEST/libvmcall.h" "$GUEST/libfeedback.h" "$GUEST/libfeedback.c" \
   "$GUEST/libvoidstar.c" coverage-go/

$DOCKER build -t bedrock/integration-tests-ready:latest ready/
$DOCKER build -t bedrock/integration-tests-coverage-pcguard:latest coverage-pcguard/
$DOCKER build -t bedrock/integration-tests-coverage-go:latest coverage-go/

# Pack into one docker-archive. `podman load` inside the initrd reads the
# embedded manifest to recover each image's name+tag, so the tarball's
# filename is opaque to consumers.
$DOCKER save \
    bedrock/integration-tests-ready:latest \
    bedrock/integration-tests-coverage-pcguard:latest \
    bedrock/integration-tests-coverage-go:latest \
    -o images.tar

echo
echo "Wrote $(pwd)/images.tar ($(du -h images.tar | cut -f1))"
