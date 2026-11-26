# The machine the README pictures are taken on; see tests/demo/run.sh.
FROM debian:13
RUN apt-get update -q && DEBIAN_FRONTEND=noninteractive apt-get install -yq --no-install-recommends \
        build-essential qt6-base-dev qt6-base-dev-tools libgl-dev python3 curl ca-certificates cargo \
        qt6-svg-plugins breeze-icon-theme fonts-noto-core fonts-noto-mono plasma-integration \
        xvfb xdotool x11-utils procps ffmpeg imagemagick \
    && rm -rf /var/lib/apt/lists/*
COPY . /src
RUN cd /src && TRAWLR_NO_START=1 ./install.sh
