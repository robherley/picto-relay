# ESP32 PictoChat firmware

C firmware for the ESP-WROOM-32. It joins a DS-hosted PictoChat room, exchanges
profiles and drawings, and answers the DS's polls within their timing window.
The computer runs the web server and talks to it over USB serial. Hosting a room
is not supported.

## Build and flash

Requires ESP-IDF **v5.5.2** exactly, with its ESP32 toolchain. The build refuses
any other version because it hooks private parts of the Wi-Fi libraries.

From the repository root:

```sh
just espressif          # Once: ESP-IDF and toolchain into .esp/ (about 2 GB)
just firmware-build
just firmware-upload
```

`espressif` clones ESP-IDF and downloads its toolchain into the gitignored
`.esp/` directory at the repository root. Nothing is installed system-wide, and
the build recipes activate it themselves, so there is no `export.sh` to source.
It needs `git`, Python 3 and CMake's usual prerequisites. Delete `.esp/` to
uninstall. If `.esp/` is absent, the recipes fall back to an ESP-IDF v5.5.2
environment you have already sourced.

`firmware-upload` builds first. Stop the relay server before flashing so the
serial port is free. The port defaults to `/dev/cu.usbserial-0001`:

```sh
PICTO_SERIAL_PORT=/dev/ttyUSB0 just firmware-upload
```

There are no build options to match against the server. The USB protocol is
defined in `crates/picto-relay-link`; `main/link_protocol.h` is generated from it
with `just protocol-header`. The server checks the firmware's protocol version
when it connects and names the firmware build if they differ.

## Test

The protocol code has no ESP-IDF dependencies and is tested on the host with any
C compiler:

```sh
just firmware-test
```

## Layout

| File | Role |
| --- | --- |
| `main/client.{h,c}` | Protocol state machine: join, profile exchange, drawing fragmentation, poll replies, delivery tracking. Portable C |
| `main/link.{h,c}` | USB framing. Portable C |
| `main/link_protocol.h` | Protocol constants, generated from the Rust link crate. Do not edit |
| `main/bridge.{h,c}` | USB commands, events and channel search; serializes access to the client between tasks. Portable C |
| `main/platform.c` | ESP-IDF setup, Wi-Fi callbacks, UART |
| `main/radio_compat.c` | Hooks that let the stock Wi-Fi driver send our frames and reply fast enough |
| `main/radio_prepared.c` | Stages the next drawing reply before its poll arrives |
| `main/diag.h` | Optional timing counters |
| `prepare_radio.py` | Build step: makes private copies of SDK objects with the hook points exposed, after checking library hashes |
| `verify_radio_layout.py` | Build step: fails if timing-critical code is not in instruction RAM |
| `test/` | Host tests and fixtures |

`sdkconfig.defaults` holds the reproducible ESP-IDF settings; `sdkconfig` and
`build/` are generated.

## Capturing radio traffic

The firmware can report every PictoChat frame it hears or sends. Run the server
with a capture file and open the result in Wireshark:

```sh
just server-capture            # Writes capture.pcap in the repository root
```

See [Capturing radio traffic](../../docs/esp32-usb.md#capturing-radio-traffic).

## Timing diagnostics

To investigate reply latency, build with the counters enabled and run the server
with debug logging:

```sh
PICTO_DIAGNOSTICS=ON just firmware-upload
RUST_LOG=picto_relay_server=debug just server
```

See the [USB protocol](../../docs/esp32-usb.md) and the
[latency investigation log](../../docs/history/esp32-latency.md).
