# picto-relay-radio

`picto-relay-radio` defines the blocking raw-radio boundary used by Picto Relay and
provides both development and Linux implementations.

## Backends

- `ScriptedRadio` executes deterministic receive/transmit scripts for tests and
  hardware-free development.
- `LinuxMonitorRadio` receives radiotap packets and injects frames through an existing
  Linux monitor-mode interface. It can also write diagnostic PCAP captures.

All implementations satisfy the small `RadioBackend` trait: configure, receive by a
deadline, and transmit. Interface preparation and the custom AR9271 firmware workflow
are intentionally handled by higher-level tools.

The Linux backend uses blocking datalink operations and should have one owner, normally
the dedicated physical-session worker in `picto-relay-join`.

## Verify

```sh
cargo test -p picto-relay-radio --locked
```

See [active-join.md](../../docs/active-join.md) for physical interface requirements.
