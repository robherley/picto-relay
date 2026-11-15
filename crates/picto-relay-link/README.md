# picto-relay-link

The USB protocol between the server and the ESP32 firmware, defined once: packet
kinds, payload layouts, sizes and timeouts, plus the host side of the framing.
No dependencies.

The firmware's C header, `firmware/esp32/main/link_protocol.h`, is generated
from this crate's constants. After changing them:

```sh
just protocol-header    # Rewrite the header and the shared test fixtures
just test               # Rust and C sides against the same fixtures
```

`cargo test` fails if the checked-in header no longer matches the constants.
The framing is implemented here and in `firmware/esp32/main/link.c`; both are
tested against one fixture file.

See the [protocol description](../../docs/esp32-usb.md).
