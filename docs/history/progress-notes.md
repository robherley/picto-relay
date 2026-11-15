> Historical investigation of the retired AR9271/Pi implementation. Commands and runtime claims below are not current. See the [ESP32 setup](../../README.md).

# Picto Relay progress notes

This is the chronological engineering log for active Picto Relay work. Each entry
records the observed behavior, the evidence retained, the implementation consequence,
and the next unresolved step. Protocol facts belong in `pictochat-protocol.md` and
`packet-shapes.md`; this file records how and why those facts changed over time.

## 2026-08-21

### Stable repeated outbound messages

- Verified that one virtual station can join a real DS Room A, publish the `Relay`
  profile, and send two drawings in one association.
- Verified a clean leave/rejoin followed by two more drawings without reloading the
  AR9271 driver or firmware.
- The repeated-send failures had four independent causes:
  - retained firmware TX buffers exhausted the AR9271 HTC credit pool;
  - API message IDs were incorrectly used as the two-byte wire transfer tag;
  - application fragment frame numbers restarted at `3` for every drawing;
  - the second drawing replayed the first drawing's `d0a` control values.
- The working sequence keeps transfer tag `0x0000`, advances fragment frames from
  `3..16` to `17..30`, advances announcement command counters `12, 15`, advances
  announcement transfer counters `5, 7`, and uses completion counters `6, 8`.
- Firmware now releases one-shot drawing buffers after transmission and releases all
  retained buffers on disarm.
- API `delivered` now requires a matching host `d0a` announcement echo followed by a
  complete host rebroadcast from the virtual station. A locally observed transmission
  alone is not considered delivery.

Evidence:

- `captures/pi/20260820-test2-false-delivery-and-timeout.{pcap,json}`
- `captures/pi/20260820-two-virtual-dots-only-first-rendered.{pcap,json}`
- `captures/pi/20260820-two-dots-after-clean-rejoin-success.{pcap,json}`

### Text rendering and the one-band limit

- A live capture showed that text sent by the REPL arrived as a valid 2,084-byte,
  256x16 drawing, but wrapped 256x96 input had been vertically resampled into that
  band. The lossy resampling caused missing/cut glyph strokes.
- The REPL now renders directly for the physical sixteen-row band:
  - short input uses a two-times pixel font;
  - longer input uses two one-times lines;
  - oversized input returns an error instead of silently truncating.
- The physical encoder still supports one 16-row band. Native PictoChat supports up to
  five bands (80 rows); full-height output requires a streamed firmware reply window
  because five bands produce 65 drawing fragments rather than the current 14.
- Clarification after physical testing: input beyond the current 52-character safe
  layout is not oversized for PictoChat. It is unsupported by Picto Relay's temporary
  one-band sender. The DS wire format has four additional 16-row bands available, and
  only the first band needs to avoid the top-left nametag. The correct long-term layout
  flows text through the lower bands instead of rejecting it.
- Found that an oversized text validation error escaped the interactive command loop,
  causing the REPL to delete its session and exit. Local text-layout errors are now
  printed at the prompt and treated as recoverable; the session remains connected.

### Sliding firmware reply window and multi-band output

- Replaced the fixed one-band encoder path with a variable-height encoder for one to
  five native 256x16 drawing bands. A full 256x80 message is 10,276 bytes and produces
  65 transfer fragments, plus its announcement and completion replies (67 total).
- The AR9271 still retains only sixteen drawing reply buffers. Those buffers now form
  a circular window rather than representing a complete fixed-size message:
  - firmware transmits logical reply `n` from physical slot `n mod 16`;
  - firmware waits for the DS host acknowledgement before advancing;
  - a failed-slot bit rearms the same buffer and retries the same logical reply;
  - a successful acknowledgement makes that physical slot available for refresh;
  - Rust uploads the next body and firmware copies it into the retained slot in place.
- The commit control frame now carries the total logical reply count, so firmware can
  stop after any one- through five-band message rather than after exactly 16 replies.
- The REPL now flows text through all five bands. The first rows start at x=88 to avoid
  the sender nametag; lower rows start at x=28 and use the wider calibrated area.
  Large text uses as many as five lines, with a compact ten-line layout as fallback.
- Rust codec, join-backend, and REPL tests pass, including round-tripping an 80-row
  drawing and verifying that it generates 67 reply bodies.
- Added an explicit firmware diagnostic for a queued drawing reply so Rust cannot
  mistake an idle reply for the first ring entry while the commit frame is completing.
- Extended the live delivery timeout to 20 seconds for the longer transfer plus host
  rebroadcast. If the transport itself has not accepted every reply at that deadline,
  the worker now leaves/disarms instead of loading another message over a live ring.
- Built and installed firmware
  `d5e2618b947ce5b2cae0a11a1483a4d2297fd8bf421205ac5fb1a8a80c6173b2`, then
  deployed the matching server and REPL. The final gateway is running with capture
  `/home/rob/picto-relay-captures/20260821-sliding-window-ready.pcap.gz`.
