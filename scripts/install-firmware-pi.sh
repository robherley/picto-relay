#!/usr/bin/env bash
set -euo pipefail

readonly SOURCE="${PICTO_RELAY_FIRMWARE_SOURCE:-/home/rob/picto-relay-firmware/htc_9271-picto-relay.fw}"
readonly DESTINATION="/lib/firmware/htc_9271.fw"
readonly BACKUP_DIR="/home/rob/picto-relay-firmware/backups"

if ((EUID != 0)); then
  exec sudo -- "$0" "$@"
fi

[[ -f "$SOURCE" ]] || {
  echo "error: staged firmware does not exist: $SOURCE" >&2
  exit 1
}
[[ -f "$DESTINATION" ]] || {
  echo "error: installed AR9271 firmware does not exist: $DESTINATION" >&2
  exit 1
}
command -v sha256sum >/dev/null 2>&1 || { echo "error: sha256sum is required" >&2; exit 1; }
command -v modprobe >/dev/null 2>&1 || { echo "error: modprobe is required" >&2; exit 1; }

source_hash="$(sha256sum "$SOURCE" | cut -d' ' -f1)"
installed_hash="$(sha256sum "$DESTINATION" | cut -d' ' -f1)"
mkdir -p -- "$BACKUP_DIR"

if [[ "$source_hash" != "$installed_hash" ]]; then
  backup="${BACKUP_DIR}/htc_9271-${installed_hash}.fw"
  if [[ ! -f "$backup" ]]; then
    install -m 0644 -- "$DESTINATION" "$backup"
    printf 'Backed up previous firmware to %s\n' "$backup"
  fi
  install -m 0644 -- "$SOURCE" "$DESTINATION"
  sync "$DESTINATION"
  printf 'Installed Picto Relay firmware %s\n' "$source_hash"
else
  printf 'Picto Relay firmware is already installed (%s).\n' "$source_hash"
fi

modprobe -r ath9k_htc
modprobe ath9k_htc
command -v udevadm >/dev/null 2>&1 && udevadm settle || true

loaded_hash="$(sha256sum "$DESTINATION" | cut -d' ' -f1)"
[[ "$loaded_hash" == "$source_hash" ]] || {
  echo "error: installed firmware hash changed during reload" >&2
  exit 1
}
printf 'Reloaded ath9k_htc with firmware %s\n' "$loaded_hash"
