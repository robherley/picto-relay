# Blueberry capture archive

Packet captures from Raspberry Pi `blueberry` use scenario-based names when their
contents have been verified. Timestamp-prefixed captures have a JSON sidecar with
the scenario, device roles, outcome, packet counts, and capture hash. A filename is
not treated as evidence on its own; use the sidecar and replay the PCAP with
`picto-relay-listen`.

## Canonical captures

| Capture | What it proves |
|---|---|
| `01-two-ds-baseline.pcap` | Baseline protocol exchange between two real Nintendo DS systems. |
| `1787193673345-real-ds-clean-join-baseline.pcap` | Cold Room A host, second real DS joins, idles, then leaves without sending a message. |
| `1787275056402-virtual-client-outbound-dot-padded-final.pcap` | Virtual AR9271 client joins a real DS host, initializes its profile, and delivers a dot; the host pads its short final rebroadcast fragment. This is the canonical outbound capture. |
| `1786660066567-joined-client-bitmap-validation.pcap` | Controlled repeated drawing shapes from the real DS client that joined second. |
| `1786659145818-client-dot-matrix.pcap` | Controlled real-client dot sequence with usable Nintendo traffic. |
| `20260821-sliding-window-hello-rendered-then-timeout.pcap` | Diagnostic first sliding-window send: the real DS rendered HELLO, but Linux captured only 15 of 16 drawing-reply markers and the software queue timed out. Its sidecar documents the cumulative-cursor fix this motivated. |
| `20260821-cumulative-cursor-half-o-fragment-skip.pcap` | Diagnostic cumulative-cursor session: an ACK-window race skips offset `0x0640`, visibly removing the lower half of the final `O` in `HELLO`. The sidecar records the exact offsets and notes the truncated final PCAP record. |
| `20260821-ack-window-retry-clean.pcap` | Mixed verification: two complete consecutive one-band messages prove offset `0x0640` retransmission fixed the half-`O`; a later five-band message stalls at cursor 50, preserving the next sliding-ring defect. |
| `20260821-direct-return-ring.pcap` | Canonical variable-height success: one session delivers 16-, 41-, 28-, 54-, and 67-reply messages. The last host rebroadcast is a complete 10,276-byte 256x80 drawing, proving more than four in-place ring wraps. |

## Supporting and negative captures

| Capture | Classification |
|---|---|
| `1787274121174-virtual-client-outbound-dot-pre-commit-partial.pcap` | Supporting outbound success before the firmware commit barrier. Explicitly partial because it was copied while open and ends with a truncated PCAP record. |
| `1787274651933-virtual-client-auth-timeout-room-already-occupied.pcap` | Negative authentication test. Despite its former `outbound-message-debug-1` name, it contains no association or drawing. |
| `identity-attempt.pcap`, `identity-clean-attempt.pcap`, `identity-offset-diagnostic.pcap`, `inline-identity-stall.pcap` | Virtual-client identity/profile handshake diagnostics. |
| `hidden-preload-control-stall.pcap`, `no-leak-sequence-diagnostic.pcap`, `prearmed-first-poll.pcap`, `session-state-fix-2.pcap`, `fresh-room-initialization-2.pcap` | Firmware responder state and timing diagnostics. Names describe the experimental change under test, not a successful protocol milestone. |
| `active-idle-host-4d.pcap` | Steady host/idle traffic diagnostic. |
| `20260821-known-good-three-band-control.pcap` | Negative baseline: valid multi-band fragments stop at cursor 25/offset `0x0e60`. |
| `20260821-proactive-credit-return.pcap` | Negative HTC-credit hypothesis test: a one-band control succeeds, but the three-band transfer still stops at cursor 25. |
| `20260821-in-place-message-ring-retry.pcap` | Supporting intermediate: retained-slot byte refresh works, but refill uploads still consume TX descriptors and the transfer stalls. |
| `session-state-fix.pcap` | Empty PCAP containing only the 24-byte global header; retain only as a record of the aborted run. |
| `1785458857956-client-one-pixel.pcap`, `1786659123866-client-dot-matrix.pcap` | Early captures whose sidecars show zero decoded Nintendo frames; superseded by later controlled captures. |
| `pictochat-ch1-20260730-185103.pcap` | Original exploratory channel-1 capture; superseded by `01-two-ds-baseline.pcap`. |

Temporary files under `/private/tmp` are working snapshots and are not canonical.
The archive copy plus its sidecar is the labeled source of truth.