- A live session reached the Room A scan state, then timed out cleanly because no DS
  room beacon was present. Physical ring advancement and rendering therefore remain
  to be verified when the DS is back in Room A.

### Visible-area and nametag calibration

- A real DS sent a rough 256x80 outline of the area that remains visible around the
  PictoChat sender nametag.
- Decoded outline bounds were `x=24..251, y=0..79`. Interpreting the center of the
  thick hand-drawn stroke gives:
  - outer usable interior approximately `x=28..244, y=1..76`;
  - top-left nametag exclusion approximately `x=24..85, y=0..16`;
  - conservative one-band text range `x=88..243, y=0..15`.
- The REPL now lays out all one-band text inside that conservative range. Large text
  fits 13 characters; compact text fits two lines and at most 52 characters.
- The physical bitmap converter now preserves horizontal coordinates for drawings
  already 16 rows or shorter. Previously it recentered their ink bounds, undoing the
  renderer's deliberate avoidance of the nametag.
- Regression tests verify that compact text remains inside the calibrated safe area
  and that native-height ink retains its requested x coordinate.

Evidence:

- `captures/pi/20260821-visible-area-outline-calibration.{pcap,json}`

### Raspberry Pi power and runtime

- After installing the new power supply, `vcgencmd get_throttled` returned `0x0`:
  no current undervoltage/throttling and no such event recorded since boot.
- The kernel log contained no undervoltage or throttling messages.
- The AR9271 JSON gateway was redeployed with the calibrated encoder and restarted on
  capture path `/home/rob/picto-relay-captures/20260821-safe-area-testing.pcap.gz`.
- The API health endpoint returned the expected `ar9271` backend and 256x96 bitmap
  contract after restart.
- Restarted the calibrated gateway again for visual safe-area testing. The fresh
  capture path is `/home/rob/picto-relay-captures/20260821-safe-area-restart.pcap.gz`;
  the API returned healthy immediately afterward and power remained `0x0`.
- Rebuilding the missing Xtensa toolchain under four-core load set historical
  frequency-cap, throttle, and soft-temperature flags (`get_throttled=0xe0000`), but
  no current flags or historical undervoltage bit were set. Live radio testing was
  kept idle afterward.

### Verification

- `cargo test --workspace --locked` passes.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
  passes.
- The deployed Pi binaries were cross-compiled for `aarch64-unknown-linux-gnu`.

### First sliding-window hardware result and cumulative cursor fix

- The first physical test rendered `HELLO` on the real DS, proving that the new
  variable-length firmware transport can send a one-band message.
- The long text and dot queued behind it did not start. Rust timed out message 0 and
  left the room because its monitor capture contained only 15 drawing-reply markers
  for the 16-reply transfer. The firmware had progressed farther than Linux's
  per-frame bookkeeping.
- Pairing individual monitor copies of a firmware-queued reply with the next host ACK
  is therefore not a reliable sliding-window clock. Monitor capture loss can leave
  the software cursor behind even when the DS accepts and renders the message.
- Firmware now exports a cumulative accepted-reply cursor as `0xA500 | cursor` in the
  captured host command's duration field. Rust catches up from any later cursor value
  and refills every newly freed ring slot, so one or several missed frames no longer
  stall the window.
- Delivery now requires both the matching host announcement and a cursor equal to the
  message's total reply count. A 20-second message timeout clears only that message;
  it no longer tears down an otherwise healthy physical room session.
- Added regression coverage for decoding the firmware cursor, catching up several
  lost observations at once, and requiring both acceptance signals before delivery.
- Built and installed firmware
  `66394a76b09c0b2bfb5d309fa242d1565fdc60f9cabff8e05c53a48e27481d71`, deployed
  the matching server and REPL, and started the gateway with capture
  `/home/rob/picto-relay-captures/20260821-cumulative-cursor-verification.pcap.gz`.
- The API health check passed. The immediate physical verification attempt reached
  the Room A scan state and timed out without a beacon, so consecutive-message and
  multi-band rendering remain the next real-DS checks.

Evidence:

- `captures/pi/20260821-sliding-window-hello-rendered-then-timeout.{pcap,json}`

### Missing drawing fragment and acknowledgement-window retry

- In the cumulative-cursor verification session, the real DS rendered the first
  `HELLO`, but the lower half of the final `O` was absent. This was not safe-area or
  font clipping: `HELLO` occupies `x=88..145`, well inside the calibrated boundary.
- The host rebroadcast jumped from drawing offset `0x05a0` to `0x06e0`, omitting
  `0x0640`. That missing 160-byte fragment contains the lower 8-pixel tile row of the
  `O`, exactly matching the visible defect.
- At logical reply cursor 11, firmware diagnostics showed an idle reply queued while
  the preceding drawing reply still awaited its host acknowledgement. A later
  successful acknowledgement for that idle round was then applied to the drawing
  cursor, skipping reply 11.
