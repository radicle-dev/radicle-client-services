# Build
FROM rust:1.61.0-slim@sha256:91ab0966aa0d8eff103f42c04e0f4dd0bc628d1330942616a94bbe260f26fe6e as build

RUN apt-get update && apt-get install -y pkg-config libssl-dev git cmake

WORKDIR /usr/src/radicle-client-services
COPY . .

WORKDIR /usr/src/radicle-client-services/http-api
RUN set -eux; \
    cargo install --profile=container --all-features --locked --path .; \
    objcopy --compress-debug-sections /usr/local/cargo/bin/radicle-http-api /usr/local/cargo/bin/radicle-http-api.compressed

# Run
FROM debian:bullseye-slim@sha256:4c25ffa6ef572cf0d57da8c634769a08ae94529f7de5be5587ec8ce7b9b50f9c

EXPOSE 8777/tcp
RUN echo deb http://deb.debian.org/debian bullseye-backports main contrib non-free >/etc/apt/sources.list.d/backports.list
RUN apt-get update && apt-get install -y libssl1.1 && apt -t bullseye-backports install --yes git && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/cargo/bin/radicle-http-api.compressed /usr/local/bin/radicle-http-api
WORKDIR /app/radicle
ENTRYPOINT ["/usr/local/bin/radicle-http-api", "--listen", "0.0.0.0:8777", "--root", "/app/radicle"]
