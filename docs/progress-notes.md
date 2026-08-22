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

- `captures/blueberry/20260820-test2-false-delivery-and-timeout.{pcap,json}`
- `captures/blueberry/20260820-two-virtual-dots-only-first-rendered.{pcap,json}`
- `captures/blueberry/20260820-two-dots-after-clean-rejoin-success.{pcap,json}`

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
  `/home/rob/picto-relay-captures/20260821-sliding-window-ready.pcap`.
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

- `captures/blueberry/20260821-visible-area-outline-calibration.{pcap,json}`

### Raspberry Pi power and runtime

- After installing the new power supply, `vcgencmd get_throttled` returned `0x0`:
  no current undervoltage/throttling and no such event recorded since boot.
- The kernel log contained no undervoltage or throttling messages.
- The AR9271 JSON gateway was redeployed with the calibrated encoder and restarted on
  capture path `/home/rob/picto-relay-captures/20260821-safe-area-testing.pcap`.
- The API health endpoint returned the expected `ar9271` backend and 256x96 bitmap
  contract after restart.
- Restarted the calibrated gateway again for visual safe-area testing. The fresh
  capture path is `/home/rob/picto-relay-captures/20260821-safe-area-restart.pcap`;
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
  `/home/rob/picto-relay-captures/20260821-cumulative-cursor-verification.pcap`.
- The API health check passed. The immediate physical verification attempt reached
  the Room A scan state and timed out without a beacon, so consecutive-message and
  multi-band rendering remain the next real-DS checks.

Evidence:

- `captures/blueberry/20260821-sliding-window-hello-rendered-then-timeout.{pcap,json}`

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

- `captures/blueberry/20260821-cumulative-cursor-half-o-fragment-skip.{pcap,json}`
- `captures/blueberry/20260821-ack-window-retry-clean.{pcap,json}`

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

- `captures/blueberry/20260821-known-good-three-band-control.{pcap,json}`
- `captures/blueberry/20260821-proactive-credit-return.{pcap,json}`
- `captures/blueberry/20260821-in-place-message-ring-retry.{pcap,json}`
- `captures/blueberry/20260821-direct-return-ring.{pcap,json}`

### Next work

1. Confirm the safe-area contour on messages from nicknames with different lengths;
   decide whether the exclusion width is fixed or name-dependent.
2. Exercise repeated maximum-height messages and receive-side traffic over a longer
   soak session.
3. Decode the remaining profile metadata and drawing-control fields.
