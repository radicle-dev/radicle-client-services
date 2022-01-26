# Build
FROM rust:1.57.0-slim@sha256:43d8c9a7e8f0784372a4c0009b064b9f40051f4f950c7c96b9650ad8b445add4 as build

RUN apt-get update && apt-get install -y pkg-config libssl-dev git cmake

WORKDIR /usr/src/radicle-client-services
COPY . .

WORKDIR /usr/src/radicle-client-services/git-server
RUN cargo install --all-features --locked --path .

WORKDIR /usr/src/radicle-client-services/service-init
RUN cargo install --all-features --locked --path .

# Run
FROM debian:bullseye-slim@sha256:96e65f809d753e35c54b7ba33e59d92e53acc8eb8b57bf1ccece73b99700b3b0

RUN echo deb http://deb.debian.org/debian bullseye-backports main contrib non-free >/etc/apt/sources.list.d/backports.list
RUN apt-get update && apt-get install -y libssl1.1 && apt -t bullseye-backports install --yes git && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/cargo/bin/radicle-git-server /usr/local/bin/radicle-git-server
COPY --from=build /usr/local/cargo/bin/pre-receive /usr/local/bin/pre-receive
COPY --from=build /usr/local/cargo/bin/post-receive /usr/local/bin/post-receive
COPY --from=build /usr/local/cargo/bin/radicle-service-init /usr/local/bin/radicle-service-init
COPY --from=build /usr/src/radicle-client-services/git-server/docker/start-up.sh /usr/local/bin/start-up-radicle-git-server.sh

WORKDIR /app/radicle

ENTRYPOINT ["/usr/local/bin/start-up-radicle-git-server.sh"]