- Host commands and acknowledgements can be pipelined, so a later command does not
  prove that the preceding drawing failed. Firmware now chooses the safe idempotent
  behavior: while that drawing acknowledgement is unresolved, it rearms the retained
  buffer and retransmits the same logical fragment on a later targeted command instead
  of sending an idle substitute. The cursor therefore cannot advance from an idle
  acknowledgement.
- Patched firmware builds successfully as
  `ef879a927fec1694a3742119d97f544e22b4eb9ec3f0f9429879976856475107`.
- A clean real-DS run delivered `HELLO` and `HELLO AGAIN` consecutively. Both host
  rebroadcasts contain every offset from `0x0000` through the final `0x0820`; the first
  drawing reassembles as 256x16 with ink at `x=88..145, y=1..14`. Offset `0x0640` is
  present, so the lower tile row of the final `O` is no longer missing on the wire.
- The same run exposed a separate multi-band defect. A 67-reply, five-band message
  advanced through cursor 50 and the host rebroadcast contiguous data through offset
  `0x1e00`, then radio traffic stopped and delivery timed out. A following four-band
  dot also timed out because the virtual participant had already stopped progressing.
  Sliding-ring transfers beyond one band are therefore still experimental.

Evidence:

- `captures/pi/20260821-cumulative-cursor-half-o-fragment-skip.{pcap,json}`
- `captures/pi/20260821-ack-window-retry-clean.{pcap,json}`

### Multi-band ring resource exhaustion and direct-return fix

- Repeated three-band controls stopped at cumulative cursor 25. The DS host had
  accepted the announcement plus 24 contiguous fragments and rebroadcast offsets
  `0x0000..0x0e60`; the PictoChat transaction itself was valid.
- Cursor 25 matched the AR9271's 33-credit budget after retaining nine identity/idle
  templates and sixteen message templates. Delayed-ACK and command-ACK FIFO variants
  did not change the boundary. Proactive HTC credit reporting also stopped at cursor
  25, so its threshold was not the complete repair.
- The first in-place experiment proved that copying a new body into an accepted ring
  slot works, but its temporary refill uploads still traversed the Wi-Fi TX queue and
  exhausted target descriptors.
- The final firmware intercepts a refill after copying it, prepares the retained ring
  buffer, and returns the temporary upload directly to HTC. It neither consumes a new
  retained credit nor sends a private refill frame over the air.
- One physical session then delivered five consecutive messages containing 16, 41,
  28, 54, and 67 replies. The final transfer reassembled as a complete 10,276-byte,
  256x80 drawing, proving the maximum five-band path across more than four ring wraps.
- Validated firmware SHA-256:
  `67a59440cdc7de433d9d2668363fb59cff435e374a49c4dda1974fdb13cac37a`.

Evidence:

- `captures/pi/20260821-known-good-three-band-control.{pcap,json}`
- `captures/pi/20260821-proactive-credit-return.{pcap,json}`
- `captures/pi/20260821-in-place-message-ring-retry.{pcap,json}`
- `captures/pi/20260821-direct-return-ring.{pcap,json}`

### Next work

1. Confirm the safe-area contour on messages from nicknames with different lengths;
   decide whether the exclusion width is fixed or name-dependent.
2. Exercise repeated maximum-height messages and receive-side traffic over a longer
   soak session.
3. Decode the remaining profile metadata and drawing-control fields.

## 2026-08-22

### Runtime-boundary and lint cleanup

- Removed the unused deterministic engine crate. Its room state was only read by its
  own tests, timer wakeups were discarded by the broker, and its synthetic
  acknowledgement model did not match physical captures.
- Replaced the default server backend with an explicit in-memory JSON-contract test
  double. It loops messages back at the API level and makes no Nintendo protocol or
  physical-delivery claim. The working AR9271 path remains the only interoperable
  backend.
- Converted `picto-relay-join` to a normal library plus a three-line binary entry
  point. Removed the `#[path = "main.rs"]` module inclusion and blanket dead-code
  suppression that previously hid unused items.
- Removed the capture-derived `--experimental-profile` path because it retains the
  session-deadlock behavior disproven on hardware. Profile initialization now has one
  supported firmware-assisted implementation.
- Removed unused codec API (`build_data_frame_to_ds`, `FrameControl::more_fragments`,
  `MacFrame::fcs`, and `encode_one_band_message`) and consolidated hexadecimal fixture
  parsing in one test-only source module beside the codec capture fixtures.
- Promoted Rust warnings plus Clippy `all` and `pedantic` lints to workspace errors.
  Added GitHub Actions checks for formatting, the full workspace test suite, and the
  documented strict Clippy command.
- Verification after the cleanup: 87 tests pass; strict Clippy and formatting checks
  pass.

## 2026-08-23

### Test and radio-boundary cleanup

- Removed `proptest` and its two randomized codec tests. They were useful while the
  codec was speculative, but the project now has deterministic capture-backed parser,
  fragmentation, reassembly, malformed-input, and physical-flow coverage without the
  additional randomized-test dependency graph.
