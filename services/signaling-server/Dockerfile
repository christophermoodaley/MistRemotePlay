# Build: docker build -f services/signaling-server/Dockerfile .
FROM rust:1.89-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p mist-signaling-server

FROM debian:bookworm-slim
RUN useradd --system --no-create-home mist
COPY --from=build /src/target/release/mist-signaling-server /usr/local/bin/
USER mist
EXPOSE 8443
ENTRYPOINT ["/usr/local/bin/mist-signaling-server"]
