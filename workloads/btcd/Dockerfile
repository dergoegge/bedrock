# syntax=docker/dockerfile:1
#
# Three per-role runtime images sharing a single Go build stage:
#   - bedrock/btcd          — btcd peer node (built as target `btcd`)
#   - bedrock/btcwallet     — btcwallet sidecar (built as target `btcwallet`)
#   - bedrock/btcd-workload — orchestrator + drivers (built as target `workload`)
#
# build.sh invokes `docker build --target=<name>` once per image and packs
# all three into images.tar. Each runtime image carries only the binaries,
# certs, and scripts the matching service actually needs, so the
# bedrock-io kernel module's per-container driver enumeration finds
# drivers only under the workload container (no tmpfs override needed).

FROM golang:1.25-alpine AS gobuild
# gcc + musl-dev are for cgo: the instrumented btcd links the Antithesis SDK's
# native coverage loader, which is cgo-only (it compiles to a no-op when
# CGO_ENABLED=0). bash is needed because antithesis-go-instrumentor shells out
# to it to build the generated notifier module (alpine ships only busybox sh).
RUN apk add --no-cache git gcc musl-dev bash
WORKDIR /go/src

# btcd + btcctl. Pinning to master is intentional: the workload tracks
# upstream behaviour, and the host-side build.sh produces a fresh
# images.tar per run anyway, so reproducibility lives at that layer.
RUN git clone --depth 1 https://github.com/btcsuite/btcd.git
# Drop genaddr under btcd/cmd/ so it inherits btcd's go.mod and pulls in
# the hdkeychain / chaincfg v2 submodules without us maintaining a
# parallel go.sum.
COPY genaddr/main.go /go/src/btcd/cmd/genaddr/main.go

# Instrument btcd for edge coverage. The antithesis-go-instrumentor rewrites
# the module into btcd-instr/customer and emits a generated notifier whose
# init() calls init_coverage_module() in /usr/lib/libvoidstar.so — our bedrock
# shim (libvoidstar.c) that registers the coverage bitmap as a feedback buffer
# keyed by the instrumentor's symbol-table name. delorean reads coverage back
# by that id. cgo must be on so the SDK's native loader is compiled in, so the
# instrumented btcd now links musl dynamically; that's fine on the alpine
# runtime image, which ships musl too.
RUN go install github.com/antithesishq/antithesis-sdk-go/tools/antithesis-go-instrumentor@latest
RUN mkdir -p /go/src/btcd-instr \
 && antithesis-go-instrumentor /go/src/btcd /go/src/btcd-instr
RUN cd /go/src/btcd-instr/customer && CGO_ENABLED=1 go install -v . ./cmd/...

# btcwallet (separate module, separate install). Instrumented like btcd; as a
# distinct module it gets its own symbol-table id, so delorean tracks btcd and
# btcwallet coverage independently. The btcwallet image ships libvoidstar.so so
# the wallet daemon registers its buffer.
RUN git clone --depth 1 https://github.com/btcsuite/btcwallet.git
RUN mkdir -p /go/src/btcwallet-instr \
 && antithesis-go-instrumentor /go/src/btcwallet /go/src/btcwallet-instr
RUN cd /go/src/btcwallet-instr/customer && CGO_ENABLED=1 go install -v . ./cmd/...

# Emit SEED=... ADDR=... so the runtime images have them as a sourceable
# env file. btcd uses ADDR for --miningaddr; btcwallet feeds SEED into
# --create's interactive seed prompt so its first default-account
# address derivation matches ADDR exactly.
RUN /go/bin/genaddr > /key.env