- Removed the public `ScriptedRadio`, its matching/masking language, and its
  script-specific production error variants. Join-flow tests retain a private,
  exact-frame ordered test double containing only receive, transmit, and completion
  checks.
- Removed the server's obsolete `scripted` spelling for the protocol-free `memory`
  backend and renamed the fragment-size constant to describe the wire format rather
  than an old test harness.
- Confirmed that the room BSSID is discovered from beacon Address 2 at runtime and is
  passed through every live management and data operation. Only Nintendo's logical
  `03:09:bf:*` multiplayer destinations remain fixed.
- Removed the zero-BSSID placeholder formerly used while constructing message bodies;
  the codec can now encode a fragment envelope without manufacturing and discarding
  an 802.11 header.
- Verification after this cleanup: 86 tests pass; the full all-targets/all-features
  strict Clippy command and formatting check pass.

### Browser client and room-aware API

- Replaced the terminal REPL with a framework-free browser client embedded directly
  in `picto-relay-server`; there is no frontend build step or external asset runtime.
- Added room selection to session creation and `GET /api/v1/rooms` snapshots for rooms
  A through D. Memory-backend messages and participant updates are isolated by room.
- The AR9271 backend remains one physical virtual station at a time, but the browser
  may select which room that station joins.
- The page renders incoming packed bitmaps, auto-scrolls received messages, provides a
  pointer pen/eraser canvas, and converts typed text into the same monochrome bitmap
  contract.
- Replaced browser-dependent canvas text with the REPL's exact 5×7 bitmap glyph table
  and calibrated large/compact wrapping rules. Text is now pixel-identical across
  browsers, avoids antialiasing, and preserves the top-left nickname exclusion area.
- Browser checks covered two clients joining Room C, roster propagation, text drawing
  delivery to both clients, leaving, and a 390-pixel responsive viewport.
- Verification after the replacement: 82 Rust tests pass; strict Clippy, Rust
  formatting, JavaScript syntax, and whitespace checks pass.
- Cross-compiled and deployed the embedded-web server to `pi`, then started the
  AR9271 backend on port 3001. The verified browser URL is
  `http://pi:3001/`; the session capture and log are
  `/home/rob/picto-relay-captures/20260823-220015-web-ui.{pcap,log}`.

### Physical participant names

- Added typed decoders for the 84-byte `e30` identity record and 104-byte `c34`
  member/session roster. Nicknames decode from the fixed ten-unit UTF-16LE field and
  station MACs decode from Nintendo's word-swapped representation. A capture-backed
  Japanese-name fixture verifies that trailing U+3000 padding is removed alongside
  the U+0000 padding used by ASCII profiles.
- The live worker now joins profile names to the sixteen roster slots, emits complete
  participant snapshots, removes client-originated reason-3 deauthentications, and
  resolves incoming drawing senders through the same nickname table.
- The live room endpoint and browser participant list now show physical DS clients for
  the actively joined room. Unobserved profiles retain a stable `DS <mac>` fallback;
  the other three rooms remain unknown while the single virtual station is joined.
- Cross-compiled and deployed the implementation to `pi`. A live Room A API
  join resolved host `00:24:f3:67:41:87` to nickname `reb`, retained virtual nickname
  `Relay`, and exposed both through the participant event and `GET /api/v1/rooms`.
  The verification capture and log are
  `/home/rob/picto-relay-captures/20260823-223821-physical-roster.{pcap,log}`.
- Final verification: 89 Rust tests pass; strict Clippy, Rust formatting, JavaScript
  syntax, and whitespace checks pass.

### WebSocket browser transport

- Replaced the browser's 1-second room refresh and 500-millisecond session-event
  polling timers with pushed WebSocket streams. Room subscribers receive a complete
  snapshot immediately and after roster changes; session subscribers receive one
  versioned event envelope per text frame.
- Kept `GET /api/v1/rooms` and `GET /api/v1/sessions/:id/events` intact for scripts
  and existing clients. A connected session WebSocket exclusively owns that session's
  bounded receiver; disconnecting restores it so reconnect or REST polling can resume.
- Added a broker `watch` channel so roster delivery is driven by state changes rather
  than per-browser request traffic, plus coverage for snapshot publication and a
  browser regression check that rejects polling intervals.
- Local verification: all 91 workspace tests pass, Rust formatting is clean, and the
  embedded JavaScript passes `node --check`.
- Cross-compiled and deployed the gateway to `pi`. The live AR9271 server
  immediately returned a four-room snapshot over `/api/v1/rooms/ws`, and the deployed
  browser asset contains both WebSocket clients with no `setInterval` polling loop.
  The running capture and log are
  `/home/rob/picto-relay-captures/20260823-230100-websocket-ui.{pcap,log}`.

### Participant lifecycle events

- Added versioned `participant_joined` and `participant_left` session events. Each
  carries the room plus the participant's stable ID and last-known nickname; both the
  WebSocket stream and retained REST polling endpoint expose the same envelopes.
