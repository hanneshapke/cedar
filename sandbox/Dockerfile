# The environment an agent's tools run in. Build with:
#
#     docker build -t cedar-sandbox sandbox/
#
# Keep it small and unprivileged. cedar starts it with no network, dropped capabilities and
# memory/CPU/pid limits (see src/sandbox.rs); anything the agent should be able to run has to be
# installed here, because it can't install packages itself while offline.
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates coreutils curl git jq python3 \
    && rm -rf /var/lib/apt/lists/*

# uid/gid 1000 must match the `--user` cedar passes to `docker run`. /workspace is created and
# owned by that user so the per-run named volume mounted there inherits the right ownership.
RUN groupadd --gid 1000 agent \
    && useradd --uid 1000 --gid 1000 --create-home --shell /bin/sh agent \
    && mkdir /workspace \
    && chown agent:agent /workspace

USER agent
WORKDIR /workspace
