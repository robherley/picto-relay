# AR9271 slot-response firmware experiment

The stock `ath9k_htc` path sends injected frames through USB and userspace. A
Nintendo DS host reports the virtual participant as missing before that round trip
finishes. This experiment keeps a nine-template PictoChat identity sequence in the
AR9271 firmware and submits the appropriate reply from the firmware RX interrupt
path as soon as a host command targets the participant slot. An opt-in outbound
sender reserves sixteen more buffers as a circular window over a variable-length
native drawing transfer.

The patch is based on Qualcomm Atheros' open firmware at commit `e888634`. It does
five deliberately narrow things:

1. recognizes client-reply templates with two trailing private bytes: identity
   template ID `0xA0..0xA8`, outbound-message template ID `0xC0..0xCF`, and
   participant slot `0xB0 | slot`;
2. removes those private bytes and retains session, announcement, zero-payload,
   profile, completion, and idle TX buffers after their first transmission;
3. follows the capture-backed opening session plus two-pass `c34`/`d0a`/`e30`
   identity exchange, then requeues the idle template for later polls, always at
   2 Mbps with short preamble;
4. advances an outbound drawing only after the DS host acknowledgement accepts the
   virtual slot, retrying the same reply on a failed-slot bit and refreshing an
   accepted ring slot in place from Rust; and
5. marks the captured host poll's radiotap antenna byte with a `0xd0..0xdf`
   diagnostic value and mirrors its low four bits into the fragment-number nibble
   as a fallback for kernels that omit the antenna metadata. This changes only the
   post-reception copy delivered to the monitor interface, not the frame on air;
   while a drawing is active, the duration field also carries `0xA500 | cursor`,
   where `cursor` is the cumulative number of replies accepted by the DS host.

Template ID `0xAF` is reserved for disarming the responder. The matching Rust probe
sends it before its final deauthentication frame so the firmware stops answering
slot polls immediately when a bounded join run ends. Once any in-flight reply
completes, disarm returns all retained identity and message buffers to the normal HTC
pool. Replacing a template also releases an older retained buffer. This cleanup is
required for repeated API sessions without reloading the adapter firmware.

This is an experimental transport implementation, not production firmware. A live
station retains nine HTC TX buffers for its reusable identity and idle replies until
disarm. Sixteen outbound drawing buffers form a sliding ring. Each successful host
acknowledgement leaves its physical buffer retained but unavailable. Rust reads the
cumulative firmware cursor and uploads the next body for that slot; firmware copies
those bytes into the retained buffer and returns the temporary upload directly to HTC
without sending it through the Wi-Fi TX queue. This permits repeated ring wraps
without consuming additional HTC credits or target TX descriptors. Cumulative
reporting makes dropped monitor copies recoverable: the next host command catches
Linux up by any number of replies. A failed acknowledgement rearms the same buffer
without advancing. While a drawing acknowledgement is unresolved, firmware
retransmits that drawing fragment on a later targeted command instead of substituting
an idle reply whose acknowledgement could falsely advance the cursor.
It supports one physical virtual participant and one in-flight drawing at a time.
Disarming stops replies; unplugging the USB adapter
clears the running firmware because the AR9271 loads firmware into RAM at startup.

Returning retained buffers is mandatory, not merely an optimization. The AR9271
advertises only 33 host-to-target TX credits; leaking nine identity buffers per
session and then retaining sixteen drawing fragments exhausts that pool. The visible
failure sequence is a drawing that stalls followed by authentication frames that are
accepted locally but never transmitted. Firmware builds containing this cleanup
release the buffers after disarm, including when disarm races an in-flight slot reply.

## Build on the Pi

Sync the project, install the two build prerequisites once, and build:

```sh
./scripts/sync-pi.sh
ssh rob@blueberry.lab.reb.gg
sudo apt-get install -y m4 cmake
cd /home/rob/picto-relay
./scripts/build-firmware.sh
```

The output is `/home/rob/picto-relay-firmware/htc_9271-picto-relay.fw`.
The build script recognizes the earlier Picto Relay experiment and replaces only its
two patched upstream source files before applying the current patch.
Do not replace a file under `/lib/firmware` until its actual filename has been
confirmed from the Pi's kernel log. Preserve the stock file before installing the
custom image.

## Client contract

Deploy the matching Rust probe and run it with `--firmware-assist`. Before association,
the probe sends nine marked reply frames and stops injecting per-poll replies. With
`--send-dot`, it additionally preloads an announcement, fourteen native drawing
fragments, and a completion frame before the final idle activation template. Its final
report correlates acknowledgements only with polls targeting the assigned slot and
prints firmware recognized/queued/completed/clean/busy counters plus radiotap-TSF
poll-to-host-ack timing. Unrelated host acknowledgements no longer inflate the result.

```sh
sudo /home/rob/.local/bin/picto-relay-join \
  --prepare \
  --interface wlan1 \
  --channel 1 \
  --room A \
  --firmware-assist \
  --send-dot
```

The transport success criterion is a sustained run where `queued` matches `recognized`,
`completed` and `clean` trail `queued` by at most the final in-flight reply, `busy`
and `missed` remain zero, and air timing is stable.
The application success criterion is the `--name` value appearing in the real DS
participant list after the host has received and rebroadcast the virtual profile.
