#!/bin/sh
# lnd node entrypoint. Runs lnd in regtest against a btcd backend chosen
# per-service via $BTCD_HOST (lnd1->btcd1, lnd2->btcd2), so Lightning
# bridges the existing two-peer btcd network. Both lnd1 and lnd2 run this
# image; the launcher is service-agnostic apart from $BTCD_HOST.
#
# TLS: lnd reuses the build-time-baked self-signed cert shared with btcd
# (/etc/bedrock/rpc.cert + rpc.key) as *both* its own gRPC server cert and
# the cert it verifies btcd's RPC with. The cert's SANs cover lnd1/lnd2, so
# lncli in the workload container reaches either node over the network with
# the same cert. --tlsdisableautofill stops lnd appending interface IPs and
# regenerating the (otherwise complete, 2020-2099-valid) cert.
#
# --no-macaroons: drivers then need only the shared cert, no per-run secret
# (the macaroon root key is generated at runtime and isn't known at build
# time). --noseedbackup auto-creates and unlocks the embedded wallet on
# first boot from kernel entropy, which under bedrock comes from the
# deterministic RDRAND traps — so the wallet seed, node identity, and
# on-chain addresses are reproducible across replays.
#
# --nobootstrap: there's no external network on the closed regtest, so lnd's
# DNS-seed peer bootstrap only spins every 60s logging "Unable to retrieve
# initial bootstrap peers" — disabling it removes that recurring serial noise
# from delorean's log-shape feedback. Peers are wired explicitly by the
# workload script and the open-channel driver, so bootstrap isn't needed.

set -e

BTCD_HOST="${BTCD_HOST:-btcd1}"

# lnd refuses --no-macaroons when an RPC listener is on a publicly-reachable
# (unspecified/0.0.0.0) interface, but skips that check for loopback *and
# RFC1918-private* addresses (lncfg.EnforceSafeAuthentication). Podman's bridge
# assigns each container a private IP, so bind the gRPC listener to this
# container's own bridge IP: the check passes and lncli reaching us by hostname
# (lnd1/lnd2) from the workload container still resolves to this same IP. The
# p2p --listen and the loopback-default REST listener aren't subject to the
# check, so they can stay as-is.
RPC_IP=$(hostname -i 2>/dev/null | awk '{print $1}')
: "${RPC_IP:?could not determine container bridge IP for --rpclisten}"

exec lnd \
    --bitcoin.regtest \
    --bitcoin.node=btcd \
    --btcd.rpchost="${BTCD_HOST}:18334" \
    --btcd.rpcuser=user \
    --btcd.rpcpass=password \
    --btcd.rpccert=/etc/bedrock/rpc.cert \
    --rpclisten="${RPC_IP}:10009" \
    --listen=0.0.0.0:9735 \
    --tlscertpath=/etc/bedrock/rpc.cert \
    --tlskeypath=/etc/bedrock/rpc.key \
    --tlsdisableautofill \
    --no-macaroons \
    --noseedbackup \
    --nobootstrap \
    --bitcoin.defaultchanconfs=1 \
    --debuglevel=info \
    "$@"
