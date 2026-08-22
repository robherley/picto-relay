#!/usr/bin/env bash
set -euo pipefail

readonly DEFAULT_HOST="rob@blueberry.lab.reb.gg"
readonly DEFAULT_DESTINATION="/home/rob/picto-relay"
readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly PROJECT_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"

host="${PICTO_RELAY_PI_HOST:-${DEFAULT_HOST}}"
destination="${PICTO_RELAY_PI_DIR:-${DEFAULT_DESTINATION}}"
dry_run=0
delete_stale=0

usage() {
  cat <<EOF
Usage: $(basename "$0") [--dry-run] [--delete] [--host USER@HOST] [--destination PATH]

Sync the Picto Relay source tree to a Raspberry Pi.

Options:
  -n, --dry-run           Show what rsync would change without copying files.
      --delete            Delete stale files at the destination after transfer.
      --host VALUE        SSH destination (default: ${DEFAULT_HOST}).
      --destination PATH  Remote project directory (default: ${DEFAULT_DESTINATION}).
  -h, --help              Show this help.

Environment:
  PICTO_RELAY_PI_HOST     Overrides the default SSH destination.
  PICTO_RELAY_PI_DIR      Overrides the default remote project directory.
EOF
}

while (($#)); do
  case "$1" in
    -n|--dry-run)
      dry_run=1
      shift
      ;;
    --delete)
      delete_stale=1
      shift
      ;;
    --host)
      [[ $# -ge 2 ]] || { echo "error: --host requires a value" >&2; exit 2; }
      host="$2"
      shift 2
      ;;
    --destination)
      [[ $# -ge 2 ]] || { echo "error: --destination requires a value" >&2; exit 2; }
      destination="$2"
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

command -v rsync >/dev/null 2>&1 || {
  echo "error: rsync is required" >&2
  exit 1
}
command -v ssh >/dev/null 2>&1 || {
  echo "error: ssh is required" >&2
  exit 1
}
[[ "$destination" =~ ^/[a-zA-Z0-9._/-]+$ ]] || {
  echo "error: destination must be an absolute path containing only safe path characters" >&2
  exit 2
}

rsync_options=(
  --archive
  --compress
  --human-readable
  --itemize-changes
  --exclude=/.git/
  --exclude=/target/
  --exclude=/captures/
  --exclude='*.pcap'
  --exclude='*.pcapng'
)

((dry_run == 0)) || rsync_options+=(--dry-run)
((delete_stale == 0)) || rsync_options+=(--delete-delay)

printf 'Syncing %s/ to %s:%s/\n' "$PROJECT_ROOT" "$host" "$destination"
ssh -- "$host" "mkdir -p -- '$destination'"
rsync "${rsync_options[@]}" -- "$PROJECT_ROOT/" "$host:$destination/"
