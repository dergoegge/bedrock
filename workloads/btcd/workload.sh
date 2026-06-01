#!/bin/sh
# Bedrock btcd workload bootstrap. The btcwallet sidecar owns wallet
# creation (with a deterministic seed) and the daemon lifecycle; this
# container only orchestrates:
#   1. Wait for btcd1, btcd2, and btcwallet RPC.
#   2. Pull the first default-account address out of btcwallet and
#      sanity-check it matches the genaddr-baked ADDR — this also
#      registers the address with btcd via notifyreceived, so the
#      wallet's chain client picks up coinbase outputs as they're
#      mined.
#   3. Drive btcd1 to mine 200 regtest blocks (>100 = first batch of
#      coinbase outputs is past maturity) into the wallet's default
#      account. Per the chicken-and-egg in btcd's netsync (see
#      compose.yaml comment on btcd2), prime btcd1 with one block then
#      issue `addnode` on btcd2 so peering can sync.
#   4. Wait until btcwallet reports a non-zero balance — that proves
#      the chain-sync notification path is alive end-to-end.
#   5. Bring up the Lightning layer: wait for lnd1/lnd2/lnd3, fund all three
#      from btcwallet, and wire them into a chain lnd1->lnd2->lnd3 (two
#      channels) so payments work at the ready checkpoint and lnd1<->lnd3 is
#      forced to route multi-hop through lnd2.
#   6. Signal HYPERCALL_READY to the hypervisor.
#   7. Sleep forever; drivers under /opt/bedrock/drivers/ then handle
#      ongoing mining + txs plus Lightning channel open/close and payments.
#      Because the mining UTXOs live in the default account, sendtoaddress
#      works directly — no importprivkey, no IsWatchOnlyAccount trap.

set -eu

. /etc/bedrock/key.env  # SEED, ADDR

mkdir -p /root/.btcctl

BTCD_RPC_HOST=btcd1
BTCD_RPC_PORT=18334
BTCD2_RPC_PORT=18334
WALLET_RPC_HOST=btcwallet
WALLET_RPC_PORT=18332
WALLET_PASS=password

btcctl_btcd1() {
    btcctl --regtest -u user -P password \
        --rpccert=/etc/bedrock/rpc.cert \
        --rpcserver="$BTCD_RPC_HOST:$BTCD_RPC_PORT" "$@"
}
btcctl_btcd2() {
    btcctl --regtest -u user -P password \
        --rpccert=/etc/bedrock/rpc.cert \
        --rpcserver="btcd2:$BTCD2_RPC_PORT" "$@"
}
btcctl_wallet() {
    btcctl --regtest --wallet -u user -P password \
        --rpccert=/etc/bedrock/rpc.cert \
        --rpcserver="$WALLET_RPC_HOST:$WALLET_RPC_PORT" "$@"
}

# lnd nodes share the baked TLS cert and run with --no-macaroons, so lncli
# needs only the cert and the node's rpcserver to drive it over the network.
LNCLI="lncli --network=regtest --no-macaroons --tlscertpath=/etc/bedrock/rpc.cert"
lncli1() { $LNCLI --rpcserver=lnd1:10009 "$@"; }
lncli2() { $LNCLI --rpcserver=lnd2:10009 "$@"; }
lncli3() { $LNCLI --rpcserver=lnd3:10009 "$@"; }

# Connect <opener_fn>'s node to <peer> and open a 0.01 BTC channel toward it.
# Best-effort with retries but non-fatal — if a leg doesn't open we still
# signal ready and the open-channel driver can establish one later.
open_channel() {
    opener=$1; peer=$2
    peer_pk=$($LNCLI --rpcserver="$peer:10009" getinfo | json_str identity_pubkey)
    if [ -z "$peer_pk" ]; then
        echo "WARN: could not read $peer pubkey; skipping $opener -> $peer" >&2
        return 0
    fi
    "$opener" connect "$peer_pk@$peer:9735" 2>/dev/null || true
    # openchannel fails if the peer link isn't up yet, and connect is async.
    for _ in $(seq 1 60); do
        "$opener" listpeers | grep -q "$peer_pk" && break
        sleep 1
    done
    for i in $(seq 1 5); do
        if "$opener" openchannel --node_key "$peer_pk" --local_amt 1000000 >/dev/null 2>&1; then
            echo "  channel funding broadcast: $opener -> $peer"
            return 0
        fi
        [ "$i" = 5 ] && echo "WARN: channel open $opener -> $peer did not succeed" >&2
        sleep 2
    done
}

# Extract a string field from lncli's pretty-printed JSON without pulling in
# jq (the workload image ships none). Field values are stable enough that a
# regex is fine, matching the sed approach the btcd drivers already use.
json_str() { sed -n "s/.*\"$1\":[[:space:]]*\"\\([^\"]*\\)\".*/\\1/p" | head -n1; }

wait_for() {
    label="$1"; shift
    for i in $(seq 1 120); do
        if "$@" >/dev/null 2>&1; then
            echo "  $label: up after ${i}s"
            return 0
        fi
        sleep 1
    done
    echo "FATAL: $label did not come up within 120s" >&2
    exit 1
}

echo "=== btcd workload setup ==="

echo "[1/5] Waiting for btcd1, btcd2, btcwallet RPC..."
wait_for btcd1     btcctl_btcd1  getblockcount
wait_for btcd2     btcctl_btcd2  getblockcount
wait_for btcwallet btcctl_wallet getinfo

