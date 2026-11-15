> Historical comparison. AR9271 patch files and the Pi backend were removed in the ESP32-only cleanup; the measurements remain evidence.

# Pi/AR9271 firmware comparison

Compared against `../patches/0001-picto-relay-slot-reply.patch` and
`0002-picto-relay-host-round.patch` after the 2026-09-09 Rust retry stalled at
profile phase 3. These patches run on the AR9271 radio, not the Pi CPU.

| Behavior | AR9271 client patch | Current ESP32 Rust client |
| --- | --- | --- |
| Queue scheduling | Sets CWMIN/CWMAX/AIFS to zero and ignores backoff/virtual carrier sense on the dedicated reply queue | Nintendo-only no-ordinary-ACK metadata and zero AIFS/backoff via the pinned HAL; no dedicated queue remapping |
| Packet submission | Prepared DMA descriptors, one `picto_inflight_buf` | Public raw-TX API with inline draining of Nintendo replies through the existing HMAC and low-MAC queues; one correlated frame in flight |
| TX completion | Actual descriptor completion releases in-flight state and arms drawing ACK tracking | Completion callback copies sequence/status into an atomic mailbox; successful completion and a matching host result are both required |
| Drawing retries | Retains the same logical buffer until host acceptance; never replaces an unresolved drawing with idle | Message and retained profile body advance only after successful TX completion and matched host result |
| Profile progression | Transitions when selecting an available template, then waits for semantic host messages/echoes | Commits proposed profile phase on matched MAC result; this is a behavioral change, not a literal translation |
| Delivery reporting | Cumulative accepted cursor plus desktop matching announcement echo | Acknowledged completion plus matching announcement and all fragment echoes |

The v7 implementation retained unresolved profile bodies across changed poll payloads. It
also serialized driver submissions and retained an early host ACK until
successful TX completion. Its invalidation of pending replies on a later poll
was incorrect; the v11 fix is described below.
Failed TX leaves the logical reply available for retry. A missing completion
stops the session after 100 ms and prevents restart until the outstanding buffer
completes (or the device is reset). Sequence numbers persist across sessions.
The callback mailbox is drained before and after engine calls; the UART wait is
1 ms to bound the fallback wakeup delay. No callback-owned pointers are retained.

Eighteen host tests pass, including the frozen C byte-parity test and explicit
completion-ordering, failure, timeout, stale-event and management-transition
cases. Strict Clippy and the ESP-IDF firmware build pass. Hardware outcomes below
remain separate from these software checks.

### Initial queue-control investigation (before 2026-09-11)

The pinned ESP32 `libpp.a` contains `ic_set_ac_param`, `lmacSetAcParam`,
`lmacTxFrame`, and `hal_mac_tx_config_edca`. Disassembly shows `lmacTxFrame`
calling `hal_random`, masking its result according to the current contention
window, storing the selected backoff, and calling `hal_mac_tx_config_edca`
before enabling the hardware queue. A one-time register write would therefore
be overwritten by normal submissions.

The private `esp_wifi_improve_contention_ability` symbol sets a global flag;
its WME update path configures access category 2 with nonzero contention
parameters. It is not evidence of zero-backoff support for our raw reply path.
Neither that symbol nor these queue setters has a verified declaration in the
installed public ESP32 headers. No speculative queue/register patch is enabled.
A dedicated-queue mapping and the private ABI must be established before an
ESP32 equivalent of the AR9271 scheduling patch can be claimed.

ESP-IDF 5.5.2's `esp_80211_tx_info_t` supplies the MAC header and status, but no
hardware TX timestamp. Completion callback time must not be used as RF time.
The AR9271 register values cannot be copied onto ESP32 hardware.

The host-mode patch schedules its ACK using hardware TX timestamp plus poll
airtime, client window and 26 us. It is relevant for eventual ESP32 hosting,
not a drop-in client fix.

Capture caution: the AR9271 client patch may overwrite the captured poll's
sequence/duration fields with diagnostics when armed. Those fields must not be
assumed to be untouched on-air sequence numbers without checking firmware state.
Hardware TSF timestamps and frame lengths are the basis of the latency comparison.

Earlier v4 retry: connected at transport level, stalled at profile phase 3, no HELLO
transmission. The Pi captured 18,520 packets with zero kernel drops. The captured
100-byte profile replies had median apparent post-poll delay around 468 us;
callback preparation was about 13 us late in the run. This points toward radio
queue behavior as an important next investigation, not Rust compute speed alone.

### Lifecycle hardware checks (v6/v7)

Both builds were flashed and verified by esptool. v6 authenticated but its first
association request appeared approximately 7 ms after the host authentication
response; no association response was captured. v7 drains callbacks again on
leaving the engine guard and reduces the UART-loop wait from 10 ms to 1 ms.
It joined with AID 1, observed 1,080 targeted polls, submitted 1,056 replies,
and processed 129 matching negative host results with successful local TX
completion. Profile stayed at phase 0; the DS disconnected it with reason 1.
No HELLO fragments or delivery were reported. These runs validate that the
completion path operates on hardware, not that it fixes the radio timing gap.

