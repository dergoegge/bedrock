// Derive the first default-account address btcwallet would generate
// from a fixed seed. Emits SEED= and ADDR= lines so the runtime images
// can bake both: btcwallet pipes SEED through the interactive --create
// prompt to fix its HD seed, and btcd uses ADDR as --miningaddr so
// coinbase UTXOs land in the wallet's default account natively.
//
// Avoiding `importprivkey` sidesteps a btcwallet quirk
// (waddrmgr/manager.go:411) where IsWatchOnlyAccount unconditionally
// returns true for ImportedAddrAccount — that flag skips
// AddAllInputScripts in createtx.go and produces unsigned txs that
// btcd rejects. With default-account addresses there's no imported
// account involved and sendtoaddress works directly.
//
// The derivation path replicates what btcwallet's
// waddrmgr/manager.go:deriveCoinTypeKey / deriveAccountKey and
// scoped_manager.go:deriveKey do: m/44'/0'/0'/0/0 via DeriveNonStandard
// at every level. Coin type is 0 because waddrmgr.KeyScopeBIP0044
// hardcodes Coin=0 regardless of network
// (scoped_manager.go:196-199). Account index 0 is the default
// account, branch 0 is external, index 0 is the first address
// (nextExternalIndex starts at 0 in a fresh account).
package main

import (
	"encoding/hex"
	"fmt"
	"os"

	"github.com/btcsuite/btcd/btcutil/v2/hdkeychain"
	"github.com/btcsuite/btcd/chaincfg/v2"
)

const seedHex = "bed0cca5e0bed0cca5e0bed0cca5e0bed0cca5e0bed0cca5e0bed0cca5e0cafe"

func main() {
	seed, err := hex.DecodeString(seedHex)
	if err != nil {
		fail("decode seed", err)
	}
	if len(seed) != 32 {
		fmt.Fprintln(os.Stderr, "seed must be 32 bytes")
		os.Exit(1)
	}

	net := &chaincfg.RegressionNetParams

	master, err := hdkeychain.NewMaster(seed, net)
	if err != nil {
		fail("new master", err)
	}

	h := uint32(hdkeychain.HardenedKeyStart)
	purpose, err := master.DeriveNonStandard(44 + h)
	if err != nil {
		fail("derive purpose'", err)
	}
	coin, err := purpose.DeriveNonStandard(0 + h)
	if err != nil {
		fail("derive coin'", err)
	}
	account, err := coin.DeriveNonStandard(0 + h)
	if err != nil {
		fail("derive account'", err)
	}
	branch, err := account.DeriveNonStandard(0)
	if err != nil {
		fail("derive branch", err)
	}
	addrKey, err := branch.DeriveNonStandard(0)
	if err != nil {
		fail("derive index", err)
	}

	addr, err := addrKey.Address(net)
	if err != nil {
		fail("addr encode", err)
	}

	fmt.Printf("SEED=%s\n", seedHex)
	fmt.Printf("ADDR=%s\n", addr.EncodeAddress())
}

func fail(stage string, err error) {
	fmt.Fprintf(os.Stderr, "%s: %v\n", stage, err)
	os.Exit(1)
}
