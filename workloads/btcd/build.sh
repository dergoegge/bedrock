#!/usr/bin/env bash
# Build the four btcd-workload container images (btcd, btcwallet, lnd,
# workload) and pack them into a single docker-archive tarball at
# workloads/btcd/images.tar. Hand that file (along with compose.yaml)
# to mkPodmanInitrd in flake.nix to bake them into a bootable bedrock
# initramfs.
#
# Usage:  ./build.sh
#
# Requires a working `docker` daemon (or `podman` with a `docker` shim).
# btcd / btcwallet share one Go build stage and lnd has its own (it pins a
# newer Go), so `docker build` compiles each daemon once and reuses the
# cached layers across the four --target invocations.

set -euo pipefail
cd "$(dirname "$0")"

DOCKER="${DOCKER:-docker}"

$DOCKER build --target=btcd      -t bedrock/btcd:latest          .
$DOCKER build --target=btcwallet -t bedrock/btcwallet:latest     .
$DOCKER build --target=lnd       -t bedrock/lnd:latest           .
$DOCKER build --target=workload  -t bedrock/btcd-workload:latest .

$DOCKER save \
    bedrock/btcd:latest \
    bedrock/btcwallet:latest \
    bedrock/lnd:latest \
    bedrock/btcd-workload:latest \
    -o images.tar

echo
echo "Wrote $(pwd)/images.tar ($(du -h images.tar | cut -f1))"
