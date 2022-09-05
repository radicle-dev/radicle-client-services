# Build
FROM rust:1.61.0-slim@sha256:91ab0966aa0d8eff103f42c04e0f4dd0bc628d1330942616a94bbe260f26fe6e as build

RUN apt-get update && apt-get install -y pkg-config libssl-dev git cmake

RUN git clone --depth 1 https://github.com/radicle-dev/radicle-link.git /usr/src/radicle-link
WORKDIR /usr/src/radicle-link
RUN git reset --hard 622c1bcd59a6ce584f957ffe6b874b2af0b207fd

WORKDIR /usr/src/radicle-link/bins/linkd
RUN set -eux; \
    cargo install --locked --path .; \
    objcopy --compress-debug-sections /usr/local/cargo/bin/linkd /usr/local/cargo/bin/radicle-linkd.compressed

# Run
FROM debian:bullseye-slim@sha256:4c25ffa6ef572cf0d57da8c634769a08ae94529f7de5be5587ec8ce7b9b50f9c

EXPOSE 8777/tcp
RUN echo deb http://deb.debian.org/debian bullseye-backports main contrib non-free >/etc/apt/sources.list.d/backports.list
RUN apt-get update && apt-get install -y libssl1.1 && apt -t bullseye-backports install --yes git && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/cargo/bin/radicle-linkd.compressed /usr/local/bin/radicle-linkd
WORKDIR /app/radicle
ENTRYPOINT ["/usr/local/bin/radicle-linkd", "--protocol-listen", "0.0.0.0:8776", "--lnk-home", "/app/radicle", "--track", "everything", "--signer", "key", "--key-format", "binary", "--key-source", "file", "--key-file-path", "/app/radicle/linkd.key"]
