# Picto Relay implementation plan

## Goal

Build Picto Relay as a Raspberry Pi service that exposes a real Nintendo DS
PictoChat participant through a local JSON API. The Pi uses an AR9271 USB radio
and custom firmware for timing-critical slot replies.

All application, protocol, radio, server, and listener code is implemented in Rust.
The browser client is plain HTML, CSS, and JavaScript embedded in the server binary.
The small AR9271 firmware patch is the only device-side code.

## Current architecture

```text
Browser UI / JSON client
            |
            v
     Tokio session broker
       |              |
       | local        | --backend ar9271
       v              v
In-memory API      dedicated blocking
test double        physical-session worker
                         |
                         v
                RadioBackend + AR9271
                         |
                         v
                   physical DS host
```

The in-memory backend validates only the JSON session contract. It deliberately
does not construct Nintendo packets, synthesize acknowledgements, or imply physical
delivery. The physical worker in `picto-relay-join` is the sole implementation of
room discovery, authentication, association, profile initialization, firmware-window
scheduling, drawing delivery, and receive reassembly.

The physical radio has one owner on a dedicated OS thread. The Tokio broker exchanges
bounded commands and events with that worker, keeping blocking datalink operations out
of async tasks. Current firmware represents one virtual station, so the AR9271 backend
accepts one API session. The in-memory backend supports multiple local API sessions.

## Workspace boundaries

```text
crates/
|-- picto-relay-codec/         Packet parsing, encoding, and reassembly
|-- picto-relay-radio/         Raw-radio trait and Linux backend
|-- picto-relay-join/          Physical station worker and diagnostic CLI
|-- picto-relay-listener/      Tagged capture, replay, and protocol inspection
`-- picto-relay-server/        Room broker, JSON API, and embedded web client
```

The former deterministic engine was removed. Its room model was never fed by the
default broker, its wakeups were dropped instead of scheduled, and its acknowledgement
model was contradicted by physical captures. Retaining it beside the proven hardware
worker made the simulator look authoritative when it was not.

## Phase 0: executable skeleton — complete, then retired

The first hardware-free skeleton established crate boundaries, a radio trait, an
ordered test double, and a reducer-style protocol experiment. It was useful before
hardware arrived, but its engine and application runner were removed once captures
showed that they did not represent the real transport. Radio-facing tests now keep a
small private test double beside the physical worker they exercise.

## Phase 1: local product loop — complete

Delivered:

- versioned JSON room, session, message, status, and event endpoints;
- bounded broker and per-session event queues;
- an explicit in-memory API backend for hardware-free client development;
- a framework-free browser UI for room rosters, joining, drawing, text, and received
  messages;
- WebSocket-pushed room snapshots and session events, with REST polling retained for
  API compatibility;
- no frontend build-tool dependency.

## Phase 2: capture-backed protocol model — complete

Delivered:

- lossless IEEE 802.11 parsing with explicit FCS handling;
- typed Nintendo beacons, authentication, association, and multiplayer headers;
- capture-backed PictoChat envelope and fragment parsing;
- offset-aware, out-of-order message reassembly;
- native one- through five-band drawing encoding and decoding;
- golden two-DS capture fixtures and fixed malformed-input parser tests;
- consolidated packet-shape, protocol, and evidence documents.

Unknown bytes remain opaque. No capture-derived field is promoted to semantic protocol
state without physical evidence.

## Phase 3: single physical station — complete

Verified against real DS hardware:

1. room discovery;
2. open-system authentication;
3. Nintendo binary-session association;
4. timing-correct slot presence;
5. both nickname/profile passes;
6. receiving and reassembling host-broadcast drawings;
7. sending 256x16 through 256x80 drawings;
8. repeated sends through a sixteen-slot circular firmware window;
9. graceful disarm and room departure.

The custom AR9271 firmware handles only timing-critical selection and transmission.
Rust owns message construction, window refill, delivery correlation, and the JSON API.

## Phase 4: hardening and protocol completion — next

- Run longer send/receive soak tests with the stable power supply.
- Replace remaining capture-derived constants as their meanings are proven.
- Decode remote profile/name records so incoming events show real DS nicknames.
- Improve disconnect and radio-recovery behavior after USB or driver resets.
- Expand capture-backed negative tests for malformed and interrupted transfers.

Exit criterion: one physical station can run for an extended interactive session,
recover cleanly from expected radio failures, and report every message outcome without
manual PCAP interpretation.

## Phase 5: multiplexing and hosting — future

1. determine whether one AR9271 firmware instance can schedule multiple virtual station
   slots without violating Nintendo timing;
2. represent multiple API sessions as distinct physical participants;
3. implement DS-compatible room hosting only after client-mode multiplexing is stable.

These steps require new hardware evidence and are intentionally separate from the
working single-station backend.