- The memory broker publishes lifecycle deltas to all remaining members of the room.
  The physical broker compares successive host rosters by station-derived ID, so a
  newly decoded nickname does not create a false leave/rejoin pair.
- The first physical roster is treated as a baseline for users who were already in the
  room. The relay announces its own join only after the profile handshake reaches the
  ready state; subsequent physical additions and removals are emitted explicitly.
- The browser renders joined and left notices in the auto-scrolling message timeline
  while continuing to replace its client list from authoritative roster snapshots.
- A local two-client WebSocket scenario observed Alice joining, Bob joining, and Bob
  leaving with the expected room and participant objects. All 94 workspace tests and
  strict Clippy pass; JavaScript syntax and repository whitespace checks are clean.
- Cross-compiled and deployed the lifecycle build to `pi`; the AR9271 gateway
  is running on port 3001 and serves both lifecycle handlers. Its capture and log are
  `/home/rob/picto-relay-captures/20260823-231500-participant-lifecycle.{pcap,log}`.

### Test helper package removal

- Removed the `picto-relay-test-support` workspace package and all three dev-dependency
  edges that existed solely for one hexadecimal fixture decoder.
- Kept one test-only decoder source beside the codec capture fixtures. Codec integration
  tests include it normally; join and listener tests include the same source by path,
  avoiding both production API and copy-pasted implementations.
- Regenerated `Cargo.lock`; Cargo metadata contains only the five runtime crates. All
  93 remaining workspace tests pass.

### Live room counts

- Added a singular/plural client count to every room-card heading. Counts and nickname
  lists are rendered from the same complete room snapshot, so WebSocket join/leave and
  physical-roster updates change them together without polling.
- Deployed the embedded asset to `pi`; the running capture and log are
  `/home/rob/picto-relay-captures/20260824-004500-room-counts.{pcap,log}`.

### Passive physical room counts

- Corrected the live lobby semantics: the broker previously published only the
  detailed roster learned after the relay itself joined, so a DS-only room could not
  update the lobby.
- Added a read-only AR9271 observer that decodes Nintendo room beacons continuously,
  reports A-through-D user counts, and expires hosts after two seconds without a
  beacon. Adapter preparation now happens once at server startup so the observer and
  active session share the monitor interface without either recreating it.
- Added `participant_count` to room REST and WebSocket snapshots. Detailed
  `participants` remain empty until profiles are known, avoiding invented identities.
- Added observer expiry/aggregation tests, broker/API count tests, and updated the
  embedded lobby to distinguish detected physical clients from named roster members.
  All 98 workspace tests and strict Clippy pass.
- Deployed and restarted `pi`. Live verification showed the REST snapshot and
  browser WebSocket update converge on `Room A · 1 client` while one real DS was in the
  room. The current capture and log are
  `/home/rob/picto-relay-captures/20260823-passive-room-counts.{pcap,log}`.

### Software host milestone 1

- Added captured-shape encoders for `PictoChat` room beacons, successful open-system
  authentication responses, and Nintendo association responses. The latter two match
  exact real-host frames from the clean two-DS baseline.
- Added `picto-relay-host`, a software-only AR9271 probe that advertises a runtime BSSID,
  rotates beacon state, validates the joining client's binary session fields, assigns
  slots 1 through 15, updates the advertised user count, and reports deauthentication.
- The probe intentionally stops before Nintendo multiplayer poll rounds. Its first
  physical criterion is that a real DS sees Room A and reaches association without any
  timing-specific firmware behavior.
- Added codec round-trip/capture tests and host session/slot tests. All 104 workspace
  tests and strict Clippy pass before physical deployment.
- The first 60-second physical trial radiated the intended beacon, but the DS sent no
  authentication or association request and instead began hosting its own Room A.
  Capture comparison isolated the key architectural mismatch: user-space injection
  supplied a synthetic beacon timestamp, while a working ESP32 host uses the radio's
  AP beacon engine and hardware TSF.
- Reworked the probe to start a raw Linux AP beacon template through nl80211 using the
  Pi's existing `iw` binary. The template deliberately omits SSID and TIM information;
  ath9k inserts TSF/TIM and transmits at the configured 100-TU interval, while the
  Nintendo vendor element remains in the software-owned tail. No hostapd package or
  additional firmware change is needed for the next discovery trial.
- Physical AP-template trial succeeded. DS `00:24:f3:67:41:87` selected the software
  room, authenticated, and associated as slot 1 eleven times. Ten cycles ended in a
  client deauthentication roughly 4.09 seconds later because the host sent no CMD
  polls; the final cycle was still associated when the bounded probe stopped.
- Labeled the canonical capture as
  `captures/pi/20260823-software-host-ap-template.{pcap,json}`. It contains 587
  host beacons, 11 authentication requests, 11 association requests, 10 client
  deauthentications, and no Nintendo data frames. This establishes that custom adapter
  firmware is not required for the host management plane; the host poll loop is next.
