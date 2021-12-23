# Build
FROM rust:1.57.0-slim@sha256:43d8c9a7e8f0784372a4c0009b064b9f40051f4f950c7c96b9650ad8b445add4 as build

RUN apt-get update && apt-get install -y pkg-config libssl-dev git cmake

WORKDIR /usr/src/radicle-client-services
COPY . .

WORKDIR /usr/src/radicle-client-services/git-server
RUN cargo install --all-features --locked --path .

# Run
FROM debian:buster-slim@sha256:c8152821b158dd171b4acf92afb0a58fc2faa179a7e0af8ace358fbe1668e99d

EXPOSE 8778/tcp
RUN apt-get update && apt-get install -y libssl1.1 git && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/cargo/bin/radicle-git-server /usr/local/bin/radicle-git-server

WORKDIR /app/radicle

ENTRYPOINT ["/usr/local/bin/radicle-git-server", "--git-receive-pack", "--root", "/app/radicle/root", "--listen", "0.0.0.0:8778"]
