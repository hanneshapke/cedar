# The desktop behind cedar's `computer` tool: an Xfce desktop on a virtual screen (Xvfb), with a
# web browser, an office suite, a file manager, a text editor and a terminal, kept running for the
# length of a run. The model looks at it through screenshots and works it with the mouse and
# keyboard. Build with:
#
#     docker build -t cedar-computer computer/
#
# The container's command starts the screen and the desktop; each call is a `docker exec`, like
# the browser's:
#
#     docker run -d --name cedar-computer-try --shm-size 512m cedar-computer
#     docker exec cedar-computer-try cedar-computer call '{"action":"screenshot"}'
#     docker exec cedar-computer-try cedar-computer call '{"action":"click","x":40,"y":12}'
#     docker rm -f cedar-computer-try
#
# See driver.py for the request and reply format. It is a separate image from the sandbox because
# a desktop and its applications add well over a GB that most runs never use.
FROM debian:bookworm-slim

# One layer: the screen and what works it (Xvfb, xdotool, scrot, Pillow to scale screenshots),
# the desktop, and the applications. Firefox gets a policy that keeps its first-run pages,
# update checks and telemetry out of the way.
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates python3 python3-pil xvfb xdotool scrot dbus-x11 xdg-utils x11-xserver-utils \
        xfwm4 xfce4-panel xfce4-session xfce4-settings xfdesktop4 thunar xfce4-terminal \
        mousepad ristretto firefox-esr \
        libreoffice-calc libreoffice-writer libreoffice-impress libreoffice-gtk3 \
        adwaita-icon-theme fonts-dejavu fonts-liberation fonts-noto-color-emoji \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /usr/lib/firefox-esr/distribution /tmp/.X11-unix \
    && chmod 1777 /tmp/.X11-unix
COPY firefox-policies.json /usr/lib/firefox-esr/distribution/policies.json
COPY config/ /usr/local/share/cedar-computer/
COPY --chmod=0755 driver.py /usr/local/bin/cedar-computer

# The same user id as the sandbox's, so both can work on the workspace's files. The user starts
# with a desktop that is quiet to look at (see config/): a plain background, one panel, and
# LibreOffice past its first-start dialogs.
RUN groupadd --gid 1000 user \
    && useradd --uid 1000 --gid 1000 --create-home --shell /bin/bash user \
    && xfconf=/home/user/.config/xfce4/xfconf/xfce-perchannel-xml \
    && office=/home/user/.config/libreoffice/4/user \
    && mkdir -p /workspace "$xfconf" "$office" \
    && cp /usr/local/share/cedar-computer/xf*.xml "$xfconf/" \
    && version=$(dpkg-query -W -f='${Version}' libreoffice-core | sed -E 's/^([0-9]+:)?([0-9]+\.[0-9]+).*/\2/') \
    && sed "s/@LIBREOFFICE_VERSION@/$version/" /usr/local/share/cedar-computer/registrymodifications.xcu \
        > "$office/registrymodifications.xcu" \
    && chown -R user:user /home/user /workspace

ENV DISPLAY=:1 LANG=C.UTF-8
USER user
WORKDIR /workspace
CMD ["cedar-computer", "serve"]
