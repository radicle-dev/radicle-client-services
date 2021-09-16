# Build
FROM rustlang/rust:nightly-slim@sha256:54296ad56b743b198e020bff1777995f83746fb7cd938847c2d4fa62f8bf630f as build

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
ENTRYPOINT ["/usr/local/bin/radicle-git-server", "--root", "/app/radicle/root", "--listen", "0.0.0.0:8778"]
