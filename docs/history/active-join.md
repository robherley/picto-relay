> Historical investigation of the retired AR9271/Pi implementation. Commands and runtime claims below are not current. See the [ESP32 setup](../../README.md).

# Active PictoChat join probe

`picto-relay-join` is the first active-radio milestone. It uses the AR9271 to discover
a DS-hosted room, inject an open-system authentication request, inject the captured
Nintendo association request, and require both a successful association response and
a subsequent beacon advertising the additional user.

After association it answers slot-directed multiplayer commands for ten seconds.
The firmware-assisted path completes the session and identity exchange before settling
on the captured two-byte idle status, so the virtual participant can acquire a visible
name as well as sustain transport presence.
Physical testing confirmed that the generated `PictoRelay` nickname appears in a real
DS participant list.
Association slot `n` maps directly to poll-mask bit `1 << n`; the first client in
slot 1 is therefore invited by mask `0x0002`.

With `--firmware-assist`, the matching AR9271 firmware preloads nine replies before
association: the opening session record, initial and repeated transfer announcements,
reusable empty response, initial and final 84-byte profiles, both completion records,
and steady-state idle status. It selects them from the capture-backed two-pass
`c34`/`d0a`/`e30` host sequence and sends them at 2 Mbps with short preamble directly
from its RX path. Preloading before association ensures the opening `c34` poll can be
answered immediately. The probe reports polls
recognized by firmware, replies queued, prior replies completed, clean TX descriptors,
busy polls, application initialization progress, and adapter-TSF poll-to-host-ack
timing. Diagnostic markers change capture metadata only; they do not alter the
Nintendo frame or FCS.

`--send-dot` retains the first outbound-message probe. It preloads a capture-shaped
2,084-byte, 256x16 native PictoChat drawing as fourteen fragments, surrounded by the
observed `d0a` announcement and completion packets. The firmware sends those sixteen
frames on consecutive targeted polls after the identity exchange reaches steady state.
Physical testing confirmed that the DS renders this drawing and that its host
rebroadcasts all fourteen fragments. Short final rebroadcasts occupy a padded physical
slot as documented in [pictochat-protocol.md](../pictochat-protocol.md).
The long-lived JSON backend uses the same sixteen template shapes but loads API
drawings after profile initialization. Updated firmware releases each one-shot message
buffer after transmission so another drawing can be loaded without resetting the
adapter.

At the end of the hold interval, the probe first sends a private disarm marker to
stop firmware-timed replies and then sends an 802.11 deauthentication frame to the
room host. This lets the host release the participant slot instead of retaining an
invisible virtual client after the process exits normally.

The 84-byte virtual profile contains the selected station MAC and `--name` (default
`PictoRelay`). Several non-semantic transfer bytes are still capture-derived, but the
participant slot and station identity are generated for the active session. The
capture-derived userspace profile experiment was removed after hardware testing proved
that it deadlocks the session; identity exchange requires the firmware-assisted path.

## Deploy

From the Mac:

```sh
./scripts/deploy-join.sh
```

The default destination is `/home/rob/.local/bin/picto-relay-join` on `pi`.

## Run the first physical test

1. On the physical Nintendo DS, open PictoChat and enter Room A.
2. Leave the DS on the room screen where its participant list is visible.
3. On the Pi, run:

```sh
sudo /home/rob/.local/bin/picto-relay-join \
  --prepare \
  --interface wlan1 \
  --channel 1 \
  --room A \
  --firmware-assist \
  --send-dot
```

The default virtual station is `00:09:bf:52:00:01`. Physical testing showed that a
PictoChat host acknowledges management frames from arbitrary unicast addresses at
the radio layer, but only answers authentication when the station uses Nintendo's
`00:09:bf` OUI. Keep the complete address unique on the channel; this intentionally
spoofs a Nintendo station identity.

Before every association attempt, the probe waits for a fresh beacon from the chosen
host and copies its current stream code into Nintendo's 32-byte binary SSID. Captures
show a real DS doing the same: its association follows the relevant beacon by only a
few milliseconds.

`ath9k_htc` rejects mac80211's `active` monitor flag, so `--prepare` creates two
interfaces on the AR9271. The requested interface (normally `wlan1`) becomes an
unassociated managed interface carrying the virtual station MAC; the derived
`wlan1mon` interface performs capture and injection. The managed interface exists so
the adapter can send hardware-timed ACKs for host responses addressed to the virtual
station. Receiving a successful authentication body is not sufficient if the host
never sees its response acknowledged.

On NetworkManager systems, `--prepare` marks the dedicated AR9271 interface unmanaged
before configuring it. It then locks `wlan1mon` to the requested channel while the
managed ACK interface is still down and brings the ACK interface up last. This avoids
NetworkManager scanning on the adapter and making the channel change fail with
`Device or resource busy`.

Success requires four independently observed steps:

```text
1/4 room beacon discovered
2/4 authentication response status 0
3/4 association response status 0 with participant slot
4/4 host beacon user count remains increased while the identity exchange and slot polls succeed
```

If a management response is missed, the probe retries up to four times with the IEEE
retry bit set. Useful diagnostic overrides include:

```sh
--scan-timeout 120
--response-timeout-ms 1500
--attempts 8
--hold-seconds 20
```

Do not run the live listener with `--prepare` concurrently; both processes would try
to reconfigure the adapter. A separate capture process may listen on an already
prepared monitor interface if evidence from a failed attempt is needed.