Flashed v7 application SHA-256:
`ffe84b6083ff5cc5bb2278e14d05639c344fb3e88c9a97e0358bf6c8bb57605e`.

The v7 Pi capture contains 2,422 packets with zero kernel drops. Among 834
captured 108-byte replies with a preceding poll, the apparent post-poll delay
had median 436 us (monitor-derived estimate, not callback timing). The dedicated
Pi adapter was restored to managed mode after capture.


### 2026-09-10: capture-backed late-reply bug and v11 fix

A same-device C comparison displayed HELLO (confirmed by the user). Rust v8
also displayed HELLO, but stock-queue v7/v9 runs repeatedly stayed at phase 0.
The diagnostic trace and CRC-recovered capture explain a concrete failing case:

| Event | ESP32 time, microseconds | Sequence/result |
| --- | ---: | --- |
| Host poll | 3,610,456 | 4088 |
| Reply submitted | 3,610,489 | client TX 175 |
| Host result | 3,611,707 | ACK 4089, missed |
| Next observed poll | 3,618,364 | 4091 |
| Driver completion | 3,619,070 | TX 175, success |
| Host result | 3,619,626 | ACK 4092, accepted |

The PCAP confirms TX 175 arrived during poll 4091's reply window, about 40 us
after that poll ended. The driver had deferred it for approximately 8 ms.
The old Rust code discarded its proposed phase on the later poll, so it ignored
a real positive ACK and repeatedly retransmitted the session body. This is a
state-machine error exposed by radio queue latency, not proof Rust is too slow.

v11 retains that single driver buffer and rebinds its unresolved result to the
latest eligible single-slot poll. An earlier negative ACK can be superseded
while the frame is still in flight; an earlier positive ACK is preserved.
Multi-slot polls are excluded. Successful completion and a matching positive
host result are still both required. A regression test replays these exact
poll/ACK timestamps, with additional ordering and sequence-wrap coverage.

Two other incorrect assumptions were fixed: broadcast cleanup deauths before
membership no longer abort authentication/association; and delivery no longer
requires an echoed completion control. Successful real-DS captures show the
announcement and drawing echoed, with completion acknowledged but not echoed.
`delivered=1` requires that announcement, all 27 exact fragment echoes, and
acknowledged completion. It remains protocol evidence, not a screen observation.

The queue-4 experiment used the pinned driver's existing AIFS=1/zero-CW queue.
Its first data buffer never completed, tripping the 100 ms fail-closed guard.
The experiment was removed; v11 uses the stock raw-TX queue. The extra libpp.a
SHA-256 build guard remains because existing metadata hooks depend on that ABI.

`scripts/analyze-esp32-rounds.py` recovers overwritten poll sequence fields only
when a candidate reproduces the saved FCS. All 327 modified poll sequences in
the v9 comparison were recoverable this way. Original captures are not changed.


### Final v11 hardware result

Flashed application SHA-256:
`ea020be812d32f1f5af0737dcfb0040989157d552dffe8bdf14d0de34c149878`.
The user confirmed the newest HELLO was visible. The client stayed connected
through the full 60-second test, ending normally with phase=12, message=29,
echoed=27, delivered=1, 2,159 targeted polls and 2,125 submitted replies.
The independent Pi capture contains 4,708 packets and zero kernel drops; its
host echoes reassemble all 2,084 drawing bytes (SHA-256
`eaa875bd32e310e0fce1261b65a3ee0935319aad51010a1814ed31945acfc6de`).
One client-side fragment was absent from the monitor capture, but every fragment
was present in the DS echo. The capture adapter was restored to managed mode.

Evidence: `captures/esp32/20260910-v11.log`,
`captures/pi/20260910-v11.analysis.json`, and
`captures/pi/20260910-v11.rounds.json` (capture paths relative to repo root).
This is validation of the bounded HELLO experiment, not a persistent bridge
or a guarantee of deterministic ESP32 Wi-Fi scheduling.


### 2026-09-11: measured reply path and private-object IRAM fix

The private output object was linked directly, while ESP-IDF's optimized linker
mappings match `libnet80211.a` and `libpp.a` archive names. Its `.wifi*iram` sections
therefore fell through the default flash mapping. `prepare_radio.py` now renames
these sections to explicit instruction-RAM sections in every extracted object.
It verifies unchanged executable bytes and exact stock archive hashes. A post-link
check rejects any of 19 required routines outside ESP32 instruction RAM.
Performance optimization and extra Wi-Fi IRAM are enabled in `sdkconfig.defaults`.

Two narrowly scoped driver hooks are on by default for the Rust build:

