#!/usr/bin/env bash
set -euo pipefail

readonly UPSTREAM_URL="https://github.com/qca/open-ath9k-htc-firmware.git"
readonly UPSTREAM_COMMIT="e888634"
readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly PROJECT_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
readonly PATCH="${PROJECT_ROOT}/firmware/patches/0001-picto-relay-slot-reply.patch"

source_root="${PICTO_RELAY_FIRMWARE_SOURCE:-/home/rob/picto-relay-firmware/open-ath9k-htc-firmware}"
output="${PICTO_RELAY_FIRMWARE_OUTPUT:-/home/rob/picto-relay-firmware/htc_9271-picto-relay.fw}"

command -v git >/dev/null 2>&1 || { echo "error: git is required" >&2; exit 1; }
command -v cmake >/dev/null 2>&1 || { echo "error: cmake is required" >&2; exit 1; }
command -v m4 >/dev/null 2>&1 || { echo "error: m4 is required" >&2; exit 1; }
export CMAKE_POLICY_VERSION_MINIMUM="${CMAKE_POLICY_VERSION_MINIMUM:-3.5}"

if [[ ! -d "${source_root}/target_firmware" ]]; then
  mkdir -p -- "${source_root%/*}"
  git clone --no-checkout -- "$UPSTREAM_URL" "$source_root"
  git -C "$source_root" checkout --detach "$UPSTREAM_COMMIT"
fi

apply_current_patch() {
  git -C "$source_root" apply --recount --check "$PATCH"
  git -C "$source_root" apply --recount "$PATCH"
}

if grep -q 'proactiveCreditReport' \
  "$source_root/target_firmware/magpie_fw_dev/target/htc/htc.c"; then
  printf 'Removing the superseded proactive HTC credit experiment.\n'
  git -C "$source_root" restore --source "$UPSTREAM_COMMIT" -- \
    target_firmware/magpie_fw_dev/target/htc/htc.c
fi

if git -C "$source_root" apply --recount --reverse --check "$PATCH" >/dev/null 2>&1; then
  printf 'Firmware source already contains the exact Picto Relay patch.\n'
elif git -C "$source_root" diff --quiet -- \
  target_firmware/wlan/if_ath.c target_firmware/wlan/if_owl.c; then
  apply_current_patch
elif [[ "$(git -C "$source_root" diff --name-only -- \
    target_firmware/wlan/if_ath.c target_firmware/wlan/if_owl.c)" == \
    $'target_firmware/wlan/if_ath.c\ntarget_firmware/wlan/if_owl.c' ]] \
  && grep -q 'ath_tgt_picto_rx(sc,' \
    "$source_root/target_firmware/wlan/if_ath.c" \
  && grep -q 'ath_tgt_picto_arm_frame' \
    "$source_root/target_firmware/wlan/if_owl.c"; then
  printf 'Replacing the previous Picto Relay patch in the two firmware source files.\n'
  git -C "$source_root" restore --source "$UPSTREAM_COMMIT" -- \
    target_firmware/wlan/if_ath.c target_firmware/wlan/if_owl.c
  apply_current_patch
else
  echo "error: firmware source contains a different local patch" >&2
  echo "restore only target_firmware/wlan/if_ath.c and if_owl.c at ${UPSTREAM_COMMIT}, then rerun" >&2
  exit 1
fi

make -C "$source_root" toolchain
make -C "${source_root}/target_firmware"
mkdir -p -- "${output%/*}"
install -m 0644 -- "${source_root}/target_firmware/htc_9271.fw" "$output"

printf 'Built custom AR9271 firmware: %s\n' "$output"
printf 'SHA-256: '
sha256sum "$output" | cut -d' ' -f1
