# syntax=docker/dockerfile:1
#
# Python stress script + driver executables on top of redis:7-alpine (which
# already ships redis-cli, so the runtime layer only needs python3 itself).
# A tiny statically-linked bedrock-ready helper is built in a separate stage
# and copied in — Python can't emit the VMCALL instruction directly.
# Drivers are baked at /opt/bedrock/drivers/ where the bedrock-io kernel
# module enumerates them per-container for the ACTION_GET_WORKLOAD_DETAILS RPC.

FROM alpine:3.20 AS build
RUN apk add --no-cache gcc musl-dev
COPY ready.c /src/ready.c
RUN gcc -O2 -static -o /bedrock-ready /src/ready.c

FROM redis:7-alpine
RUN apk add --no-cache python3
COPY --from=build /bedrock-ready /usr/local/bin/bedrock-ready
COPY stress.py /usr/local/bin/bedrock-stress
COPY drivers/ /opt/bedrock/drivers/
RUN chmod +x /usr/local/bin/bedrock-stress /opt/bedrock/drivers/*
ENTRYPOINT ["/usr/local/bin/bedrock-stress"]
