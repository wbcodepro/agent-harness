# hame-relay container image.
#
# Build from the repository root (the workspace):
#   docker build -f crates/hame-relay/Dockerfile -t hame-relay .
#
# The runtime image is distroless (glibc, CA certificates, no shell) and
# runs as the non-root user 65532. The database lives in /data.

FROM rust:1-bookworm AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake clang \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p hame-relay \
    && cp target/release/hame-relay /hame-relay \
    && strip /hame-relay \
    && mkdir -p /data

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /hame-relay /usr/local/bin/hame-relay
COPY --from=build --chown=65532:65532 /data /data
USER 65532:65532
ENV RELAY_BIND=0.0.0.0:8080 \
    RELAY_DATABASE_PATH=/data/relay.db \
    RELAY_LOG_FORMAT=json
VOLUME ["/data"]
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/hame-relay", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/hame-relay"]
