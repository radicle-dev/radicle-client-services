# Build
FROM rustlang/rust:nightly-slim@sha256:54296ad56b743b198e020bff1777995f83746fb7cd938847c2d4fa62f8bf630f as build

WORKDIR /usr/src/radicle-client-services
COPY . .

WORKDIR /usr/src/radicle-client-services/http-api
RUN cargo install --path .

# Run
FROM debian:buster-slim

EXPOSE 8777/tcp
COPY --from=build /usr/local/cargo/bin/radicle-http-api /usr/local/bin/radicle-http-api
WORKDIR /app/radicle
ENTRYPOINT ["/usr/local/bin/radicle-http-api", "--root", "/app/radicle/root", "--listen", "0.0.0.0:8777", "--no-color"]
