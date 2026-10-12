# The browser behind cedar's `browser` tool: headless Chromium driven by Playwright, kept
# running for the length of a run so pages, dialogs, and scroll positions survive between calls.
# Build with:
#
#     docker build -t cedar-browser browser/
#
# The container's command starts the browser and waits for calls on a Unix socket; each call is a
# `docker exec`, like the sandbox's tools:
#
#     docker run -d --name cedar-browser-try --shm-size 256m cedar-browser
#     docker exec cedar-browser-try cedar-browser call '{"action":"navigate","url":"https://example.com"}'
#     docker exec cedar-browser-try cedar-browser call '{"action":"click","ref":3}'
#     docker rm -f cedar-browser-try
#
# See driver.py for the request and reply format. It is a separate image from the sandbox because
# Chromium and its libraries add a few hundred MB that most runs never use.
FROM debian:bookworm-slim

# As in the sandbox image, Debian's "externally managed" marker is removed so pip installs into
# the system Python of this disposable container.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates python3 python3-pip \
    && rm -rf /var/lib/apt/lists/* \
    && rm -f /usr/lib/python3*/EXTERNALLY-MANAGED

# Only Chromium's headless shell, which is all a headless browser needs, plus the system libraries
# it links against. Browsers go to a fixed path so the unprivileged user below can run them.
ENV PLAYWRIGHT_BROWSERS_PATH=/ms-playwright
RUN pip install --no-cache-dir playwright==1.56.0 \
    && playwright install --with-deps --only-shell chromium \
    && rm -rf /var/lib/apt/lists/*
COPY --chmod=0755 driver.py /usr/local/bin/cedar-browser

RUN groupadd --gid 1000 browser \
    && useradd --uid 1000 --gid 1000 --create-home --shell /bin/sh browser

USER browser
WORKDIR /home/browser
CMD ["cedar-browser", "serve"]