# Generate a self-signed RPC cert pair valid for every hostname clients
# reach btcd / btcwallet via. btcwallet refuses --noclienttls against
# non-localhost targets (config.go enforces it), so the RPC paths have
# to run with real TLS even though everything is on a private bridge.
#
# We can't reuse btcd's own gencerts here: it pins NotBefore to
# (time.Now() - 24h), and the bedrock guest boots at a fixed
# 2024-01-01 — anything built on a host later than that becomes
# "not yet valid" inside the guest. Our gencert pins NotBefore to
# 2020-01-01 (NotAfter 2099-01-01) so the validity window straddles
# the guest clock regardless of when the image is built.
COPY gencert/main.go /src/gencert/main.go
RUN mkdir -p /etc/bedrock \
 && cd /src/gencert \
 && go mod init gencert >/dev/null \
 && CGO_ENABLED=0 go build -o /go/bin/gencert . \
 && /go/bin/gencert /etc/bedrock/rpc.cert /etc/bedrock/rpc.key

# Compile the VMCALL helpers separately so the workload image doesn't
# need a C toolchain. libvoidstar.so is the coverage backend the instrumented
# btcd dlopen()s at runtime (built against musl to match the alpine btcd image).
FROM alpine:3.20 AS cbuild
RUN apk add --no-cache gcc musl-dev
COPY ready.c /src/ready.c
COPY libvoidstar.c /src/libvoidstar.c
RUN gcc -O2 -static -o /bedrock-ready /src/ready.c
RUN gcc -O2 -shared -fPIC -o /libvoidstar.so /src/libvoidstar.c

# -----------------------------------------------------------------------------
# lnd build stage. Separate from `gobuild` because lnd pins a newer Go than
# btcd; isolating it avoids disturbing the working btcd/btcwallet build.
# Instrumented for edge coverage exactly like btcd: antithesis-go-instrumentor
# rewrites the module into lnd-instr/customer and the build links the SDK's
# native coverage loader (cgo on) against our libvoidstar.so shim, so delorean
# tracks lnd coverage under its own symbol-table id. The tag set pulls in the
# RPC subservers the drivers drive (routerrpc for payinvoice, etc.). lncli is
# cgo-built here too but ships in the workload image without libvoidstar.so, so
# its SDK loader falls back to a no-op (same as btcctl).
# -----------------------------------------------------------------------------
FROM golang:1.26.3-alpine AS lndbuild
RUN apk add --no-cache git gcc musl-dev bash
WORKDIR /go/src
RUN git clone --depth 1 https://github.com/lightningnetwork/lnd.git
RUN go install github.com/antithesishq/antithesis-sdk-go/tools/antithesis-go-instrumentor@latest
RUN mkdir -p /go/src/lnd-instr \
 && antithesis-go-instrumentor /go/src/lnd /go/src/lnd-instr
RUN cd /go/src/lnd-instr/customer \
 && CGO_ENABLED=1 go install -v \
      -tags="signrpc walletrpc chainrpc invoicesrpc routerrpc peersrpc" \
      ./cmd/lnd ./cmd/lncli

# -----------------------------------------------------------------------------
# bedrock/btcd: btcd peer. Only needs the daemon + cert + the launcher
# wrapper that injects --miningaddr from /etc/bedrock/key.env.
# -----------------------------------------------------------------------------
FROM alpine:3.20 AS btcd
COPY --from=gobuild /go/bin/btcd          /usr/local/bin/btcd
# Coverage backend dlopen()ed by the instrumented btcd. btcctl (workload) is
# also cgo-built from the same module, but its image ships no libvoidstar.so,
# so the SDK loader there falls back to a no-op and registers no buffer.
COPY --from=cbuild  /libvoidstar.so       /usr/lib/libvoidstar.so
COPY --from=gobuild /key.env              /etc/bedrock/key.env
COPY --from=gobuild /etc/bedrock/rpc.cert /etc/bedrock/rpc.cert
COPY --from=gobuild /etc/bedrock/rpc.key  /etc/bedrock/rpc.key
COPY btcd-launcher.sh                     /usr/local/bin/btcd-launcher
RUN chmod +x /usr/local/bin/btcd-launcher
ENTRYPOINT ["/usr/local/bin/btcd-launcher"]

