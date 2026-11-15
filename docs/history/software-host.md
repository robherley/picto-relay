> Historical investigation of the retired AR9271/Pi implementation. Commands and runtime claims below are not current. See the [ESP32 setup](../../README.md).

# Firmware-assisted host probe

`picto-relay-host` uses the AR9271 AP beacon engine for discovery and the optional
host-round firmware extension for Nintendo CMD/REPLY/ACK rounds. The JSON gateway
still joins an existing DS host; it does not automatically create a room.

## What is implemented and verified

- Hardware beacons, authentication and association were verified with a real DS in
  the original AP-template trial.
- Host application initialization now advances through the opening roster,
  admission, both host profiles, client announcement echoes, both client profile
  requests/rebroadcasts, completion gates and steady membership. A regression test
  uses the real client's replies from
  `1787193673345-real-ds-clean-join-baseline.pcap.gz`, including its empty reply before
  the final announcement. This is offline evidence, not a new interoperability test.
- Each pending application transaction is stable across lost or failed rounds.
  The roster's opaque prefix retains the captured opening value instead of invented
  per-poll entropy. The meaning of that field is still unresolved.
- The adapter sends a poll at 2 Mbps with short preamble, collects the invited
  station's reply and sends the failed-slot ACK using its TSF clock. Linux never
  schedules or injects the ACK. It receives the original poll's TX-status cookie
  only after the adapter finishes the round.
- Only one slot is invited per round; multiple associated stations are serviced in
  slot order. Multi-client application interoperability is not established. The
  beacon count remains one during this probe.
- Clients with no accepted reply for five seconds expire. Missing firmware results
  and wrong reported PHY rates stop the probe explicitly. Stopping normally waits
  for the current round; firmware queue drain cancels an ACK held outside the queue.

The host-round firmware has local state-machine tests and syntax checks against
upstream `e888634`. A complete Xtensa build and physical DS run are still required
before claiming sustained hosting. The Pi was unreachable during local validation.

## Run

Build and install the firmware containing **both** patches using
[`../firmware/README.md`](../../firmware/README.md). The second patch is required for
hosting; the existing client-only firmware is insufficient. Do not run the host and
JSON gateway against the same adapter simultaneously.

```sh
./scripts/build-firmware.sh
# Install the generated image using the existing firmware installation procedure.
sudo /home/rob/.local/bin/picto-relay-host \
  --prepare --firmware-assist \
  --interface wlan1 --channel 1 --room A --name PictoRelay \
  --seconds 60 \
  --capture /home/rob/picto-relay-captures/firmware-host.pcap.gz
```

The probe prints each initialization phase and reports `ready` only after both
profile passes. Failure to receive a round completion within 100 ms is a transport
watchdog failure, not an on-air deadline. There is no echo-timed software fallback.

## Timing correction

The earlier notes compared PCAP delivery timestamps and treated the advertised
1,248-us Duration value as CMD-start to ACK-start time. Those are different things.
For the first opening roster in the clean two-DS baseline, RX TSF advances by about
1,687 us from poll to ACK; PCAP wall time advances by about 1,246 us.
At 2 Mbps with short preamble, the 142-byte poll including FCS takes 664 us. The
current firmware model schedules ACK at poll send timestamp + poll airtime +
998-us client window + 26 us. The 1,248-us reservation also includes the 224-us
ACK airtime. The interpretation of the adapter's TX timestamp and real scheduling
jitter must be confirmed using a second receiver's TSF.

The previous “100-us receive timeout” was truncated to zero milliseconds by pnet's
Linux `poll` implementation. The diagnostic receiver now explicitly polls
nonblocking and sleeps cooperatively for up to 100 us on empty reads, capped by its
remaining watchdog interval. OS scheduling may overshoot; this is not a radio clock.

## Retained capture corrections

The newer `m7` and `m8` captures contain DS association attempts and no client data
replies. Both have a truncated final record; summaries count only complete records.
The old claim that no real DS had authenticated during low-latency testing is stale.

| Capture | Poll injection requests | Poll TX-status echoes | Client data replies |
|---|---:|---:|---:|
| `20260824-software-host-c34-low-latency-real-ds-m7.pcap.gz` | 8,758 | 8,757 | 0 |
| `20260824-software-host-c34-echo-wait-m8.pcap.gz` | 12,527 | 12,527 | 0 |

Injection requests specify 2 Mbps; TX-status echoes report 1 Mbps. The upstream
multicast path selects its minimum rate. The new host firmware bypasses that path
with an explicit descriptor rate, and the host checks the completion's rate.
Neither old trace establishes that ACK timing alone caused the DS's silence.

Reproduce the summaries:

```sh
python3 scripts/summarize-host-capture.py \
  captures/pi/20260824-software-host-c34-echo-wait-m8.pcap.gz
python3 firmware/tests/test_host_round.py
cargo test -p picto-relay-join host --locked
```

For physical validation, use a second monitor radio to verify that private upload
metadata is absent on air, both CMD and ACK use 2 Mbps/short preamble, failed masks
match replies, and the TSF timing is stable. Then verify the visible nickname,
continued membership and reconnection. Hosting drawings and JSON host-mode
integration remain separate future work.
