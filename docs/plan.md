# Picto Relay implementation plan

## Goal

Build Picto Relay as a Raspberry Pi service that lets local API clients act as
virtual Nintendo DS PictoChat participants. The Pi uses an AR9271 USB radio to
exchange frames with physical DS hardware.

Protocol state, scheduling, radio interaction, and the reference terminal client
are implemented in Rust.

## Hardware assumptions

For the initial design, assume the AR9271 and its Linux driver provide every
primitive the bridge needs:

- monitor-mode reception on Nintendo DS-compatible 2.4 GHz channels;
- raw 802.11 frame injection;
- simultaneous receive and transmit;
- arbitrary source MAC addresses for virtual participants.

These assumptions are validation targets once hardware arrives, not runtime
capabilities the application must negotiate. The initial radio interface will
therefore remain deliberately small.

## Architecture

```text
JSON API client / terminal REPL
        | HTTP
        v
Async session broker (Tokio)
        | bounded commands and events
        v
Deterministic protocol engine
        | raw receive/transmit effects
        v
RadioBackend
        |-- ScriptedRadio (development)
        `-- AR9271 Linux backend (later)
```

The protocol engine is synchronous and independent of Tokio, Linux, and wall
clock time. It consumes timestamped inputs and produces explicit effects. This
makes protocol scripts deterministic and lets tests advance time without
sleeping.

The eventual physical radio will have a single owner on a dedicated thread.
The Tokio-facing broker will communicate with it through bounded channels, so
blocking driver work cannot stall an async worker and client traffic cannot
create an unbounded radio queue.

## Initial workspace

```text
crates/
|-- picto-relay-codec/   Packet parsing and serialization
|-- picto-relay-engine/  Sessions, state machines, timers, and effects
|-- picto-relay-radio/   RadioBackend and ScriptedRadio
|-- picto-relay-server/  Application runner and JSON HTTP service
`-- picto-relay-repl/    Interactive terminal client
```

Only `ScriptedRadio` is required before hardware arrives. PCAP, null, loopback,
capability negotiation, and a separate Linux radio crate are intentionally
deferred until they solve an observed problem.

## Phase 0: executable architecture skeleton — complete

Implement:

- the Cargo workspace and crate boundaries;
- virtual client and station identities;
- a minimal provisional frame codec;
- a reducer-style engine with client, radio, and timer inputs;
- explicit transmit, notification, and wakeup effects;
- a strict `ScriptedRadio` that injects receptions and checks transmissions;
- an application runner that executes engine effects;
- one end-to-end scripted integration test.

The integration test must:

1. create two independent virtual-DS sessions;
2. submit a message from one session;
3. observe the expected raw transmission;
4. inject a scripted raw reception;
5. observe the corresponding client event;
6. advance time and exercise retransmission or timeout behavior;
7. disconnect one session without affecting the other;
8. finish with every scripted radio step consumed.

The provisional codec is an internal development envelope, not a claim about
the PictoChat wire format. It exists only to test the architecture and will be
replaced incrementally by verified Nintendo/PictoChat structures.

Exit criterion: commands, raw transmissions, raw receptions, client events,
and timers traverse the complete backend deterministically.

## Phase 1: headless product loop — complete

Implement:

- a versioned JSON HTTP command/event schema;
- API session creation and teardown;
- multiple simultaneous virtual clients;
- bounded queues and explicit connection state;
- a terminal REPL for sending text, dots, and packed bitmap files.

Only completed PictoChat messages are submitted to the backend.

Exit criterion: multiple API clients behave as distinct virtual participants
against scripted protocol traffic.

Delivered:

- packed one-bit, 3,072-byte drawing messages;
- a versioned JSON HTTP protocol;
- bounded broker command and per-session event queues;
- participant roster updates for multiple virtual clients;
- interactive `ScriptedRadio` recording, RX loopback, and acknowledgement;
- delivered and timeout events;
- Rust broker and protocol tests;
- a Rust terminal REPL.

The former React/Vite frontend and WebSocket transport were intentionally removed
after development pivoted to a headless API and terminal workflow.

## Phase 2: hardware-free protocol model — complete

The provisional envelope has been removed. The current model combines documented
Nintendo structures, emulator behavior, and a historical PictoChat beacon capture:

- lossless parsing that preserves unknown bytes;
- association and room state;
- participant slots and identity fields;
- message fragmentation and reassembly;
- sequence, acknowledgement, retry, and timeout behavior;
- executable scripts representing known exchanges;
- golden round-trip tests and parser fuzzing.

Exit criterion: scripted exchanges represent the best supported model of a
real DS-hosted session, with every uncertainty explicitly tracked.

Delivered:

- lossless IEEE 802.11 MAC parsing with optional FCS preservation;
- typed Nintendo beacon, authentication, and association structures;
- PictoChat room discovery and participant-slot state;
- typed known fragment fields with unknown bytes retained as opaque data;
- offset-aware, out-of-order message reassembly;
- stable per-client virtual MAC addresses;
- Nintendo-shaped client reply, host command, and host acknowledgement flows;
- bounded duplicate suppression, sequence tracking, retries, and timeouts;
- a captured Room A beacon golden fixture and byte-preserving round-trip test;
- property-based arbitrary-input parser and fragmentation/reassembly tests;
- deterministic management, retry, timeout, multi-client, and API-broker scripts.

A new two-DS AR9271 capture has since replaced several model conventions with
wire evidence: the multiplayer prefixes, To-DS client address ordering, six-byte
application envelope, 160-byte fragments, explicit completion marker, host trailer,
and acknowledgement command counter now have exact golden fixtures. A complete
2,084-byte client message also reassembles in tests. Native message metadata, bitmap
packing, radio timing, and the API-to-native message conversion remain open and
are catalogued in [protocol-evidence.md](protocol-evidence.md).

## Phase 3: AR9271 backend — capture validation in progress

The AR9271 is now running on the Pi with `ath9k_htc`. The `picto-relay-listen`
binary performs tagged live radiotap capture, writes PCAP plus JSON scenario metadata,
and prettifies room, join, packet-family, fragment, message, and failure events. The
first two-DS baseline is decoded into golden fixtures. `picto-relay-join` now implements
the first injection milestone: captured-shaped authentication and binary-session
association requests, plus response and beacon-membership confirmation.

Implement the Linux backend in the existing `RadioBackend` boundary:

- monitor-interface and channel setup;
- raw RX/TX;
- radiotap parsing and construction as required;
- clean shutdown and actionable driver errors.

Validate reception, injection, variable source MACs, and receive-during-
transmit behavior. Add more radio abstractions only if hardware evidence calls
for them.

Exit criterion: the AR9271 backend can replace `ScriptedRadio` without changing
the engine.

## Phase 4: physical interoperability

Progress through the smallest observable milestones:

1. passively recognize a physical PictoChat room;
2. join a room hosted by a DS;
3. appear as one participant;
4. receive and send one message;
5. host a room that a DS can join;
6. represent multiple API clients over the one radio.

Client mode comes before host mode because the physical DS can act as the
behavioral and scheduling oracle. Multiple virtual stations come last because
they add shared-medium scheduling and identity multiplexing to an already
verified single-client path.
