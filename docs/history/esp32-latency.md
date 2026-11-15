> Historical investigation log, moved out of the USB contract. Build flags, baud
> rates, fragment sizes, capability bits and status layouts described here are the
> options that existed at the time. The firmware now has one fixed configuration
> (921600 baud, 120-byte fragments, prepared replies) and was since ported from
> Rust to C. Measurements remain valid as evidence.

# ESP32 latency investigations (September 2026)

## Measured full-height latency

With the same 256×80 synthetic border/cross drawing, packed USB batches reduced
upload time from 1.84 seconds to 0.41–0.43 seconds. Three measured deliveries took
2.96–3.60 seconds, compared with the prior 4.77-second delivery. Sparse drawings
compress best; dense drawings can use raw records inside batches. Radio transfer
still sends all 129 fragments and depends on DS polling and retries. Queued drawings
wait for the previous delivery. The browser reports upload, radio progress and queue
depth through backend-state events. See the saved
[evidence](../captures/captures.tar.gz).


### Radio timing investigation: 2026-09-11

The private SDK objects had lost ESP-IDF's archive-specific instruction-RAM
mapping. The build now preserves those mappings, additionally relocates measured
raw-send helpers without changing their instruction bytes, and checks 19 linked
function addresses. Nintendo replies use zero AIFS/backoff through the pinned
HAL and drain the existing transmit queues from the Wi-Fi callback. Driver
allocation, buffer ownership and completion tracking remain in place.

In same-device experiments, queue-only 80-byte transfers took 3.05–4.01 seconds
versus a fresh 5.06–5.81-second baseline. Partial RAM relocation delivered in
2.71–3.01 seconds. Correcting all private-object mappings reduced monitor-derived
median post-poll reply delay from 398 to 135 microseconds, but that run's full
transfers took 5.32–6.68 seconds with wider observed poll gaps. Faster processing
therefore does not yet establish a consistent end-to-end speedup. The 160-byte
trial still stalled at the first fragment and was disabled by default. See the
[experiment evidence](../captures/captures.tar.gz)
and [driver comparison](history/esp32-pi-comparison.md).


After resetting Room A for the final check, **six consecutive full-height
transfers delivered in 2.768–2.871 seconds** (USB upload 0.451–0.474 seconds).
Each completed with all 129 exact fragment echoes and the final ACK. The final
capture contains 5,191 packets with zero kernel drops. This is the active,
80-byte default configuration; the earlier wider-poll-gap run above remains
recorded to show the observed variability. Final firmware SHA-256 and individual
times are in the same evidence JSON under `final80`.


### Continued latency work: prepared replies and 120-byte trial

The new optional `PICTO_PREPARED_TX` firmware build allocates and configures one
upcoming drawing reply before the next poll, using the stock SDK routines. It
holds that buffer before hardware queue submission. On a matching poll it updates
the MAC sequence and buffer age, then submits through the normal queue/completion
path. Preparing never advances the Rust state machine. After a completed TX it
may prepare the same unacknowledged fragment for retry; a positive ACK replaces
that preparation with the next fragment. An unused buffer is recycled by the SDK.
Only the Wi-Fi task may touch the cache, and nonmatching buffers cannot transmit
during preparation. The option defaults OFF and requires inline TX.

The initial prepared 80-byte trial delivered three full drawings in
2.791–3.014 seconds. The first prepared 160-byte trial still failed at fragment
one, and exposed the retry-preparation gap described above.

On September 12 (UTC), the corrected build with **120-byte fragments delivered
nine consecutive full-height test drawings in 1.842–1.887 seconds**. Upload took
358–372 ms; the DS echoed all 86 fragments of every drawing and acknowledged the
final control. The user confirmed the border/diagonal drawings appeared correctly.
There were no session interruptions. The prior verified 80-byte run took
2.768–2.871 seconds: this configuration reduces elapsed time by about one-third.
The sparse test pattern is full height; dense images can require longer USB uploads.

The Pi capture recorded 17,696 packets with zero kernel drops. Captured full
120-byte fragment replies started a median 181 µs after the poll, so this result
does not establish Pi-like radio turnaround. The main established reduction is
86 fragments instead of 129; this run does not isolate the benefit of preparation.
The 45-second capture covers part of the nine-transfer run; the saved server log
covers all nine. See [verified results](../captures/captures.tar.gz)
and [earlier experiments](../captures/captures.tar.gz).

The tested firmware is saved as `backups/verified-rust-prepared120.bin` under the
firmware directory, SHA-256 `b5de0db3c5b8f539e0c2d31c251ed279b90bb16b4ecddf305d38adee78bba9a0`.
That configuration was subsequently superseded by the faster UART build below.
Build with `-DPICTO_PREPARED_TX=ON`, then start the server
with `PICTO_ESP32_FRAGMENT_BYTES=120` to reproduce this configuration. Defaults
remain prepared TX OFF / 80 bytes for compatibility with older firmware.
Use `just benchmark` for repeat delivery measurements (stop the server first). The firmware
protocol tests cover 80/120/160-byte previews, retries and exact echoes;
prepared-buffer interception and instruction-RAM layout are checked by
`firmware/esp32/verify_radio_layout.py` during the firmware build.

### USB latency investigation: 2026-09-12

The interval from upload completion to the server's delivery event includes
the ESP32-to-host USB queue and 250-ms status reporting. Earlier explanations
that called this entire interval radio time were incorrect. Captured first-to-last
fragment spans were about 0.53–0.60 s at 115200 baud and 0.53–0.61 s at 460800,
even though the server's post-upload interval at 115200 was about 1.51 s.
These spans exclude announcement/final-ACK time and require captured endpoints;
missing intermediate packets are reported explicitly by
the archived `.transfers.json` sidecars (the retired capture-analysis script is no longer required).

| Serial rate | Line drawing upload / delivered | Dense checkerboard upload / delivered |
| --- | --- | --- |
| 115200 | 365–375 ms / 1.868–1.882 s (3) | 1532–1561 ms / 3.035–3.074 s (3) |
| 460800 | 117–319 ms / 0.796–1.002 s (3) | 440–528 ms / 1.045–1.298 s (6) |
| 921600 | 67–72 ms / 0.751–0.902 s (3) | 271–299 ms / 1.045–1.051 s (6) |

A further six dense drawings after reconnecting at 921600 all delivered, with
265–527 ms uploads and 1.046–1.653 s elapsed time. There were no USB receive-gap
warnings or disconnects, but one BEGIN command retried. Whether that retry was a
busy response or lost response was not isolated. These are sequential hardware
trials, not a guarantee of constant latency; the full results retain all outliers.

Prepared-reply samples locate approximately 49 µs in SDK enqueueing and 18 µs in
queue draining, with submission reached around 32 µs after callback entry. Radio
turnaround stayed around 182–183 µs in the captures. The faster serial rate helps
both directions of USB traffic; it does not solve the 160-byte radio deadline.
No 160-byte attempt was made during this investigation.

The running firmware is `backups/verified-prepared120-baud921600.bin`, SHA-256
`5734a488b0309c17da547ff7ef22678b32a4424a5cd633d0cc2761bd511aa73c`.
Use `-DPICTO_PREPARED_TX=ON -DPICTO_UART_BAUD=921600` when building and
`PICTO_ESP32_BAUD=921600 PICTO_ESP32_FRAGMENT_BYTES=120` when running the host.
Fresh compatibility defaults remain unchanged. Pi wlan1 was restored to managed/up.
See [all timing evidence](../captures/captures.tar.gz).
