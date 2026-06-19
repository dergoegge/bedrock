# syntax=docker/dockerfile:1
#
# coverage-pcguard: a program instrumented with LLVM
# -fsanitize-coverage=trace-pc-guard whose edge coverage is recorded through our
# libpcguard -> libfeedback shim, for the integration tests to read back. The
# container idles; the lab execs /opt/bedrock/drivers/cov_pcguard (see
# tests/integration/coverage.rs).
#
# The instrumented target (cov_target.c) is built with clang and the coverage
# flag; the runtime (libpcguard.c, libfeedback.c) is built WITHOUT it, or its
# hooks would instrument and recurse into themselves. clang only emits the
# instrumented object (gcc doesn't support trace-pc-guard); gcc does the static
# link. -Wl,--build-id makes the binary carry a GNU build-id note, which
# libfeedback reads to key the coverage buffer ("cov-<build-id>").
# libfeedback.c/libpcguard.c/libfeedback.h/libvmcall.h are staged into this
# build context by build.sh from guest/ (outside the context).

FROM alpine:3.20 AS build
RUN apk add --no-cache clang gcc musl-dev
COPY libvmcall.h libfeedback.h libfeedback.c libpcguard.c cov_target.c /src/
WORKDIR /src
RUN clang -O2 -fsanitize-coverage=trace-pc-guard -I. -c cov_target.c -o cov_target.o
RUN gcc -O2 -I. -c libpcguard.c -o libpcguard.o
RUN gcc -O2 -I. -c libfeedback.c -o libfeedback.o
RUN gcc -O2 -static -Wl,--build-id -o /cov_pcguard cov_target.o libpcguard.o libfeedback.o

FROM alpine:3.20
# Lab-driven test driver, exec'd by the integration tests.
COPY --from=build /cov_pcguard /opt/bedrock/drivers/cov_pcguard
# Overridden by compose (entrypoint+command), but keep a sane default: idle so
# the lab can exec into the container.
ENTRYPOINT ["/bin/sh", "-c", "sleep infinity"]