echo "[2/5] Registering mining address with the wallet..."
# `getnewaddress` derives the next external address in the default
# account (index 0 on first call) AND tells btcd via notifyreceived to
# watch for txs paying to it. genaddr derives the same address offline
# so we can pin btcd's --miningaddr; this RPC just lights up the
# wallet's chain-client side. If our derivation drifts from the
# wallet's, the diff would silently break tx routing, so fail loudly
# instead of hoping for the best.
wallet_addr=$(btcctl_wallet getnewaddress)
echo "  wallet derived: $wallet_addr"
echo "  genaddr baked:  $ADDR"
if [ "$wallet_addr" != "$ADDR" ]; then
    echo "FATAL: genaddr and btcwallet HD derivations diverged" >&2
    exit 1
fi
# Unlock for sendtoaddress later — idempotent re-arm on driver
# invocations.
btcctl_wallet walletpassphrase "$WALLET_PASS" 86400

echo "[3/5] Mining 200 regtest blocks on btcd1..."
# Step A: prime btcd1 with one block so its tip timestamp jumps from
#   the 2011-02-02 regtest genesis to the guest's 2024-01-01 clock,
#   flipping chain.IsCurrent() true and letting btcd1 relay invs.
btcctl_btcd1 generate 1 >/dev/null
# Step B: have btcd2 dial btcd1 as a fresh outbound peer so the new
#   version handshake reports btcd1.start_height=1, giving btcd2 a
#   sync candidate with height > 0. See compose.yaml comment.
btcctl_btcd2 addnode btcd1:18444 add >/dev/null
# Step C: wait for btcd2 to fetch the priming block before mining the
#   rest. Once btcd2 is past genesis, its IsCurrent() flips true too.
for i in $(seq 1 60); do
    h=$(btcctl_btcd2 getblockcount 2>/dev/null || echo 0)
    if [ "$h" -ge 1 ] 2>/dev/null; then
        echo "  btcd2 caught up to height $h after ${i}s"
        break
    fi
    sleep 1
done
# Step D: mine the remaining 199 with relay working normally.
btcctl_btcd1 generate 199 >/dev/null
# Step E: confirm peers agree before declaring setup done.
for i in $(seq 1 120); do
    h1=$(btcctl_btcd1 getblockcount 2>/dev/null || echo 0)
    h2=$(btcctl_btcd2 getblockcount 2>/dev/null || echo 0)
    if [ "$h1" = "200" ] && [ "$h2" = "200" ]; then
        echo "  both peers at height 200 after ${i}s"
        break
    fi
    sleep 1
done

echo "[4/5] Waiting for btcwallet to register mined balance..."
for i in $(seq 1 120); do
    bal=$(btcctl_wallet getbalance "*" 0 2>/dev/null || echo 0)
    whole="${bal%%.*}"
    if [ -n "$whole" ] && [ "$whole" -gt 0 ] 2>/dev/null; then
        echo "  wallet balance: $bal BTC (after ${i}s)"
        break
    fi
    sleep 1
done

echo "=== lnd setup ==="

echo "[lnd 1/3] Waiting for lnd1, lnd2, lnd3 RPC..."
wait_for lnd1 lncli1 getinfo
wait_for lnd2 lncli2 getinfo
wait_for lnd3 lncli3 getinfo

echo "[lnd 2/3] Funding all three lnd nodes from btcwallet..."
# lnd uses its own embedded wallet, so it starts with no coins. Pull a fresh
# on-chain address from each node and pay it from btcwallet's matured mining
# balance, then mine to confirm. lnd sees the funds via its btcd backend's
# block notifications.
for fn in lncli1 lncli2 lncli3; do
    addr=$($fn newaddress p2wkh | json_str address)
    echo "  $fn addr: $addr"
    btcctl_wallet sendtoaddress "$addr" 10.0 >/dev/null
done
# Confirm the funding txs (defaultchanconfs=1, but give coinbase-funded
# spends a few confirmations so the wallet treats them as spendable).
btcctl_btcd1 generate 6 >/dev/null
for fn in lncli1 lncli2 lncli3; do
    for i in $(seq 1 120); do
        bal=$($fn walletbalance \
            | sed -n 's/.*"confirmed_balance":[[:space:]]*"\([0-9]*\)".*/\1/p')
        if [ -n "$bal" ] && [ "$bal" -gt 0 ] 2>/dev/null; then
            echo "  $fn confirmed balance: $bal sat (after ${i}s)"
            break
        fi
        sleep 1
    done
done

echo "[lnd 3/3] Wiring the chain lnd1 -> lnd2 -> lnd3..."
# Open the two legs of the chain so payments work at the ready checkpoint and
# lnd1<->lnd3 is forced to route multi-hop through lnd2. The open-/close-
# channel drivers create and tear down more channels from here.
open_channel lncli1 lnd2
open_channel lncli2 lnd3
btcctl_btcd1 generate 6 >/dev/null
# lnd2 is the hub: once both legs confirm it should report two active channels.
for i in $(seq 1 120); do
    if [ "$(lncli2 listchannels | grep -c '"active": true')" -ge 2 ] 2>/dev/null; then
        echo "  chain lnd1<->lnd2<->lnd3 active after ${i}s"
        break
    fi
    sleep 1
done

echo "[5/5] Signaling bedrock VM ready..."
# Settling pause before the checkpoint snapshot. Lets late background
# work from setup drain: btcwallet's chain notifications committing
# the last batch of coinbase UTXOs to disk, peer keep-alives quieting
# down, journald flushing, anything else in flight. Quieter state at
# the moment HYPERCALL_READY fires makes the checkpoint smaller and
# subsequent branch executions less noisy. Kept short: the three lnd
# nodes gossip/bootstrap perpetually so a longer pause buys little, and
# the full setup already runs close to the boot ready-deadline (bump
# delorean's --ready-deadline-secs well above the observed boot time).
sleep 10
/usr/local/bin/bedrock-ready

echo "=== setup complete; sleeping (drivers take over) ==="
exec sleep infinity
