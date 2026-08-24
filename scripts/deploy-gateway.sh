#!/usr/bin/env bash
set -euo pipefail

readonly DEFAULT_HOST="rob@blueberry.lab.reb.gg"
readonly DEFAULT_TARGET="aarch64-unknown-linux-gnu"
readonly DEFAULT_BIN_DIR="/home/rob/.local/bin"
readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly PROJECT_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"

host="${PICTO_RELAY_PI_HOST:-${DEFAULT_HOST}}"
target="${PICTO_RELAY_PI_TARGET:-${DEFAULT_TARGET}}"
bin_dir="${PICTO_RELAY_PI_BIN_DIR:-${DEFAULT_BIN_DIR}}"

usage() {
  cat <<EOF
Usage: $(basename "$0") [--host USER@HOST] [--destination PATH] [--target TRIPLE]

Cross-compile and deploy the Picto Relay JSON and web server to the Pi.

Options:
  --host VALUE        SSH destination (default: ${DEFAULT_HOST}).
  --destination PATH  Remote binary directory (default: ${DEFAULT_BIN_DIR}).
  --target TRIPLE     Rust target (default: ${DEFAULT_TARGET}).
  -h, --help          Show this help.
EOF
}

while (($#)); do
  case "$1" in
    --host)
      [[ $# -ge 2 ]] || { echo "error: --host requires a value" >&2; exit 2; }
      host="$2"
      shift 2
      ;;
    --destination)
      [[ $# -ge 2 ]] || { echo "error: --destination requires a value" >&2; exit 2; }
      bin_dir="$2"
      shift 2
      ;;
    --target)
      [[ $# -ge 2 ]] || { echo "error: --target requires a value" >&2; exit 2; }
      target="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

command -v cargo-zigbuild >/dev/null 2>&1 || {
  echo "error: cargo-zigbuild is required" >&2
  exit 1
}
command -v zig >/dev/null 2>&1 || {
  echo "error: zig is required" >&2
  exit 1
}
command -v rsync >/dev/null 2>&1 || {
  echo "error: rsync is required" >&2
  exit 1
}
command -v ssh >/dev/null 2>&1 || {
  echo "error: ssh is required" >&2
  exit 1
}
[[ "$bin_dir" =~ ^/[a-zA-Z0-9._/-]+$ ]] || {
  echo "error: destination must be an absolute safe path" >&2
  exit 2
}

printf 'Building gateway for %s\n' "$target"
cargo zigbuild \
  --manifest-path "${PROJECT_ROOT}/Cargo.toml" \
  --release \
  --locked \
  --target "$target" \
  --package picto-relay-server

readonly release_dir="${PROJECT_ROOT}/target/${target}/release"
readonly server="${release_dir}/picto-relay-server"

ssh -- "$host" "mkdir -p -- '$bin_dir'"
rsync --archive --compress --human-readable --itemize-changes -- \
  "$server" "$host:$bin_dir/"
ssh -- "$host" "chmod 0755 -- '$bin_dir/picto-relay-server'"

printf 'Deployed gateway to %s:%s\n' "$host" "$bin_dir"
