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
        fonts-dejavu libpango-1.0-0 libpangoft2-1.0-0 libharfbuzz0b libharfbuzz-subset0 libfontconfig1 \
    && rm -rf /var/lib/apt/lists/* \
    && rm -f /usr/lib/python3*/EXTERNALLY-MANAGED

# The renderer behind cedar's `generate_pdf` tool: Markdown or HTML in, PDF out, via WeasyPrint.
# WeasyPrint comes from PyPI rather than apt because Debian's package pulls in scipy and a C++
# toolchain (over 1 GB); the pango and fontconfig libraries above are all it needs from the OS.
# The script is a plain command, so the agent can also call it from `bash`.
RUN pip install --no-cache-dir weasyprint markdown
COPY --chmod=0755 render_pdf.py /usr/local/bin/cedar-render-pdf

# The libraries behind `generate_chart` (matplotlib), `read_document` (pypdf, python-docx,
# openpyxl, python-pptx), and the page images of attached PDFs (pypdfium2, pillow). They are
# ordinary packages too, so the agent can also use them directly from `run_python`. MPLBACKEND
# keeps matplotlib off any display backend.
RUN pip install --no-cache-dir matplotlib pypdf python-docx openpyxl python-pptx pypdfium2 pillow
COPY --chmod=0755 render_chart.py /usr/local/bin/cedar-render-chart
COPY --chmod=0755 read_document.py /usr/local/bin/cedar-read-document
COPY --chmod=0755 render_pages.py /usr/local/bin/cedar-render-pages

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
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    MPLBACKEND=Agg

USER agent
WORKDIR /workspace
