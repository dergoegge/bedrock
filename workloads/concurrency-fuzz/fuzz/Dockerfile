# syntax=docker/dockerfile:1
#
# Concurrency-fuzz workload image: the producer/consumer sample target and the
# VMCALL ready/shutdown helper.
#
# The fuzzing scheduler itself is NOT in this image. It is a generic guest
# capability: the sched_ext BPF scheduler + its init service (scx-init) and the
# crun wrapper (crun-shim) live in the initrd (see nix/podman-initrd.nix) and
# are loaded once at boot. crun-shim — registered as podman's OCI runtime —
# switches this container's processes into SCHED_EXT, so everything the
# container runs is governed by the fuzzing scheduler with no per-image setup.
#
# Build context is this directory (fuzz/); build.sh stages guest/libvmcall.h in.

FROM debian:trixie AS build

RUN apt-get update && apt-get install -y --no-install-recommends \
      gcc libc6-dev \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY queue.c vmcall.c libvmcall.h ./

# Target and VMCALL helper have no third-party deps, so link them static.
RUN gcc -O2 -static -pthread -o queue queue.c
RUN gcc -O2 -static -o bedrock-vmcall vmcall.c

FROM debian:trixie-slim
COPY --from=build /src/queue          /usr/local/bin/queue
COPY --from=build /src/bedrock-vmcall /usr/local/bin/bedrock-vmcall
COPY run.sh /usr/local/bin/run.sh
RUN chmod +x /usr/local/bin/run.sh
ENTRYPOINT ["/usr/local/bin/run.sh"]
