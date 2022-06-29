# Build
FROM rust:1.61.0-slim@sha256:91ab0966aa0d8eff103f42c04e0f4dd0bc628d1330942616a94bbe260f26fe6e as build

RUN apt-get update && apt-get install -y pkg-config libssl-dev git cmake

WORKDIR /usr/src/radicle-client-services
COPY . .

WORKDIR /usr/src/radicle-client-services/git-server
RUN set -eux; \
    cargo install --profile=container --all-features --locked --path .; \
    objcopy --compress-debug-sections /usr/local/cargo/bin/radicle-git-server /usr/local/cargo/bin/radicle-git-server.compressed; \
    objcopy --compress-debug-sections /usr/local/cargo/bin/pre-receive /usr/local/cargo/bin/pre-receive.compressed; \
    objcopy --compress-debug-sections /usr/local/cargo/bin/post-receive /usr/local/cargo/bin/post-receive.compressed

# Run
FROM debian:bullseye-slim@sha256:4c25ffa6ef572cf0d57da8c634769a08ae94529f7de5be5587ec8ce7b9b50f9c

RUN echo deb http://deb.debian.org/debian bullseye-backports main contrib non-free >/etc/apt/sources.list.d/backports.list
RUN apt-get update && apt-get install -y libssl1.1 && apt -t bullseye-backports install --yes git && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/cargo/bin/radicle-git-server.compressed /usr/local/bin/radicle-git-server
COPY --from=build /usr/local/cargo/bin/pre-receive.compressed /usr/local/bin/pre-receive
COPY --from=build /usr/local/cargo/bin/post-receive.compressed /usr/local/bin/post-receive
COPY --from=build /usr/src/radicle-client-services/git-server/docker/radicle-git-server.sh /usr/local/bin/radicle-git-server.sh

WORKDIR /app/radicle

ENTRYPOINT ["/usr/local/bin/radicle-git-server.sh", "--listen", "0.0.0.0:8778", "--git-receive-pack", "--root", "/app/radicle"]
