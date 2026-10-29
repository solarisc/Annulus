# The Annulus sync server (ADR-0008). Build from the repository root:
#   docker build -f deploy/Dockerfile -t annulus-server .
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p annulus-server

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --home /data annulus && mkdir /data && chown annulus /data
COPY --from=build /src/target/release/annulus-server /usr/local/bin/annulus-server
USER annulus
VOLUME /data
EXPOSE 8443
# Plain HTTP inside the container network; TLS comes from the proxy in front (see
# compose.yaml) or from --tls-cert/--tls-key.
ENTRYPOINT ["annulus-server"]
CMD ["serve", "--listen", "0.0.0.0:8443", "--data", "/data/annulus-server.db", "--behind-proxy"]
