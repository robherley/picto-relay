export PICTO_SERIAL_PORT := env("PICTO_SERIAL_PORT", "/dev/cu.usbserial-0001")
export PICTO_RELAY_BIND := env("PICTO_RELAY_BIND", "127.0.0.1:3001")
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
server: web-build
    cargo run --locked -p picto-relay-server -- --backend esp32 --serial "$PICTO_SERIAL_PORT"

# Run the server and record every radio frame the ESP32 hears to capture.pcap.
server-capture file="capture.pcap": web-build
    cargo run --locked -p picto-relay-server -- --backend esp32 --serial "$PICTO_SERIAL_PORT" --capture "{{file}}"

# Run the web/REST server without hardware.
server-memory: web-build
    cargo run --locked -p picto-relay-server -- --backend memory

# Build the web client into services/web/dist, which the server embeds when it compiles.
web-build: _web-deps
    npm --prefix services/web run build

# Run the web client with hot reload; needs a server running (`just server-memory`).
web-dev: _web-deps
    npm --prefix services/web run dev

# Run the demo page, which drives the ESP32 itself with no server (Chrome or Edge); stop the server first.
demo: _web-deps
    npm --prefix services/web run dev:serial

# Build the demo page into services/web/dist-demo: static files, with the last firmware build to flash.
demo-build: _web-deps
    npm --prefix services/web run build:serial

# Deploy the demo page to the Vercel project this folder is linked to (`vercel link`, once).
demo-deploy: demo-build
    rm -rf .vercel/output
    mkdir -p .vercel/output/static
    cp -R services/web/dist-demo/. .vercel/output/static/
    cp services/web/vercel-output.json .vercel/output/config.json
    vercel deploy --prebuilt --prod

# Install the web client's dependencies when package-lock.json has changed.
_web-deps:
    @[ services/web/node_modules/.package-lock.json -nt services/web/package-lock.json ] || npm --prefix services/web ci --no-audit --no-fund

# Host a model in a PictoChat room: just bot --room A --model <id> [--name <nickname>]
[positional-arguments]
bot *flags: _bot-deps
    @# With no API key, use the linked Vercel project's token, which lasts 12 hours: fetch a fresh one.
    @[ -n "${AI_GATEWAY_API_KEY:-}" ] || [ ! -e .vercel/project.json ] || vercel env pull services/bot/.env.local --yes >/dev/null 2>&1 || echo "Could not fetch a fresh Vercel token; try: vercel env pull services/bot/.env.local" >&2
    @npm --prefix services/bot start --silent -- "$@"

# Install the bot's dependencies when package-lock.json has changed.
_bot-deps:
    @[ services/bot/node_modules/.package-lock.json -nt services/bot/package-lock.json ] || npm --prefix services/bot ci --no-audit --no-fund

# Run every test that needs no hardware.
test: firmware-test web-build _bot-deps
    npm --prefix services/web test
    npm --prefix services/bot test
    cargo test --workspace --locked

# Regenerate the firmware's protocol header and shared test fixtures from the Rust crates.
protocol-header:
    PICTO_WRITE_FIXTURES=1 cargo test --locked -p picto-relay-link --test firmware_fixture
    PICTO_WRITE_FIXTURES=1 cargo test --locked -p picto-relay-server firmware_drawing_fixture
    PICTO_WRITE_FIXTURES=1 cargo test --locked -p picto-relay-wasm web_fixture

# Regenerate docs/openapi.json from the server's handlers and types.
openapi:
    PICTO_WRITE_FIXTURES=1 cargo test --locked -p picto-relay-server checked_in_openapi_document

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
