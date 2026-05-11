#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="x86_64-unknown-linux-musl"
OUT_DIR="${ROOT_DIR}/dist"
BIN_NAME="room-server"

export CARGO_TARGET_DIR="${ROOT_DIR}/target"

rustup target add "${TARGET}" >/dev/null
cargo build --manifest-path "${ROOT_DIR}/Cargo.toml" --release --target "${TARGET}"

mkdir -p "${OUT_DIR}"
cp "${ROOT_DIR}/target/${TARGET}/release/${BIN_NAME}" "${OUT_DIR}/${BIN_NAME}-linux-musl"
chmod +x "${OUT_DIR}/${BIN_NAME}-linux-musl"

echo "built ${OUT_DIR}/${BIN_NAME}-linux-musl"
