#!/bin/sh
# btcwallet sidecar entrypoint. First boot creates the on-disk wallet
# via the interactive --create flow, feeding a deterministic seed so
# the wallet's HD chain produces the same first default-account
# address that btcd is mining to (--miningaddr from /etc/bedrock/key.env).
# Subsequent restarts skip the create and go straight to the daemon.

set -e

. /etc/bedrock/key.env  # SEED, ADDR

WALLET_DIR=/root/.btcwallet/regtest
mkdir -p /root/.btcctl

if [ ! -d "$WALLET_DIR" ]; then
    echo "Creating btcwallet (first boot) with deterministic seed..."
    # Prompts driven under a pty (golang.org/x/term refuses non-TTY
    # stdin). The "yes" + seed-hex path makes btcwallet's first
    # default-account derivation match our genaddr output exactly.
    SEED="$SEED" expect <<'EOF'
set timeout 60
set seed $env(SEED)
log_user 1
spawn btcwallet --regtest --create
expect "Enter the private passphrase for your new wallet:"
send -- "password\r"
expect "Confirm passphrase:"
send -- "password\r"
expect "Do you want to add an additional layer of encryption for public data?"
send -- "no\r"
expect "Do you have an existing wallet seed you want to use?"
send -- "yes\r"
expect "Enter existing wallet seed:"
send -- "$seed\r"
# No "OK to continue" prompt on the existing-seed branch — that one
# only fires when btcwallet auto-generated a seed it wants the user to
# write down (prompt.go's `if !useUserSeed` branch). With our seed
# supplied, --create returns from Seed() straight after validation
# and exits cleanly; trying to send "OK" here would target a closed
# spawn ("send: spawn id expN not open").
expect eof
catch wait result
exit [lindex $result 3]
EOF
fi

echo "Starting btcwallet RPC on 0.0.0.0:18332..."
exec btcwallet \
    --regtest \
    --cafile=/etc/bedrock/rpc.cert \
    --rpccert=/etc/bedrock/rpc.cert \
    --rpckey=/etc/bedrock/rpc.key \
    --username=user \
    --password=password \
    --btcdusername=user \
    --btcdpassword=password \
    --rpcconnect=btcd1:18334 \
    --rpclisten=0.0.0.0:18332
