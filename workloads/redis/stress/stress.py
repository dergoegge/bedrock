#!/usr/bin/env python3
# Bedrock redis-stress — runs a deterministic batch of Redis operations on a
# fixed interval. Output goes to stdout (and from there to the bedrock serial
# console), so divergence between runs surfaces in the captured logs.
#
# The fuzzer separately invokes driver scripts under /opt/bedrock/drivers/ via
# the bedrock-io channel; this loop runs alongside, exercising the same surface
# area so the workload makes progress even without any fuzz input.

import subprocess
import sys
import time

PRIMARY_PORT = 6379
REPLICA_PORTS = [6380, 6381]
ALL_PORTS = [PRIMARY_PORT, *REPLICA_PORTS]

# Each redis server runs in its own container/netns under bridge networking;
# aardvark-dns resolves the container_name. Map the per-server port we use
# back to the service name so callers can stay port-keyed.
PORT_TO_HOST = {
    6379: "redis",
    6380: "redis-replica-1",
    6381: "redis-replica-2",
}

BATCH = [
    ["SET", "key:a", "stress-value"],
    ["INCR", "shared:counter"],
    ["LPUSH", "shared:list", "item"],
    ["SADD", "shared:set", "member"],
    ["HSET", "shared:hash", "field", "value"],
    ["GET", "key:a"],
    ["LRANGE", "shared:list", "0", "9"],
    ["SMEMBERS", "shared:set"],
]


def redis_cli(args, port=PRIMARY_PORT):
    """Run `redis-cli -h <host> -p <port> <args>` and return (rc, stdout-first-line)."""
    result = subprocess.run(
        ["redis-cli", "-h", PORT_TO_HOST[port], "-p", str(port), *args],
        capture_output=True,
        text=True,
    )
    out = result.stdout.strip().splitlines()
    return result.returncode, (out[0] if out else "")


def wait_for_redis(port, timeout=30):
    for _ in range(timeout):
        rc, out = redis_cli(["PING"], port=port)
        if rc == 0 and out == "PONG":
            return True
        time.sleep(1)
    return False


def main():
    interval = int(sys.argv[1]) if len(sys.argv) > 1 else 30
    print(f"Bedrock redis-stress starting (interval: {interval}s)", flush=True)

    for port in ALL_PORTS:
        print(f"Waiting for redis on port {port}...", flush=True)
        if not wait_for_redis(port):
            print(
                f"redis on port {port} did not come up within 30 seconds",
                file=sys.stderr,
            )
            sys.exit(1)
    print("all redis servers are up", flush=True)

    print("Signaling bedrock VM ready...", flush=True)
    subprocess.run(["/usr/local/bin/bedrock-ready"], check=True)

    batches = 0
    failures = 0
    while True:
        for cmd in BATCH:
            rc, _ = redis_cli(cmd)
            if rc != 0:
                failures += 1

        batches += 1
        _, counter = redis_cli(["GET", "shared:counter"])
        _, dbsize_p = redis_cli(["DBSIZE"])
        _, dbsize_r1 = redis_cli(["DBSIZE"], port=REPLICA_PORTS[0])
        _, dbsize_r2 = redis_cli(["DBSIZE"], port=REPLICA_PORTS[1])
        print(
            f"Batch {batches} done "
            f"(counter={counter}, dbsize p/r1/r2={dbsize_p}/{dbsize_r1}/{dbsize_r2}, "
            f"failures={failures})",
            flush=True,
        )

        time.sleep(interval)


if __name__ == "__main__":
    main()
