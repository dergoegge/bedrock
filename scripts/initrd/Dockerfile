FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    iproute2 \
    iputils-ping \
    && rm -rf /var/lib/apt/lists/*

# Install Bitcoin Core
ADD https://bitcoincore.org/bin/bitcoin-core-28.0/bitcoin-28.0-x86_64-linux-gnu.tar.gz /tmp/bitcoin.tar.gz
RUN tar -C /usr --strip-components=1 -xzf /tmp/bitcoin.tar.gz && rm /tmp/bitcoin.tar.gz

# Create necessary directories
RUN mkdir -p /proc /sys /dev /tmp /run

# Copy init script
COPY init /init
RUN chmod +x /init
