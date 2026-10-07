#!/usr/bin/env bash
# Pins action.yml to a published release's binaries: scripts/pin-release.sh v0.4.0 [owner/repo]
set -euo pipefail
tag=$1
repo=${2:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}
sums=$(gh release download "$tag" --repo "$repo" --pattern SHA256SUMS --output -)
digest() { awk -v f="$1" '$2 == f || $2 == "*"f { print $1 }' <<<"$sums"; }
root=$(cd "$(dirname "$0")/.." && pwd)
for pair in "KERN_VERSION=$tag" "KERN_SHA256_LINUX_X64=$(digest kern-linux-x64)" "KERN_SHA256_WINDOWS_X64=$(digest kern-windows-x64.exe)" "KERN_SHA256_MACOS_ARM64=$(digest kern-macos-arm64)"; do
  key=${pair%%=*} value=${pair#*=}
  [[ "$value" =~ ^(v[0-9][0-9A-Za-z.+-]*|[0-9a-f]{64})$ ]] || { echo "Release $tag has no valid value for $key" >&2; exit 1; }
  sed -i.bak "s|$key: \".*\"|$key: \"$value\"|" "$root/action.yml"
done
rm -f "$root/action.yml.bak"
git -C "$root" --no-pager diff -- action.yml
echo "Commit action.yml, then pin workflows to that commit: uses: $repo@<commit-sha>"
