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
