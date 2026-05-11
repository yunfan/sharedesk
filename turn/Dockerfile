FROM rust:1.87-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.toml
COPY src src
RUN cargo build --release

FROM debian:bookworm-slim
WORKDIR /app
COPY --from=build /app/target/release/room-turn /usr/local/bin/room-turn
EXPOSE 3478/tcp
EXPOSE 3478/udp
CMD ["room-turn"]
