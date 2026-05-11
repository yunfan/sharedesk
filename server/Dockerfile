FROM rust:1.87-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.toml
COPY src src
RUN cargo build --release

FROM debian:bookworm-slim
WORKDIR /app
COPY --from=build /app/target/release/room-server /usr/local/bin/room-server
EXPOSE 38080
CMD ["room-server"]
