#!/bin/sh
# Installs Kern on Linux or macOS from a GitHub release, checked against the release's SHA256SUMS:
#   curl -fsSL https://github.com/OWNER/kern/releases/latest/download/install.sh | sh
# KERN_VERSION=v0.4.0 picks the release, KERN_INSTALL_DIR the folder (default ~/.local/bin). On Windows, use
# install.ps1 from the same release in PowerShell. The release workflow writes its repository and tag into this file.
set -eu

repo="${KERN_REPO:-__KERN_REPO__}"
version="${KERN_VERSION:-__KERN_VERSION__}"
dir="${KERN_INSTALL_DIR:-$HOME/.local/bin}"

fail() { printf 'kern install: %s\n' "$1" >&2; exit 1; }
case "$repo$version" in *__KERN_*) fail "this copy of install.sh is not from a release; set KERN_REPO and KERN_VERSION" ;; esac
base="${KERN_DOWNLOAD_URL:-https://github.com/$repo/releases/download/$version}"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64 | Linux-amd64) asset=kern-linux-x64 ;;
  Linux-aarch64 | Linux-arm64) asset=kern-linux-arm64 ;;
  Darwin-arm64) asset=kern-macos-arm64 ;;
  Darwin-x86_64)
    # A shell under Rosetta on an Apple silicon Mac says x86_64; the native build is the right one there.
    if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then asset=kern-macos-arm64; else asset=kern-macos-x64; fi ;;
  MINGW* | MSYS* | CYGWIN*) fail "on Windows, run this in PowerShell instead: irm $base/install.ps1 | iex" ;;
  *) fail "there is no Kern build for $(uname -s) $(uname -m)" ;;
esac

# Downloads use HTTPS only, unless KERN_DOWNLOAD_URL names a mirror of your own.
fetch() {
  if command -v curl >/dev/null 2>&1; then
    case "$1" in
      https://*) curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$2" "$1" ;;
      *) curl -fsSL --retry 3 -o "$2" "$1" ;;
    esac
  elif command -v wget >/dev/null 2>&1; then
    case "$1" in
      https://*) wget -q --https-only -O "$2" "$1" ;;
      *) wget -q -O "$2" "$1" ;;
    esac
  else
    fail "curl or wget is needed to download Kern"
  fi
}
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{ print $1 }'
  elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | awk '{ print $1 }'
  else fail "sha256sum or shasum is needed to check the download"
  fi
}

tmp=$(mktemp -d 2>/dev/null || mktemp -d -t kern) || fail "cannot create a temporary folder"
trap 'rm -rf "$tmp"' EXIT
trap 'exit 1' INT TERM
fetch "$base/$asset" "$tmp/$asset" || fail "could not download $base/$asset"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || fail "could not download $base/SHA256SUMS"
want=$(awk -v f="$asset" '$2 == f || $2 == "*" f { print $1; exit }' "$tmp/SHA256SUMS")
[ -n "$want" ] || fail "SHA256SUMS of $version does not list $asset; nothing was installed"
got=$(sha256 "$tmp/$asset")
[ "$got" = "$want" ] || fail "the download does not match SHA256SUMS (expected $want, got $got); nothing was installed"

# Tried where it will live (some systems forbid running programs from /tmp), then moved into place in one step,
# so a failed install never leaves a broken kern behind.
mkdir -p "$dir" || fail "cannot create $dir"
cp "$tmp/$asset" "$dir/.kern.new" || fail "cannot write to $dir; set KERN_INSTALL_DIR to a folder you can write"
chmod 755 "$dir/.kern.new"
if ! "$dir/.kern.new" --version >/dev/null 2>&1; then rm -f "$dir/.kern.new"; fail "the downloaded Kern does not run on this machine"; fi
mv -f "$dir/.kern.new" "$dir/kern" || fail "cannot replace $dir/kern"

printf 'Installed Kern %s to %s/kern\n' "$version" "$dir"
# shellcheck disable=SC2016 # the line shown to the user keeps $PATH as written
case ":$PATH:" in
  *":$dir:"*) ;;
  *) printf 'Add %s to your PATH, for example with this line in ~/.profile (or ~/.zshrc on a Mac):\n  export PATH="%s:$PATH"\n' "$dir" "$dir" ;;
esac
