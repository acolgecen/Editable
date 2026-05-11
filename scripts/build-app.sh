#!/usr/bin/env bash
set -euo pipefail

APP_PATH="${APP_PATH:-dist/Editable.app}"
SIGN_IDENTITY="${SIGN_IDENTITY:--}"

cargo build --release -p editable
cargo run --release -p editable-build-tools -- icon
cargo run --release -p editable-build-tools -- bundle

echo "Signing ${APP_PATH} with identity '${SIGN_IDENTITY}'"
codesign --force --deep --sign "${SIGN_IDENTITY}" "${APP_PATH}"

echo "Verifying signature for ${APP_PATH}"
codesign --verify --deep --strict --verbose=2 "${APP_PATH}"
