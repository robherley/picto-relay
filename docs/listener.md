# Live PictoChat listener

`picto-relay-listen` captures directly from the AR9271 monitor interface. It replaces
the manual `tcpdump` step while preserving a standard PCAP for Wireshark and later
analysis.

Each run creates:

- `<timestamp>-<scenario>.pcap`: complete radiotap capture;
- `<timestamp>-<scenario>.json`: scenario labels, notes, timing, capture totals, and
  decoded packet-family counters.

## Deploy from the Mac

The Pi does not need a Rust toolchain. Cross-compile and copy the binary with:

```sh
./scripts/deploy-listener.sh
```

The default destination is `/home/rob/.local/bin/picto-relay-listen` on
`rob@blueberry.lab.reb.gg`.

## Record a scenario

The AR9271 currently appears as `wlan1`. This command changes that dedicated adapter
to monitor mode, selects channel 1, listens for 30 seconds, and records a labeled run:

```sh
sudo /home/rob/.local/bin/picto-relay-listen \
  --prepare \
  --interface wlan1 \
  --channel 1 \
  --duration 30 \
  --scenario "client one pixel" \
  --tag sender=client \
  --tag drawing=one-pixel \
  --tag coordinate=128,24 \
  --notes "Black pen; send exactly once"
```

Omit `--duration` to run until Ctrl-C. Omit `--prepare` when the interface is already
in monitor mode on the correct channel.

The normal display suppresses repeated idle polls and successful acknowledgements.
It reports:

- semantic room/user-count changes; opaque beacon rotations appear only with `--verbose`;
- authentication, association, participant slots, and leaves;
- the first occurrence of every application packet family;
- every message fragment, including opaque transfer tag, offset, size, frame number,
  and final marker;
- decoded 4bpp drawing dimensions, sender MAC, ink bounds, and used palette indices;
- the first occurrence of each failed-client acknowledgement mask, with repeats
  retained in the summary (`--verbose` prints every occurrence);
- final file paths and aggregate counters.

Use `--verbose` to print every valid application envelope (including raw application
payload hex), successful host acknowledgement, and the 36-byte metadata prefix for
completed drawings. A message run looks approximately like:

```text
   0.000s  ROOM        host 00:24:f3:67:41:87 · room A · 1 user(s)
   0.052s  JOIN        00:16:56:4d:27:ca → 00:24:f3:67:41:87 · AID 0xc001 · slot 1
   7.619s  PACKET      client → host · id 0x8e56 · declared 172 · payload 170 B
   7.619s  FRAGMENT    client → host · tag 0x0000 · offset 0x0000 · 160 B · frame 3
   7.714s  FRAGMENT    client → host · offset 0x0820 · 4 B · frame 16 · FINAL
   7.714s  MESSAGE     client → host · sender 00:16:56:4d:27:ca · tag 0x0000 · 256x16 4bpp · ink x=128..128, y=24..24 · palette [0,1] · 2084 B
complete  42556 packets · 1 completed message(s)
```

## Replay a pulled capture locally

Offline replay uses the same radiotap, 802.11, transport, reassembly, and drawing
decoder as live capture. It does not require an interface, root access, or a scenario
name:

```sh
cargo run -p picto-relay-listener -- \
  --read captures/blueberry/1786660066567-joined-client-bitmap-validation.pcap
```

Classic microsecond and nanosecond PCAP files in either byte order are accepted when
their link type is radiotap (`127`). PCAPNG is not yet supported. Add `--verbose` to
inspect every envelope, acknowledgement, and repeated leave frame.

## Run without sudo

`--prepare` invokes `ip` and `iw`, so the one-command workflow should run under `sudo`.
To run the capture itself without sudo, prepare the dedicated adapter once and grant
only raw-socket access to the listener:

```sh
sudo /usr/sbin/ip link set dev wlan1 down
sudo /usr/sbin/iw dev wlan1 set type monitor
sudo /usr/sbin/ip link set dev wlan1 up
sudo /usr/sbin/iw dev wlan1 set channel 1
sudo /usr/sbin/setcap cap_net_raw=eip /home/rob/.local/bin/picto-relay-listen

/home/rob/.local/bin/picto-relay-listen \
  --interface wlan1 \
  --scenario "client one pixel" \
  --tag sender=client
```

File capabilities are attached to that copy of the binary, so run `setcap` again after
each deployment. The adapter remains in monitor mode after capture; use `--prepare` on
the next run or change it back to managed mode explicitly if needed.

## Pull captures and metadata

```sh
./scripts/pull-pcaps.sh
```

The pull script transfers `.pcap`, `.pcapng`, and matching `.json` manifests into
`captures/blueberry/`.

## Suggested scenario tags

Keep one intentional change per run. Useful keys include:

| Tag | Examples |
|---|---|
| `sender` | `host`, `client` |
| `action` | `join`, `leave`, `message` |
| `drawing` | `empty`, `one-pixel`, `horizontal-line` |
| `coordinate` | `0,0`, `128,24`, `255,47` |
| `color` | `black`, `red`, `green` |
| `nickname` | Exact nickname used for the run |
| `repeat` | `1`, `2`, `3` for otherwise identical captures |

The JSON manifest keeps these tags beside decoded counters, preventing capture files
from becoming detached from the experiment that produced them.
