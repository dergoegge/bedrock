// Self-signed TLS cert generator pinned to a NotBefore well in the past
// (2020-01-01) and NotAfter far in the future (2099-01-01). The bedrock
// guest boots with a fixed wall clock of 2024-01-01, so btcd's own
// `gencerts` (which uses time.Now() at build time, i.e. 2026+) produces
// certs the guest sees as not-yet-valid. We can't move the guest's
// clock, so we move the cert's validity window instead.
//
// Stdlib only — no btcd deps — so the build stage doesn't have to drop
// this into btcd's cmd/ tree.
//
// Usage: gencert <cert-out-path> <key-out-path>
package main

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/pem"
	"fmt"
	"math/big"
	"net"
	"os"
	"time"
)

func main() {
	if len(os.Args) != 3 {
		fmt.Fprintln(os.Stderr, "usage: gencert <cert-path> <key-path>")
		os.Exit(1)
	}
	certPath, keyPath := os.Args[1], os.Args[2]

	priv, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		fail("generate key:", err)
	}
	serial, err := rand.Int(rand.Reader, new(big.Int).Lsh(big.NewInt(1), 128))
	if err != nil {
		fail("serial:", err)
	}

	tpl := x509.Certificate{
		SerialNumber: serial,
		Subject: pkix.Name{
			Organization: []string{"bedrock-btcd"},
			CommonName:   "btcd1",
		},
		NotBefore: time.Date(2020, 1, 1, 0, 0, 0, 0, time.UTC),
		NotAfter:  time.Date(2099, 1, 1, 0, 0, 0, 0, time.UTC),
		KeyUsage: x509.KeyUsageKeyEncipherment |
			x509.KeyUsageDigitalSignature |
			x509.KeyUsageCertSign,
		ExtKeyUsage: []x509.ExtKeyUsage{
			x509.ExtKeyUsageServerAuth,
			x509.ExtKeyUsageClientAuth,
		},
		IsCA:                  true,
		BasicConstraintsValid: true,
		DNSNames:              []string{"btcd1", "btcd2", "btcwallet", "lnd1", "lnd2", "lnd3", "localhost"},
		IPAddresses:           []net.IP{net.ParseIP("127.0.0.1"), net.ParseIP("::1")},
	}

	der, err := x509.CreateCertificate(rand.Reader, &tpl, &tpl, &priv.PublicKey, priv)
	if err != nil {
		fail("create cert:", err)
	}

	certOut, err := os.OpenFile(certPath, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, 0o644)
	if err != nil {
		fail("open cert:", err)
	}
	if err := pem.Encode(certOut, &pem.Block{Type: "CERTIFICATE", Bytes: der}); err != nil {
		fail("encode cert:", err)
	}
	certOut.Close()

	keyBytes, err := x509.MarshalECPrivateKey(priv)
	if err != nil {
		fail("marshal key:", err)
	}
	keyOut, err := os.OpenFile(keyPath, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, 0o600)
	if err != nil {
		fail("open key:", err)
	}
	if err := pem.Encode(keyOut, &pem.Block{Type: "EC PRIVATE KEY", Bytes: keyBytes}); err != nil {
		fail("encode key:", err)
	}
	keyOut.Close()
}

func fail(prefix string, err error) {
	fmt.Fprintln(os.Stderr, prefix, err)
	os.Exit(1)
}
