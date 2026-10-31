# cedar-contrib, the contribution ingest, as Cloud Run runs it. Build from the repository root:
#   docker build -f contrib/Dockerfile -t cedar-contrib .
FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY contrib-schema ./contrib-schema
COPY contrib ./contrib
COPY pairing ./pairing
COPY chat ./chat
COPY src ./src
COPY migrations ./migrations
RUN cargo build --release -p cedar-contrib --locked

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/cedar-contrib /usr/local/bin/cedar-contrib
USER 65534:65534
ENV RUST_LOG=info
ENTRYPOINT ["/usr/local/bin/cedar-contrib"]
