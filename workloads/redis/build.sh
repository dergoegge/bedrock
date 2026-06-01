#!/usr/bin/env bash
# Build the redis workload's container images and pack them into a single
# docker-archive tarball at workloads/redis/images.tar. Hand that file
# (along with compose.yaml) to mkPodmanInitrd in flake.nix to bake them
# into a bootable bedrock initramfs.
#
# Usage:  ./build.sh
#
# Requires a working `docker` daemon (or `podman` with a `docker` shim).

set -euo pipefail
cd "$(dirname "$0")"

DOCKER="${DOCKER:-docker}"

# Vanilla upstream Redis image (used by the `redis` service).
$DOCKER pull docker.io/redis:7-alpine

# Workload-specific image with bedrock binaries baked in.
$DOCKER build -t bedrock/redis-stress:latest stress/

# Pack both into one docker-archive. `podman load` inside the initrd
# reads the embedded manifest to recover each image's name+tag, so the
# tarball's filename is opaque to consumers.
$DOCKER save \
    docker.io/redis:7-alpine \
    bedrock/redis-stress:latest \
    -o images.tar

echo
echo "Wrote $(pwd)/images.tar ($(du -h images.tar | cut -f1))"
