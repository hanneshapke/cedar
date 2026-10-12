# The environment an agent's tools run in. Build with:
#
#     docker build -t cedar-sandbox sandbox/
#
# Keep it small and unprivileged. cedar starts it with dropped capabilities and memory/CPU/pid
# limits (see src/sandbox.rs), and without network unless SANDBOX_NETWORK says otherwise. Anything
# the agent should be able to run offline has to be installed here; with network access it can
# also `pip install` packages itself (see below).
FROM debian:bookworm-slim

# Debian marks its python as "externally managed", which makes `pip install` refuse to run outside
# a venv. The container is disposable, so that marker is removed and pip works directly.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates coreutils curl git jq python3 python3-pip python3-venv \
    && rm -rf /var/lib/apt/lists/* \
    && rm -f /usr/lib/python3*/EXTERNALLY-MANAGED

# uid/gid 1000 must match the `--user` cedar passes to `docker run`. /workspace is created and
# owned by that user so the per-run named volume mounted there inherits the right ownership.
RUN groupadd --gid 1000 agent \
    && useradd --uid 1000 --gid 1000 --create-home --shell /bin/sh agent \
    && mkdir /workspace \
    && chown agent:agent /workspace

# The agent can't write to the system site-packages, so pip falls back to a user install. Point
# that at the workspace: the workspace volume outlives the container, so packages installed in
# one turn are still there in the next. Scripts the packages install land on PATH the same way.
ENV PYTHONUSERBASE=/workspace/.local \
    PATH=/workspace/.local/bin:/usr/local/bin:/usr/bin:/bin \
    PIP_DISABLE_PIP_VERSION_CHECK=1

USER agent
WORKDIR /workspace