# -----------------------------------------------------------------------------
# bedrock/btcwallet: wallet daemon. Bundles `expect` to drive the
# interactive --create flow on first boot, plus the cert for both client
# (talking to btcd) and server (accepting btcctl from the workload).
# -----------------------------------------------------------------------------
FROM alpine:3.20 AS btcwallet
RUN apk add --no-cache ca-certificates expect
COPY --from=gobuild /go/bin/btcwallet     /usr/local/bin/btcwallet
# Coverage backend dlopen()ed by the instrumented btcwallet daemon.
COPY --from=cbuild  /libvoidstar.so       /usr/lib/libvoidstar.so
COPY --from=gobuild /key.env              /etc/bedrock/key.env
COPY --from=gobuild /etc/bedrock/rpc.cert /etc/bedrock/rpc.cert
COPY --from=gobuild /etc/bedrock/rpc.key  /etc/bedrock/rpc.key
COPY btcwallet-launcher.sh                /usr/local/bin/btcwallet-launcher
RUN chmod +x /usr/local/bin/btcwallet-launcher
ENTRYPOINT ["/usr/local/bin/btcwallet-launcher"]

# -----------------------------------------------------------------------------
# bedrock/lnd: Lightning node. Carries lnd + the coverage backend it
# dlopen()s, plus the shared self-signed TLS cert/key (reused as both its gRPC
# server cert and the cert it verifies btcd's RPC with) and the launcher.
# lnd1 and lnd2 both run this image, differing only by the $BTCD_HOST env the
# launcher reads.
# -----------------------------------------------------------------------------
FROM alpine:3.20 AS lnd
COPY --from=lndbuild /go/bin/lnd           /usr/local/bin/lnd
COPY --from=cbuild   /libvoidstar.so       /usr/lib/libvoidstar.so
COPY --from=gobuild  /etc/bedrock/rpc.cert /etc/bedrock/rpc.cert
COPY --from=gobuild  /etc/bedrock/rpc.key  /etc/bedrock/rpc.key
COPY lnd-launcher.sh                       /usr/local/bin/lnd-launcher
RUN chmod +x /usr/local/bin/lnd-launcher
ENTRYPOINT ["/usr/local/bin/lnd-launcher"]

# -----------------------------------------------------------------------------
# bedrock/btcd-workload: orchestrator. Carries btcctl, the cert, the
# fixed mining keypair env (for `importprivkey $WIF`), the VMCALL helper,
# the bootstrap script, and the per-container drivers.
# -----------------------------------------------------------------------------
FROM alpine:3.20 AS workload
COPY --from=gobuild  /go/bin/btcctl       /usr/local/bin/btcctl
# lncli drives the lnd nodes over the network from the drivers. Built cgo
# (instrumented) but shipped without libvoidstar.so, so its coverage loader
# no-ops here — same arrangement as btcctl.
COPY --from=lndbuild /go/bin/lncli        /usr/local/bin/lncli
COPY --from=gobuild /key.env              /etc/bedrock/key.env
COPY --from=gobuild /etc/bedrock/rpc.cert /etc/bedrock/rpc.cert
COPY --from=cbuild  /bedrock-ready        /usr/local/bin/bedrock-ready
COPY workload.sh                          /usr/local/bin/workload
COPY drivers/                             /opt/bedrock/drivers/
# Pre-create an empty btcctl.conf so btcctl's config.go:235 sees the
# file already exists and skips the "Error creating a default config
# file" stderr line on every invocation. All actual RPC settings come
# from the command-line flags the workload script and drivers pass.
RUN mkdir -p /root/.btcctl \
 && touch /root/.btcctl/btcctl.conf \
 && chmod +x /usr/local/bin/workload /opt/bedrock/drivers/*
ENTRYPOINT ["/usr/local/bin/workload"]