- Implemented the first software host heartbeat loop. Each round sends a From-DS
  Data-CF-Poll command with `client_time=998`, the associated participant mask, the
  captured two-byte idle payload, and the capture-derived 1,248-microsecond reservation.
  Client replies are collected until that fixed boundary, then Rust sends a From-DS
  host acknowledgement with the failed-slot mask before a 2,250-microsecond round gap.
- Corrected the host command and acknowledgement encoders to carry their captured
  From-DS flags and MAC duration fields. Added exact MAC-shape tests plus heartbeat
  mask/state coverage. All 108 workspace tests and strict Clippy pass; physical
  sustained-association verification is next.
- The first heartbeat trial associated real DS `00:24:f3:67:41:87` as slot 1, then
  captured 6,458 host polls and acknowledgements but zero client data replies. Packet
  comparison exposed an application-state error rather than another timing failure:
  PictoChat requires an opening `c34` member roster, while the probe sent the two-byte
  idle body that is only valid after initialization.
- Replaced that idle body with a capture-shaped `0x1c34`/sequence-5 envelope. The
  generated 104-byte roster encodes the software host in slot 0, each associated DS in
  its assigned slot, vacant zero entries, and the observed four-byte trailer. The host
  now recognizes and reports the client's `0x8c34` session response. All 109 workspace
  tests and strict Clippy pass, and the corrected binary is deployed for the next
  physical trial.
- Two `c34` physical diagnostics both associated DS `00:24:f3:67:41:87` but received
  no client data. In the first, the roster was queued 24 microseconds after the
  association response and raced ath9k's transmission of that response. Adding the
  captured 20-millisecond admission delay moved the first roster safely past
  association but still produced no reply.
- The delayed capture then exposed the remaining radio-queue discrepancy: monitor
  echoes place the host acknowledgement about 1.00 milliseconds after the roster,
  roughly 250 microseconds earlier than the 1.248-millisecond real-host boundary.
  The deployed probe now schedules 1.500 milliseconds in userspace to compensate.
  A follow-up window received no authentication request, so physical verification of
  this compensation remains pending.
- A physical retry proved that simply raising the userspace reservation from 1.248 to
  1.500 milliseconds did not move the air boundary: it remained about 0.999
  milliseconds. The blocking receiver wakes on ath9k's transmit echo after the nominal
  deadline, so the old scheduler immediately emitted the acknowledgement.
- Resynchronizing the deadline from that echo identified the right event but exposed
  the receiver's fixed 20-millisecond timeout: without a following frame, the ACK was
  emitted roughly 10 milliseconds after the echo. Added opt-in 100-microsecond receive
  mode to `picto-relay-radio` and use it only in the dedicated host probe; the gateway
  retains its normal timeout. All 109 workspace tests and strict Clippy pass. The first
  low-latency trial window received no authentication, so air-timing verification is
  still pending.


### Host review corrections and firmware-assisted rounds (2026-09-09)

- Supersedes the echo-timing conclusions in the historical host entries above.
  Local TX status is delivery evidence, not an RF timestamp. The newer m7/m8
  captures include real DS associations but zero client data and truncated tails.
  Their `.analysis.json` files distinguish requests from status echoes.
- Replaced the missing host initialization with a capture-backed two-pass state
  machine. Transactions remain stable on retries; opaque roster bytes no longer
  use invented per-command entropy. Application state advances only on expected
  replies in completed rounds.
- Added host-round firmware v1: single-slot polling, explicit 2 Mbps short preamble,
  TSF-based ACK service, failed masks and bounded buffer lifetime. Linux waits for
  a round result and never schedules the ACK from an echo. Current radio metadata
  distinguishes TX status from RX and exposes the reported PHY rate.
- Replaced the accidental zero-timeout busy loop with explicit cooperative polling.
  Fixed live API session ID reuse, including counter exhaustion without wrapping.
- Added real-client transcript, firmware deadline/wrap/mask, host result/retry,
  radiotap and stale-session regression tests. Complete firmware compilation and
  physical validation remain pending; the Pi was unreachable during this work.
- Final local verification: 118 Rust tests and strict Clippy pass; the host binary
  cross-compiles for aarch64 Linux. Both firmware patches apply to the pinned base,
  patched C passes syntax checks, and the actual host-round functions pass the C
  harness. The minimum Rust version and CI are now 1.88 (let-chain support).

### ESP-WROOM-32 client and readable HELLO (2026-09-09)

- Developed the ESP-IDF C client independently in `firmware/esp32`.
- A narrowly scoped private radio metadata hook avoids ordinary unicast ACK/retry
  policy for Nintendo replies; direct RX replies completed the profile exchange.
- Reduced drawing fragments from 160 to 80 bytes to fit the real reply window.
- The initial HELLO at x=4 was covered by the DS sender nametag. Applied the
  existing visible-area calibration: HELLO now spans x=88..145, y=1..14.
- All five ESP32 host-side C test harnesses passed, the pinned ESP-IDF build
  passed, and the connected board's application flash was hash-verified.
