#!/usr/bin/env bash
# Installs the Kern binary for the GitHub Action: the release asset pinned in action.yml, or a source build.
set -euo pipefail
case "${RUNNER_OS}-${RUNNER_ARCH}" in
  Linux-X64) asset=kern-linux-x64 digest=${KERN_SHA256_LINUX_X64:-} ;;
  Windows-X64) asset=kern-windows-x64.exe digest=${KERN_SHA256_WINDOWS_X64:-} ;;
  macOS-ARM64) asset=kern-macos-arm64 digest=${KERN_SHA256_MACOS_ARM64:-} ;;
  *) asset="" digest="" ;;
esac
if [[ "${BUILD_FROM_SOURCE:-false}" == "true" || -z "${KERN_VERSION:-}" || -z "$digest" ]]; then
  cd "$GITHUB_ACTION_PATH"
  cargo build --release --locked --quiet --manifest-path "$GITHUB_ACTION_PATH/Cargo.toml"
  bin="$GITHUB_ACTION_PATH/target/release/kern"
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
