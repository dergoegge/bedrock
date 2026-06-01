#!/bin/sh
# Wrapper that sources the fixed regtest mining keypair and exec's btcd
# pinned to a build-time-generated self-signed RPC cert. Both btcd1 and
# btcd2 share this image; --miningaddr is set on both (harmless for
# btcd2, which never receives a `generate`) so the launcher stays
# service-agnostic.

set -e
. /etc/bedrock/key.env

exec /usr/local/bin/btcd \
    --regtest \
    --txindex \
    --rpccert=/etc/bedrock/rpc.cert \
    --rpckey=/etc/bedrock/rpc.key \
    --rpcuser=user \
    --rpcpass=password \
    --rpclisten=0.0.0.0:18334 \
    --listen=0.0.0.0:18444 \
    --miningaddr="$ADDR" \
    --debuglevel=info \
    "$@"
