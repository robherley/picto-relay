#!/usr/bin/env bash
set -euo pipefail

readonly DEFAULT_HOST="rob@blueberry.lab.reb.gg"
readonly DEFAULT_LISTENER="/home/rob/.local/bin/picto-relay-listen"
readonly DEFAULT_OUTPUT="/home/rob/picto-relay-captures"

host="${PICTO_RELAY_PI_HOST:-${DEFAULT_HOST}}"
listener="${PICTO_RELAY_PI_LISTENER:-${DEFAULT_LISTENER}}"
output="${PICTO_RELAY_PI_CAPTURE_DIR:-${DEFAULT_OUTPUT}}"
interface="${PICTO_RELAY_INTERFACE:-wlan1}"
channel="${PICTO_RELAY_CHANNEL:-1}"

usage() {
  cat <<EOF
Usage: $(basename "$0") [options]

Guide a repeated-shape capture that validates PictoChat's tiled bitmap layout.
Enter always advances immediately; decoder events are informational only.

Options:
  --host USER@HOST   SSH destination (default: ${DEFAULT_HOST}).
  --interface NAME   Monitor interface (default: wlan1).
  --channel NUMBER   PictoChat channel (default: 1).
  --listener PATH    Remote listener binary (default: ${DEFAULT_LISTENER}).
  --output PATH      Remote capture directory (default: ${DEFAULT_OUTPUT}).
  -h, --help         Show this help.

Use only the console that joins Room A second for every drawing. Each shape is
sent twice unchanged so packet loss is less likely to erase the sample.
EOF
}

