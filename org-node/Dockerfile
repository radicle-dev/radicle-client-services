# Build
FROM rustlang/rust:nightly-slim@sha256:54296ad56b743b198e020bff1777995f83746fb7cd938847c2d4fa62f8bf630f as build

RUN apt-get update && apt-get install -y pkg-config libssl-dev

WORKDIR /usr/src/radicle-client-services
COPY . .

WORKDIR /usr/src/radicle-client-services/org-node
RUN cargo install --path .

# Run
FROM debian:buster-slim

EXPOSE 8776/udp
COPY --from=build /usr/local/cargo/bin/radicle-org-node /usr/local/bin/radicle-org-node
WORKDIR /app/radicle
ENTRYPOINT [ \
  "/usr/local/bin/radicle-org-node", \
  "--root", "/app/radicle/root", \
  "--identity", "/app/radicle/identity", \
  "--cache", "/app/radicle/org-node.json", \
  "--no-color", \
  "--listen", "0.0.0.0:8776" ]
