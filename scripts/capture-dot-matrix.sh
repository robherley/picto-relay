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

Run a guided client dot-matrix capture using the Raspberry Pi listener.
The script pauses for each console action and checks the listener output.

Options:
  --host USER@HOST   SSH destination (default: ${DEFAULT_HOST}).
  --interface NAME   Monitor interface (default: wlan1).
  --channel NUMBER   PictoChat channel (default: 1).
  --listener PATH    Remote listener binary (default: ${DEFAULT_LISTENER}).
  --output PATH      Remote capture directory (default: ${DEFAULT_OUTPUT}).
  -h, --help         Show this help.

Capture sequence:
  1. Start with only the host in Room A.
  2. Join from the DS/Lite client and wait for JOIN or a two-user ROOM event.
  3. Send a small dot near the upper-left twice.
  4. Send a dot near the upper-right.
  5. Send a dot near the lower-left.
  6. Leave from the client. The script then stops and flushes the capture.
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

command -v ssh >/dev/null 2>&1 || {
  echo "error: ssh is required" >&2
  exit 1
}
[[ "$interface" =~ ^[a-zA-Z0-9._-]+$ ]] || {
  echo "error: interface contains unsupported characters" >&2
  exit 2
}
[[ "$channel" =~ ^(1|7|13)$ ]] || {
  echo "error: PictoChat channel must be 1, 7, or 13" >&2
  exit 2
}
[[ "$listener" =~ ^/[a-zA-Z0-9._/-]+$ ]] || {
  echo "error: listener must be an absolute safe path" >&2
  exit 2
}
[[ "$output" =~ ^/[a-zA-Z0-9._/-]+$ ]] || {
  echo "error: output must be an absolute safe path" >&2
  exit 2
}

printf 'Starting client dot-matrix capture on %s (%s, channel %s)\n' \
  "$host" "$interface" "$channel"
printf 'The wizard pauses before every action. You can abort safely with Ctrl-C.\n\n'

printf 'Checking SSH access and sudo credentials on the Pi...\n'
ssh -tt -- "$host" "sudo -v"
ssh -n -- "$host" "sudo -n true" || {
  echo "error: sudo credentials were not available to the capture connection" >&2
  echo "Configure passwordless sudo for the listener workflow, then try again." >&2
  exit 1
}

readonly run_id="dot-matrix-$$"
readonly remote_pid_file="/tmp/picto-relay-${run_id}.pid"
log_file="$(mktemp -t picto-relay-dot-matrix.XXXXXX)"
listener_ssh_pid=""
tail_pid=""
stopped=0

