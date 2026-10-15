#!/usr/bin/env bash
set -euo pipefail
src=${KERN_SOURCE:-$GITHUB_ACTION_PATH}
case "${RUNNER_OS}-${RUNNER_ARCH}" in
  Linux-X64) asset=kern-linux-x64 digest=${KERN_SHA256_LINUX_X64:-} ;;
  Linux-ARM64) asset=kern-linux-arm64 digest=${KERN_SHA256_LINUX_ARM64:-} ;;
  Windows-X64 | Windows-ARM64) asset=kern-windows-x64.exe digest=${KERN_SHA256_WINDOWS_X64:-} ;;
  macOS-ARM64) asset=kern-macos-arm64 digest=${KERN_SHA256_MACOS_ARM64:-} ;;
  macOS-X64) asset=kern-macos-x64 digest=${KERN_SHA256_MACOS_X64:-} ;;
  *) asset="" digest="" ;;
esac
if [[ "${BUILD_FROM_SOURCE:-false}" == "true" || -z "${KERN_VERSION:-}" || -z "$digest" ]]; then
  cd "$src"
  if [[ "${KERN_STATIC:-false}" == "true" && "$RUNNER_OS" == "Linux" ]]; then
    target="$(uname -m)-unknown-linux-musl"
    command -v musl-gcc >/dev/null || { sudo apt-get update -qq && sudo apt-get install -y -qq musl-tools >/dev/null; }
    rustup target add "$target" >/dev/null
    export "CC_${target//-/_}=musl-gcc"
    cargo build --release --locked --quiet --target "$target" --manifest-path "$src/Cargo.toml"
    bin="$src/target/$target/release/kern"
  else
    cargo build --release --locked --quiet --manifest-path "$src/Cargo.toml"
    bin="$src/target/release/kern"
  fi
  echo "bin=$bin" >> "$GITHUB_OUTPUT"
  echo "KERN_BIN=$bin" >> "$GITHUB_ENV"
  exit 0
fi
bin="$RUNNER_TEMP/$asset"
curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 --retry 3 --output "$bin" \
  "https://github.com/$GITHUB_ACTION_REPOSITORY/releases/download/$KERN_VERSION/$asset"
if command -v sha256sum >/dev/null; then actual=$(sha256sum < "$bin"); else actual=$(shasum -a 256 < "$bin"); fi
actual=${actual%% *}
if [[ "$actual" != "$digest" ]]; then
  rm -f "$bin"
  echo "::error::Kern $KERN_VERSION ($asset) digest mismatch: expected $digest, got $actual"
  exit 1
fi
chmod +x "$bin"
echo "bin=$bin" >> "$GITHUB_OUTPUT"
echo "KERN_BIN=$bin" >> "$GITHUB_ENV"
