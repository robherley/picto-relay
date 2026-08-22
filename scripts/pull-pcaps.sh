#!/usr/bin/env bash
set -euo pipefail

readonly DEFAULT_HOST="rob@blueberry.lab.reb.gg"
readonly DEFAULT_SOURCE="/home/rob/picto-relay-captures"
readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly PROJECT_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
readonly DEFAULT_DESTINATION="${PROJECT_ROOT}/captures/blueberry"

host="${PICTO_RELAY_PI_HOST:-${DEFAULT_HOST}}"
source_dir="${PICTO_RELAY_PI_CAPTURE_DIR:-${DEFAULT_SOURCE}}"
destination="${PICTO_RELAY_CAPTURE_DIR:-${DEFAULT_DESTINATION}}"
dry_run=0
remove_source_files=0

usage() {
  cat <<EOF
Usage: $(basename "$0") [--dry-run] [--remove-source-files] [options]

Pull PictoChat packet captures from the Raspberry Pi.

Options:
  -n, --dry-run             Show what rsync would change without copying files.
      --remove-source-files Delete successfully transferred captures from the Pi.
      --host USER@HOST      SSH source (default: ${DEFAULT_HOST}).
      --source PATH         Remote capture directory (default: ${DEFAULT_SOURCE}).
      --destination PATH    Local capture directory (default: captures/blueberry).
  -h, --help                Show this help.

Environment:
  PICTO_RELAY_PI_HOST          Overrides the default SSH source.
  PICTO_RELAY_PI_CAPTURE_DIR   Overrides the remote capture directory.
  PICTO_RELAY_CAPTURE_DIR      Overrides the local capture directory.
EOF
}

while (($#)); do
  case "$1" in
    -n|--dry-run)
      dry_run=1
      shift
      ;;
    --remove-source-files)
      remove_source_files=1
      shift
      ;;
    --host)
      [[ $# -ge 2 ]] || { echo "error: --host requires a value" >&2; exit 2; }
      host="$2"
      shift 2
      ;;
    --source)
      [[ $# -ge 2 ]] || { echo "error: --source requires a value" >&2; exit 2; }
      source_dir="$2"
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
[[ "$source_dir" =~ ^/[a-zA-Z0-9._/-]+$ ]] || {
  echo "error: source must be an absolute path containing only safe path characters" >&2
  exit 2
}

mkdir -p -- "$destination"

rsync_options=(
  --archive
  --compress
  --human-readable
  --itemize-changes
  --include='*/'
  --include='*.pcap'
  --include='*.pcapng'
  --include='*.json'
  --exclude='*'
)

((dry_run == 0)) || rsync_options+=(--dry-run)
((remove_source_files == 0)) || rsync_options+=(--remove-source-files)

printf 'Pulling %s:%s/ to %s/\n' "$host" "$source_dir" "$destination"
rsync "${rsync_options[@]}" -- "$host:$source_dir/" "$destination/"