stop_capture() {
  ((stopped == 0)) || return 0
  stopped=1
  if [[ -n "$listener_ssh_pid" ]] && kill -0 "$listener_ssh_pid" 2>/dev/null; then
    printf '\nStopping listener and flushing the PCAP...\n'
    ssh -n -- "$host" \
      "pid=\$(cat '$remote_pid_file' 2>/dev/null || true); case \"\$pid\" in (''|*[!0-9]*) exit 1;; esac; sudo -n kill -INT \"\$pid\"" \
      >/dev/null 2>&1 || true
    for _ in {1..40}; do
      kill -0 "$listener_ssh_pid" 2>/dev/null || break
      sleep 0.25
    done
    if kill -0 "$listener_ssh_pid" 2>/dev/null; then
      echo "warning: listener did not exit after ten seconds" >&2
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
  "client dot matrix" \
  "sender=client" \
  "drawing=dot-matrix" \
  "sequence=upper-left-repeat,upper-right,lower-left" \
  "Send four messages: same upper-left dot twice, then upper-right, then lower-left" \
  "$output"

printf -v wrapper_command \
  "umask 022; echo \$\$ > %q; %s" \
  "$remote_pid_file" \
  "$listener_command"
printf -v remote_command "sudo -n sh -c %q" "$wrapper_command"

ssh -n -- "$host" "$remote_command" >"$log_file" 2>&1 &
listener_ssh_pid=$!
tail -n +1 -f "$log_file" &
tail_pid=$!

wait_for_log() {
  local pattern="$1"
  local description="$2"
  local timeout="${3:-30}"
  local started_at=$SECONDS

  while ((SECONDS - started_at < timeout)); do
    if grep -Eq "$pattern" "$log_file"; then
      printf '\nConfirmed: %s.\n' "$description"
      return 0
    fi
    if ! kill -0 "$listener_ssh_pid" 2>/dev/null; then
      printf '\nerror: listener exited before %s\n' "$description" >&2
      wait "$listener_ssh_pid" || true
      return 1
    fi
    sleep 1
  done

  printf '\nWarning: did not observe %s within %s seconds.\n' "$description" "$timeout" >&2
  read -r -p "Press Enter to continue anyway, or Ctrl-C to abort: "
}

message_count() {
  grep -Ec '[[:space:]]MESSAGE[[:space:]]' "$log_file" 2>/dev/null || true
}

wait_for_new_message() {
  local previous_count="$1"
  local timeout=30
  local started_at=$SECONDS
  local current_count

  while ((SECONDS - started_at < timeout)); do
    current_count="$(message_count)"
    if ((current_count > previous_count)); then
      printf '\nConfirmed: message %s was reassembled.\n' "$current_count"
      return 0
    fi
    if ! kill -0 "$listener_ssh_pid" 2>/dev/null; then
      printf '\nerror: listener exited while waiting for a message\n' >&2
      wait "$listener_ssh_pid" || true
      return 1
    fi
    sleep 1
  done

  printf '\nWarning: no completed MESSAGE appeared within %s seconds.\n' "$timeout" >&2
  read -r -p "Press Enter to continue anyway, or Ctrl-C to abort: "
}

wait_for_log 'recording[[:space:]]' 'the listener is recording' 20

printf '\nSTEP 1 OF 6 — Prepare the host\n'
printf 'On the host DS, open PictoChat and enter Room A. Keep the client outside the room.\n'
read -r -p "Press Enter once the host is alone in Room A: "
wait_for_log 'ROOM.*room A.*1 user' 'Room A has one user' 30

printf '\nSTEP 2 OF 6 — Join the client\n'
printf 'On the DS/Lite client, enter Room A. Do not draw yet.\n'
read -r -p "Press Enter once the client has joined: "
wait_for_log 'JOIN|ROOM.*room A.*2 user' 'the client joined Room A' 30

printf '\nSTEP 3 OF 6 — First control dot\n'
printf 'With the thin black pen, make a small dot near the UPPER-LEFT and send it.\n'
before="$(message_count)"
read -r -p "Press Enter immediately after sending the first dot: "
wait_for_new_message "$before"

printf '\nSTEP 4 OF 6 — Repeat the control dot\n'
printf 'Clear the input board, make the same upper-left dot again, and send it.\n'
before="$(message_count)"
read -r -p "Press Enter immediately after sending the repeated dot: "
wait_for_new_message "$before"

printf '\nSTEP 5 OF 6 — Spatial comparison dots\n'
printf 'Clear the board, make a dot near the UPPER-RIGHT, and send it.\n'
before="$(message_count)"
read -r -p "Press Enter immediately after sending the upper-right dot: "
wait_for_new_message "$before"

printf '\nClear the board, make a dot near the LOWER-LEFT, and send it.\n'
before="$(message_count)"
read -r -p "Press Enter immediately after sending the lower-left dot: "
wait_for_new_message "$before"

printf '\nSTEP 6 OF 6 — Leave cleanly\n'
printf 'On the client DS/Lite, leave Room A. Keep the host in the room.\n'
read -r -p "Press Enter once the client has left: "
wait_for_log 'LEAVE' 'the client leave event' 20

stop_capture
printf '\nCapture complete. Pull it with:\n  ./scripts/pull-pcaps.sh\n'
