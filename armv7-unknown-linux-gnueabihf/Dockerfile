FROM ghcr.io/cross-rs/armv7-unknown-linux-gnueabihf:main

COPY armv7-unknown-linux-gnueabihf/apt.list /etc/apt/sources.list
RUN dpkg --add-architecture armhf && \
      apt-get update && \
      apt-get install --assume-yes libssl-dev:armhf
