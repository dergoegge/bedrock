FROM debian:bookworm-slim

# Install podman and dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    podman \
    skopeo \
    containernetworking-plugins \
    fuse-overlayfs \
    slirp4netns \
    crun \
    iproute2 \
    iptables \
    procps \
    ca-certificates \
    python3 \
    python3-pip \
    gcc \
    libc6-dev \
    && rm -rf /var/lib/apt/lists/* \
    && pip3 install --break-system-packages podman-compose

# Build bedrock utilities
COPY shutdown.c /tmp/shutdown.c
COPY miner.c /tmp/miner.c
RUN gcc -O2 -static -o /usr/local/bin/bedrock-shutdown /tmp/shutdown.c \
    && gcc -O2 -static -o /usr/local/bin/bedrock-miner /tmp/miner.c \
    && rm /tmp/shutdown.c /tmp/miner.c

# Create necessary directories
RUN mkdir -p /proc /sys /dev /tmp /run /var/lib/containers /etc/containers /images /workload

# Configure podman for rootful operation with overlay
COPY containers.conf /etc/containers/containers.conf
COPY storage.conf /etc/containers/storage.conf

# Pre-bake Bitcoin Core image during build using skopeo
RUN skopeo copy docker://docker.io/bitcoin/bitcoin:latest docker-archive:/images/bitcoin.tar

# Additional pre-baked container images (add your own)
# Example: RUN skopeo copy docker://myregistry/myapp:latest docker-archive:/images/myapp.tar
# Or: COPY images/myapp.tar /images/myapp.tar
COPY images/ /images/

# Compose files
COPY compose.yaml /workload/compose.yaml

# Init script
COPY init /init
RUN chmod +x /init
