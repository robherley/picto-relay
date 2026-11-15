# Capture archive

All 231 capture files, sidecars, serial logs, and evidence summaries are bundled
in [captures.tar.gz](captures.tar.gz). Every archived file was SHA-256 verified
against its original before the loose directories were removed.

Extract from the repository root:

```sh
tar -xzf captures/captures.tar.gz -C captures
```

This restores `captures/pi/` and `captures/esp32/`, including their original
README indexes. Paths in protocol documentation refer to this extracted layout;
links to archived evidence download the complete tarball.

Packet captures inside the tarball retain their existing `.pcap.gz` compression.
To inspect one with Wireshark, decompress a copy:

```sh
gzip -dc captures/pi/01-two-ds-baseline.pcap.gz > /tmp/two-ds.pcap
```

Historical capture hashes describe the decompressed packet bytes. The runtime
and tests use their own fixtures and do not require extracting this archive.

## Rooms with several clients

Two captures taken by the ESP32 itself (`just server-capture`) on 2026-11-15, of
a host DS, a second DS and the relay. They are the evidence for
[Rooms with several clients](../docs/pictochat-protocol.md#rooms-with-several-clients).
Frame order is reliable in these files; the timestamps are not.

| File | What it shows |
|---|---|
| `multi-client/20261115-relay-joins-room-with-ds-client.pcap.gz` | The relay joining a room that already holds a DS client, and that DS leaving and rejoining: how a real DS answers the roster, introduces itself, and how members answer a newcomer. Taken with the older scripted firmware, whose join stalls for 28 s. |
| `multi-client/20261115-ds-joins-after-relay-short-window-drop.pcap.gz` | A DS joining behind the relay, then the host's short-window poll repeated about 3,400 times until it drops the relay. |

```sh
gzip -dc captures/multi-client/20261115-relay-joins-room-with-ds-client.pcap.gz > /tmp/join.pcap
```