- User confirmed ESP32 appeared and the corrected HELLO was fully visible.
- Pi wlan1 captured 15,049 packets with zero kernel drops. Monitor reception
  still missed some fragments, so the layout PCAP is not a complete reassembly.
- This is a 60-second, single-invited-slot client experiment. Persistent gateway
  integration and the original relay-host disconnection remain separate work.

Evidence: `captures/pi/20260909-esp32-layout.analysis.json` and
`captures/esp32/20260909-layout.log`.

### Rust ESP32 client rewrite (2026-09-09)

- Moved packet classification, channel parsing, authentication/association, profile
  state, ACK correlation, fragment encoding and HELLO drawing into allocation-free
  Rust under `firmware/esp32/rust`.
- ESP-IDF setup/UART diagnostics and the proven private radio hooks remain C.
  The prior C protocol implementation is a frozen regression reference, not linked.
- The protocol engine accepts frames, clock values and a transmit function. It has
  no platform dependencies; the FFI adapter owns shared storage and uses a
  nonblocking guard so Wi-Fi RX never waits on the main control loop.
- Ten Rust tests cover byte parity with the real-DS-verified C implementation,
  compatibility with the existing Rust drawing codec, complete profile/message
  exchange, malformed frames, retry limits, TX failures, host filtering, sequence
  wrap and ACK correlation. Strict Clippy and formatting passed, as did the five
  original ESP32 C harnesses against their retained reference/hook sources.
- Built with Espressif Rust 1.97.0.0 and the existing pinned ESP-IDF 5.5.2. Flashed
  with write-hash verification. Hardware reached connected state, profile phase
  12 and acknowledged message stage 29. The user reported no new HELLO in that first run. The repeat capture showed
  broadcast host deauthentication and unanswered client authentication. Added
  broadcast-leave handling and separate host-echo delivery diagnostics; final
  hardware verification is pending.
- This rewrite preserves the bounded HELLO hardware experiment. Persistent USB
  command/event framing and the desktop server transport are subsequent integration
  work, not part of this firmware parity test.

- Hardware follow-up: moved authentication-to-association into the RX entry point
  after the capture showed about 6 ms of control-loop delay (the successful C
  capture was about 1 ms). Added a regression test for immediate association.
- Added application delivery diagnostics: control echoes match the same 16-byte
  payload prefix used by the desktop client; all 27 matching fragment echoes and
  completion are required in addition to MAC acknowledgement.
- Later Rust runs stalled in profile initialization. Reply preparation measured
  about 19 us; radio response delays were larger. A C A/B run also failed to
  authenticate while DS RSSI had fallen to about -75 dBm. Closer-range physical
  verification remains outstanding; do not claim Rust drawing delivery yet.
- Restored the default Rust build after the explicit C comparison. `PICTO_REFERENCE_C`
  is an opt-in diagnostic build option; the web-server transport remains future work.


### ESP32 Rust TX lifecycle compared with Pi patches (2026-09-09)

Implemented retained profile retries, one in-flight engine submission,
sequence-correlated TX completion via a nonblocking atomic mailbox, deferred
host-ACK reconciliation, and fail-closed missing-completion handling. Fifteen
Rust tests, strict Clippy, formatting and the pinned ESP-IDF build pass. Flashed
v7 joined but remained at profile phase 0 (1,056 submitted replies, 129 matching
negative host results) before DS disconnect reason 1. HELLO remains unverified.
The pinned ESP32 driver computes backoff per frame; queue mapping/private ABI
work remains before enabling an equivalent of the AR9271 zero-backoff patch.
Details: `firmware/esp32/PI-COMPARISON.md`.


### ESP32 Rust late-queued reply diagnosis and verified HELLO (2026-09-10)

- C comparison and Rust v8 both displayed HELLO, confirmed by the user. Failed
  Rust runs were correlated using on-device event traces and CRC-recovered Pi
  poll sequences (the patched capture firmware rewrites those fields).
- Concrete failure: client TX 175 queued on poll 4088, missed ACK 4089, remained
  queued for about 8 ms, then arrived during poll 4091 and received positive ACK
  4092. Rust had discarded its proposed phase on poll 4091 and ignored success.
- v11 rebinds that still-in-flight logical reply to the later eligible single-slot
  poll, preserving an already positive result. Added exact trace regression and
  callback/ACK ordering tests. One driver submission remains in flight.
- Fixed premature aborts from broadcast cleanup before membership. Fixed the
  false-negative delivery indicator: DS completion is acknowledged, not echoed.
- The experimental alternate queue never completed its first TX and was removed.
  v11 retains the stock raw-TX queue and pins libpp.a as well as libnet80211.a.
- The user confirmed a new visible HELLO from v11; it reports phase=12,
  message=29, echoed=27 and delivered=1. Eighteen Rust tests, strict Clippy,
  formatting, compatibility harness and ESP-IDF build pass.

- Final v11 stayed connected through the full 60-second test and stopped normally.
  Pi captured 4,708 packets, zero kernel drops, and all 2,084 bytes of DS-echoed
  drawing data. wlan1 was restored to managed mode; the stock-queue v11 remains
  flashed. Hardware log and capture summaries use the `20260910-v11` prefix.


