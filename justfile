export PICTO_SERIAL_PORT := env("PICTO_SERIAL_PORT", "/dev/cu.usbserial-0001")
export PICTO_RELAY_BIND := env("PICTO_RELAY_BIND", "127.0.0.1:3001")
export PICTO_CHANNEL := env("PICTO_CHANNEL", "1")
export PICTO_FLASH_BAUD := env("PICTO_FLASH_BAUD", "460800")
# Set to ON to build firmware that reports radio timing counters.
export PICTO_DIAGNOSTICS := env("PICTO_DIAGNOSTICS", "OFF")

# ESP-IDF lives inside the project (gitignored) so nothing is installed system-wide.
# The firmware hooks private Wi-Fi library internals, so the version is exact.
idf_version := "v5.5.2"
export PICTO_ESP_DIR := justfile_directory() / ".esp"

# Shell snippet: use the project-local ESP-IDF, else one already sourced by the caller.
idf_activate := '''
if [ -f "$PICTO_ESP_DIR/esp-idf/export.sh" ]; then
    export IDF_TOOLS_PATH="$PICTO_ESP_DIR/tools"
    . "$PICTO_ESP_DIR/esp-idf/export.sh" >/dev/null || { echo "ESP-IDF activation failed; rerun: just espressif" >&2; exit 1; }
elif ! command -v idf.py >/dev/null; then
    echo "ESP-IDF not found. Run: just espressif" >&2
    exit 1
fi
'''

# List available commands.
default:
    @just --list

# Run the web/REST server with the connected ESP32.
server:
    cargo run --locked -p picto-relay-server -- --backend esp32 --serial "$PICTO_SERIAL_PORT" --channel "$PICTO_CHANNEL"

# Run the web/REST server without hardware.
server-memory:
    cargo run --locked -p picto-relay-server -- --backend memory

# Run every test that needs no hardware.
test: firmware-test
    cargo test --workspace --locked
    node --test crates/picto-relay-server/tests/drawing.test.cjs

# Build and run the firmware protocol tests on this computer (no ESP-IDF needed).
firmware-test:
    #!/usr/bin/env sh
    set -eu
    cd firmware/esp32
    mkdir -p build-host
    ${CC:-cc} -std=gnu17 -Wall -Wextra -Werror -O1 -g -fsanitize=address,undefined -fno-sanitize-recover=undefined \
        main/link.c main/client.c main/bridge.c test/test.c -o build-host/test
    ./build-host/test

# Download ESP-IDF and the ESP32 toolchain into .esp/ (about 2 GB, one time).
espressif:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p "$PICTO_ESP_DIR"
    if [ ! -f "$PICTO_ESP_DIR/esp-idf/export.sh" ]; then
        git clone --branch {{idf_version}} --depth 1 --recursive --shallow-submodules --jobs 8 \
            https://github.com/espressif/esp-idf.git "$PICTO_ESP_DIR/esp-idf"
    fi
    IDF_TOOLS_PATH="$PICTO_ESP_DIR/tools" "$PICTO_ESP_DIR/esp-idf/install.sh" esp32

# Build the ESP32 firmware (run `just espressif` once first).
firmware-build:
    #!/usr/bin/env bash
    {{idf_activate}}
    set -euo pipefail
    cd firmware/esp32
    idf.py -DIDF_TARGET=esp32 -DPICTO_DIAGNOSTICS="$PICTO_DIAGNOSTICS" build

# Build and flash the connected ESP32; stop the server first.
firmware-upload: firmware-build
    #!/usr/bin/env bash
    {{idf_activate}}
    set -euo pipefail
    cd firmware/esp32
    idf.py -p "$PICTO_SERIAL_PORT" -b "$PICTO_FLASH_BAUD" flash

# Send measured test drawings to a DS; stop the server first.
benchmark:
    cargo test --locked -p picto-relay-server --test hardware_drawing -- --ignored --nocapture
