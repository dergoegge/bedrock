# syntax=docker/dockerfile:1
#
# coverage-go: an Antithesis-instrumented Go program that records edge coverage
# through our libvoidstar -> libfeedback shim, for the integration tests to read
# back. The container idles; the lab execs /opt/bedrock/drivers/cov_go (see
# tests/integration/coverage.rs). Mirrors the btcd workload's Go-instrumentation
# pattern.

FROM golang:1.25-alpine AS gobuild
# gcc/musl-dev: cgo (the SDK's native coverage loader is cgo-only, compiling to
# a no-op when CGO_ENABLED=0). bash: antithesis-go-instrumentor shells out to it
# to build the generated notifier. git: `go install` of the instrumentor.
RUN apk add --no-cache git gcc musl-dev bash
WORKDIR /src
COPY go.mod /src/go.mod
COPY main.go /src/main.go
# Instrument the module into /src-instr/customer; its generated notifier's
# init() calls init_coverage_module() in /usr/lib/libvoidstar.so at runtime.
# cgo must be on so the SDK's native loader is compiled in. --build-id (passed
# through to the external linker) makes cov_go carry a GNU build-id note, which
# libfeedback reads to key the coverage buffer ("cov-<build-id>").
RUN go install github.com/antithesishq/antithesis-sdk-go/tools/antithesis-go-instrumentor@latest
RUN mkdir -p /src-instr \
 && antithesis-go-instrumentor /src /src-instr
RUN cd /src-instr/customer \
 && go mod tidy \
 && CGO_ENABLED=1 go build -ldflags=-extldflags=-Wl,--build-id -o /cov_go .

# libvoidstar.so: the coverage backend the instrumented binary dlopen()s, built
# from our shim (libvoidstar.c + the generic libfeedback.c backend) against musl
# to match the alpine runtime. Compiled WITHOUT instrumentation — it is the
# runtime. The sources are staged into this context by build.sh from guest/.
FROM alpine:3.20 AS cbuild
RUN apk add --no-cache gcc musl-dev
COPY libvmcall.h libfeedback.h libfeedback.c libvoidstar.c /src/
RUN gcc -O2 -shared -fPIC -I/src -o /libvoidstar.so /src/libvoidstar.c /src/libfeedback.c

FROM alpine:3.20
# Lab-driven test driver, exec'd by the integration tests.
COPY --from=gobuild /cov_go /opt/bedrock/drivers/cov_go
# Coverage backend the instrumented driver dlopen()s at runtime.
COPY --from=cbuild /libvoidstar.so /usr/lib/libvoidstar.so
# Overridden by compose (entrypoint+command), but keep a sane default: idle so
# the lab can exec into the container.
ENTRYPOINT ["/bin/sh", "-c", "sleep infinity"]