## 2026-09-10 — Rust ESP32 USB bridge

Implemented the laptop/Pi server → USB → Rust ESP32 architecture. The shared
`picto-relay-link` no_std crate owns CRC-protected framing; the firmware stores
complete outgoing transfers in a bounded arena and keeps the timed radio state
machine on-device. The server reuses existing bitmap encoding, roster/profile
parsing and incoming 80/160-byte fragment reassembly. `--backend esp32 --serial
/dev/cu.usbserial-0001 --channel 1` serves the existing browser UI. Sessions persist,
LEAVE sends directed deauthentication, and five-second owner/host silence expires
the radio session. Serial errors and stalled profile exchange trigger reconnects.

Hardware testing found and fixed three integration regressions:

- macOS restored serial settings when `stty` closed the last descriptor. Hold the
  server descriptor open while configuring the terminal to preserve timed reads.
- USB framing/duplicate detection added Wi-Fi callback work and flash-cache
  pressure. Move forwarding through a bounded RX queue to the control task and
  place the Rust radio hot path in instruction RAM. Measured reply preparation
  fell from roughly 36–53 µs to 9–10 µs (25 µs maximum in the final live run).
- The DS remembers fragment counters across drawings. Restarting at frame 3
  caused a second drawing to lose echoes for previously used numbers. The server
  now allocates continuous, wrapping u16 fragment numbers within each session.

Final host run: three delivered messages in one Room A session: browser text
“ONE” (27 fragments), browser text “SECOND MESSAGE” (52), and an API-generated
256×80 border with crossing diagonals (129). Each reported all exact echoes and
final MAC ACK, and appeared once in the browser with its delivery event. The
session stayed ready for over 150 seconds. Repeated join/leave/server restart
cycles also completed profiles and recovered the DS nickname. Physical DS visual
confirmation and an independently DS-originated drawing into the browser are
still awaiting the user's reply; do not claim those are verified yet.

Evidence: `captures/esp32/20260910-usb-bridge.json` and its
serial-worker log. Flashed image SHA256:
`4b50e0b1d78b4f14749c293c39927703aa670618a0413987998fd56964b86fba`.
A copy is saved as `backups/verified-rust-usb-bridge.bin`. Filtered Pi captures
are under `captures/pi/20260910-usb-*`; the passive capture misses some
frames, so it is corroborating RF evidence rather than the complete delivery
oracle. `usb-v4-first.pcap.gz` contains records before 00:21:00 UTC from the original
v4 capture to isolate its first message for the single-message analysis script.
Pi wlan1 was restored to managed/up; wlan0 was never changed.

Validation: full Rust workspace tests passed; subsequent server/link tests and
strict Clippy passed after the fragment-number fix; firmware's 19 host tests and
strict all-features Clippy passed; C compatibility harness passed under ASan/UBSan;
ESP-IDF build and flash verification passed. Firmware tests cover full-height and
repeated uploads, incomplete/wrong-ID rejection, persistent sessions and leave.
Host tests cover 80/160-byte reassembly, duplicate messages, full-height bitmap
roundtrip, and fragment-counter wraparound.

See `docs/esp32-usb.md` for the protocol and current scope. This is still a client
of a DS-hosted room; ESP32 hosting and passive room scanning are not implemented.


## 2026-09-10 — Full-height USB latency

The user's delay report matched the running worker log: single-band uploads took
~0.4 s and deliveries ~0.9–1.3 s, while full-height uploads took 1.84–2.04 s and
deliveries 4.77–5.52 s. Bursts waited behind prior drawings. The ESP32 had remained
connected for the user's subsequent messages, so this was throughput/queue latency.

Added capability-negotiated packed USB batches. The host compresses zero runs per
application body and packs multiple records into one bounded command. Firmware
validates the entire batch before mutating its upload cursor. The original BODY
path remains supported, and the host uses it for firmware without the capability.
This changes neither the actual DS packet bytes nor the timing-critical radio path.

Repeated the exact earlier synthetic 256×80 border/cross drawing three times:
USB upload 423/426/408 ms (formerly 1840 ms); delivery 3598/2963/3009 ms (formerly
4767 ms). Each transfer retained all 129 fragment echoes and final MAC ACK.
The full upload now takes 15–16 commands instead of 131. Actual savings depend
on drawing content and radio conditions; the DS still receives all native fragments.

The existing backend-state event now shows upload percentage, fragments sent and
confirmed, and number of drawings waiting. Added informational timing logs, packed
roundtrip/size bounds tests, malformed zero-run tests, dense fallback tests, and
an atomic-batch rejection test through the firmware's full-transfer simulation.
Server/link tests, firmware tests and strict Clippy pass; ESP-IDF build/flash pass.
Evidence: `captures/esp32/20260910-usb-packed.json` and log.
The web session was recreated without reloading its canvas or history. No Pi
configuration changes were needed for these tests.
