# picto-relay-link

Host side of the USB wire protocol between the server and the ESP32 firmware:
command IDs, packet framing, incremental decoding and packed drawing bodies. No
dependencies.

The firmware implements the same protocol in C (`firmware/esp32/main/link.c`).
The two are held together by one fixture file that both test suites check, so
change them in the same commit:

```sh
cargo test -p picto-relay-link --locked
just firmware-test
```

After an intentional format change, regenerate the fixture with
`PICTO_WRITE_FIXTURES=1 cargo test -p picto-relay-link --test firmware_fixture`.

See the [USB contract](../../docs/esp32-usb.md) for commands and payload layouts.