while (($#)); do
  case "$1" in
    --host)
      [[ $# -ge 2 ]] || { echo "error: --host requires a value" >&2; exit 2; }
      host="$2"
      shift 2
      ;;
    --interface)
      [[ $# -ge 2 ]] || { echo "error: --interface requires a value" >&2; exit 2; }
      interface="$2"
      shift 2
      ;;
    --channel)
      [[ $# -ge 2 ]] || { echo "error: --channel requires a value" >&2; exit 2; }
      channel="$2"
      shift 2
      ;;
    --listener)
      [[ $# -ge 2 ]] || { echo "error: --listener requires a value" >&2; exit 2; }
      listener="$2"
      shift 2
      ;;
    --output)
      [[ $# -ge 2 ]] || { echo "error: --output requires a value" >&2; exit 2; }
      output="$2"
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

command -v ssh >/dev/null 2>&1 || { echo "error: ssh is required" >&2; exit 1; }
[[ "$interface" =~ ^[a-zA-Z0-9._-]+$ ]] || { echo "error: invalid interface" >&2; exit 2; }
[[ "$channel" =~ ^(1|7|13)$ ]] || { echo "error: channel must be 1, 7, or 13" >&2; exit 2; }
[[ "$listener" =~ ^/[a-zA-Z0-9._/-]+$ ]] || { echo "error: invalid listener path" >&2; exit 2; }
[[ "$output" =~ ^/[a-zA-Z0-9._/-]+$ ]] || { echo "error: invalid output path" >&2; exit 2; }

printf 'Bitmap validation capture on %s (%s, channel %s)\n' "$host" "$interface" "$channel"
printf 'Checking SSH access and sudo credentials...\n'
ssh -tt -- "$host" "sudo -v"
ssh -n -- "$host" "sudo -n true" || {
  echo "error: sudo credentials are unavailable to the capture connection" >&2
  exit 1
}

readonly run_id="bitmap-validation-$$"
readonly remote_pid_file="/tmp/picto-relay-${run_id}.pid"
log_file="$(mktemp -t picto-relay-bitmap-validation.XXXXXX)"
listener_ssh_pid=""
tail_pid=""
stopped=0

stop_capture() {
  ((stopped == 0)) || return 0
  stopped=1
  if [[ -n "$listener_ssh_pid" ]] && kill -0 "$listener_ssh_pid" 2>/dev/null; then
    printf '\nStopping listener and flushing files...\n'
    ssh -n -- "$host" \
      "pid=\$(cat '$remote_pid_file' 2>/dev/null || true); case \"\$pid\" in (''|*[!0-9]*) exit 1;; esac; sudo -n kill -INT \"\$pid\"" \
      >/dev/null 2>&1 || true
    for _ in {1..40}; do
      kill -0 "$listener_ssh_pid" 2>/dev/null || break
      sleep 0.25
    done
    if kill -0 "$listener_ssh_pid" 2>/dev/null; then
      kill "$listener_ssh_pid" 2>/dev/null || true
    fi
    wait "$listener_ssh_pid" 2>/dev/null || true
  fi
}

cleanup() {
  stop_capture
  if [[ -n "$tail_pid" ]]; then
    kill "$tail_pid" 2>/dev/null || true
    wait "$tail_pid" 2>/dev/null || true
  fi
  rm -f -- "$log_file"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

printf -v listener_command \
  "exec stdbuf -oL -eL %q --no-color --prepare --interface %q --channel %q --scenario %q --tag %q --tag %q --tag %q --notes %q --output %q" \
  "$listener" \
  "$interface" \
  "$channel" \
  "joined client bitmap validation" \
  "sender=joined-second" \
  "drawing=bitmap-validation" \
  "sequence=top-left-dot,horizontal,vertical,bottom-right-thick" \
  "Each cleared shape sent twice unchanged; use only the console that joins second" \
  "$output"
printf -v wrapper_command "umask 022; echo \$\$ > %q; %s" "$remote_pid_file" "$listener_command"
printf -v remote_command "sudo -n sh -c %q" "$wrapper_command"

ssh -n -- "$host" "$remote_command" >"$log_file" 2>&1 &
listener_ssh_pid=$!
tail -n +1 -f "$log_file" &
tail_pid=$!

started_at=$SECONDS
while ! grep -Eq 'recording[[:space:]]' "$log_file"; do
  if ! kill -0 "$listener_ssh_pid" 2>/dev/null; then
    echo "error: listener exited during startup" >&2
    wait "$listener_ssh_pid" || true
    exit 1
  fi
  if ((SECONDS - started_at >= 20)); then
    echo "error: listener did not start within 20 seconds" >&2
    exit 1
  fi
  sleep 1
done

count_log() {
  grep -Ec "$1" "$log_file" 2>/dev/null || true
}

report_radio_delta() {
  local before_fragments="$1"
  local before_messages="$2"
  local after_fragments
  local after_messages
  sleep 1
  after_fragments="$(count_log '[[:space:]]FRAGMENT[[:space:]]')"
  after_messages="$(count_log '[[:space:]]MESSAGE[[:space:]]')"
  printf 'Radio check: +%s fragments, +%s completed messages. Continuing now.\n' \
    "$((after_fragments - before_fragments))" \
    "$((after_messages - before_messages))"
}

capture_shape() {
  local step="$1"
  local shape="$2"
  local instruction="$3"
  local before_fragments
  local before_messages

  printf '\n%s — %s\n' "$step" "$shape"
  printf '%s\n' "$instruction"
  printf 'Clear the input board first, then draw with the THIN black pen.\n'
  before_fragments="$(count_log '[[:space:]]FRAGMENT[[:space:]]')"
  before_messages="$(count_log '[[:space:]]MESSAGE[[:space:]]')"
  read -r -p "Send the shape once, then press Enter: "
  report_radio_delta "$before_fragments" "$before_messages"

  before_fragments="$(count_log '[[:space:]]FRAGMENT[[:space:]]')"
  before_messages="$(count_log '[[:space:]]MESSAGE[[:space:]]')"
  read -r -p "Without changing the board, send the identical shape again, then press Enter: "
  report_radio_delta "$before_fragments" "$before_messages"
}

printf '\nSTEP 1 OF 7 — Host Room A\n'
printf 'Put one console in Room A by itself. This console is the HOST; do not draw on it.\n'
read -r -p "Press Enter when the host is alone in Room A: "
sleep 1
printf 'Room evidence currently seen: %s ROOM events.\n' "$(count_log '[[:space:]]ROOM[[:space:]]')"

printf '\nSTEP 2 OF 7 — Join the sending console\n'
printf 'Join Room A with the second console. Use ONLY this second console below.\n'
read -r -p "Press Enter when the second console has joined: "
sleep 1
printf 'Join evidence currently seen: %s JOIN events. Continuing now.\n' "$(count_log '[[:space:]]JOIN[[:space:]]')"

capture_shape \
  "STEP 3 OF 7" \
  "Top-left dot" \
  "Make a small dot just inside the TOP-LEFT corner of the drawing area."

capture_shape \
  "STEP 4 OF 7" \
  "Top horizontal stroke" \
  "Draw a short LEFT-TO-RIGHT line near the top-left, about one stylus-width below the dot."

capture_shape \
  "STEP 5 OF 7" \
  "Middle vertical stroke" \
  "Draw a short TOP-TO-BOTTOM line near the left side and around the vertical middle."

printf '\nSTEP 6 OF 7 — Bottom-right thick dot\n'
printf 'Clear the board, select the THICK black pen, and make one dot just inside BOTTOM-RIGHT.\n'
before_fragments="$(count_log '[[:space:]]FRAGMENT[[:space:]]')"
before_messages="$(count_log '[[:space:]]MESSAGE[[:space:]]')"
read -r -p "Send the thick dot once, then press Enter: "
report_radio_delta "$before_fragments" "$before_messages"
before_fragments="$(count_log '[[:space:]]FRAGMENT[[:space:]]')"
before_messages="$(count_log '[[:space:]]MESSAGE[[:space:]]')"
read -r -p "Without changing the board, send the identical thick dot again, then press Enter: "
report_radio_delta "$before_fragments" "$before_messages"

printf '\nSTEP 7 OF 7 — Leave\n'
printf 'Leave Room A using the second console. Keep the host in Room A.\n'
read -r -p "Press Enter after the second console has left: "
sleep 1
printf 'Leave evidence currently seen: %s LEAVE events.\n' "$(count_log '[[:space:]]LEAVE[[:space:]]')"

stop_capture
printf '\nCapture complete. Pull it with:\n  ./scripts/pull-pcaps.sh\n'
