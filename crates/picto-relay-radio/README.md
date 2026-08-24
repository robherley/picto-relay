# picto-relay-radio

`picto-relay-radio` defines the blocking raw-radio boundary used by Picto Relay and
provides its Linux implementation.

## Backends

- `LinuxMonitorRadio` receives radiotap packets and injects frames through an existing
  Linux monitor-mode interface. It can also write diagnostic PCAP captures.

The implementation satisfies the small `RadioBackend` trait: configure, receive by a
deadline, and transmit. Tests that exercise radio-facing join logic use a private,
minimal test double rather than a public simulated backend. Interface preparation and
the custom AR9271 firmware workflow are intentionally handled by higher-level tools.

The Linux backend uses blocking datalink operations and should have one owner, normally
the dedicated physical-session worker in `picto-relay-join`.

## Verify

```sh
cargo test -p picto-relay-radio --locked
```

See [active-join.md](../../docs/active-join.md) for physical interface requirements.