- `PICTO_REPLY_EDCA`: identify a Nintendo reply using its existing ebuf metadata
  and frame, pass a local copy of queue configuration with AIFS/backoff zero to
  `hal_mac_tx_config_edca`. Hardware queue identity and persistent retry state
  are unchanged. The pinned HAL reads only queue bytes 4–7.
- `PICTO_INLINE_TX`: after public raw submission releases the SDK lock, drain
  `ieee80211_output_process`, then the selected ordinary queue through
  `ppProcessTxQ`. Only the Wi-Fi receive callback does this. Queue 4 is excluded;
  stock buffer allocation, DMA ownership, idle checks and completion stay intact.

The SDK still handles channel access; this does not reproduce every AR9271 queue
policy or establish deterministic timing. The host harness checks Nintendo-only
scope, ordinary-frame passthrough and queue bounds under ASan/UBSan. The SDK
archives remain unchanged; no instruction bytes or direct register values are
patched. The reference C build is an historical protocol comparison, not a frozen
whole-image performance comparison after these linker changes.

Captured median 80-byte-fragment post-poll delay fell from 398 us (queue-only)
to 252 us (raw functions in RAM), then 135 us with corrected mappings. Full-height
latency did not improve monotonically: observed poll cadence also changed. The
larger 160-byte trial still failed at its first fragment even after the mapping
fix; USB capability negotiation is not evidence of radio deadline compliance.
80-byte fragments remain the default. Evidence and per-run timings are in
`captures/esp32/20260911-radio-timing.json`; packet analyses are under
`captures/pi/20260911-*.rounds.json` in the repository root.


Final default-configuration check: six full-height transfers delivered in
2.768–2.871 seconds, with 129 exact fragment echoes per drawing and final ACK.
The DS room was reset before this run. The final capture had 5,191 packets and
zero kernel drops; wlan1 was restored to managed mode afterwards. Firmware SHA:
`cd5e61be158c9aba8b4605956c20673ab4d20921a775324b552acd6f5b00e78c`.
The USB bridge remains connected on the laptop. No visual screen confirmation
was requested for these synthetic drawings; delivery is based on DS echoes.


### Further preparation experiment and 120-byte validation

Moved six packet-setup routines, three queue helpers and the platform timer into
RAM. Packet-setup-only 80-byte timings were 2.695, 2.729 and 2.803 seconds. This
alone did not close the Pi gap. The optional prepared-buffer path configures one
reply before a poll using SDK allocation and raw processing, retains it before
`ppTxPkt`, and submits it only for a matching Rust-selected reply. Stock recycling,
queue submission and TX completion retain their normal ownership boundaries.

Initial prepared 80-byte timings were 3.014, 2.803 and 2.791 seconds. A 160-byte
trial still failed at the first fragment. Its first two submissions used prepared
buffers, but unanswered retries fell back to the ordinary path. That preparation
condition was fixed without advancing unacknowledged protocol state. A 120-byte
option reduces full-height fragment count to 86. On September 12 (UTC), the corrected
prepared build delivered nine consecutive drawings in 1.842–1.887 seconds, with
all 86 exact fragment echoes per drawing and final ACK. The user confirmed correct
DS display. This is about one-third below the previous 80-byte elapsed times.

Captured full-fragment post-poll delay was still a median 181 us. This run validates
the combined configuration; it does not isolate the effect of preparation from
fragment size. The sparse full-height test also does not characterize worst-case
USB upload for dense drawings. The prepared 120-byte configuration remains running;
the Pi monitor adapter was restored. Firmware hash and timings are in
`captures/esp32/20260912-prepared120.json`. Compatibility defaults remain 80 bytes and
prepared TX OFF.
Evidence: `captures/esp32/20260911-prepared-timing.json` and
`captures/pi/20260911-{ppdu80,prepared80,prepared160}.rounds.json`.


### 2026-09-12: USB return-path delay

The post-upload server interval includes USB output queueing and periodic status
reporting, not only radio transmission. Captured first-to-last fragment spans
were about 0.53–0.60 s at 115200 baud and 0.53–0.61 s at 460800. The earlier
approximate 1.5 s post-upload interval must not be attributed entirely to the radio.
Prepared SDK enqueueing sampled around 49 us, queue draining around 18 us, and
full-fragment RF turnaround remained around 182–183 us. No 160-byte trial was made.

The bridge now runs at 921600 baud with 120-byte fragments. Three line drawings
delivered in 0.751–0.902 s. Twelve dense drawings across two sessions delivered in
1.045–1.653 s, versus 3.035–3.074 s at 115200. The final six-transfer session had
no receive-gap warnings or disconnects, and one retried BEGIN command. See
`captures/esp32/20260912-uart-latency.json` for all measurements, firmware hashes and
limitations. Pi wlan1 is restored to managed mode.
