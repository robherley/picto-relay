use std::{
    fs::File,
    io::{BufReader, BufWriter, Read, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};

const PCAP_SNAPLEN: u32 = 65_535;
const LINKTYPE_IEEE802_11_RADIOTAP: u32 = 127;
const PCAP_GLOBAL_HEADER_BYTES: usize = 24;
const PCAP_PACKET_HEADER_BYTES: usize = 16;

#[derive(Clone, Copy)]
enum ByteOrder {
    Little,
    Big,
}

pub(crate) struct PcapPacket {
    pub(crate) timestamp_seconds: f64,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) struct PcapReader {
    reader: BufReader<File>,
    byte_order: ByteOrder,
    timestamp_divisor: f64,
    snaplen: u32,
}

impl PcapReader {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)
            .with_context(|| format!("failed to open capture {}", path.display()))?;
        let mut reader = BufReader::new(file);
        let mut header = [0; PCAP_GLOBAL_HEADER_BYTES];
        reader
            .read_exact(&mut header)
            .with_context(|| format!("failed to read PCAP header from {}", path.display()))?;
        let (byte_order, timestamp_divisor) = match &header[..4] {
            [0xd4, 0xc3, 0xb2, 0xa1] => (ByteOrder::Little, 1_000_000.0),
            [0xa1, 0xb2, 0xc3, 0xd4] => (ByteOrder::Big, 1_000_000.0),
            [0x4d, 0x3c, 0xb2, 0xa1] => (ByteOrder::Little, 1_000_000_000.0),
            [0xa1, 0xb2, 0x3c, 0x4d] => (ByteOrder::Big, 1_000_000_000.0),
            _ => anyhow::bail!("unsupported PCAP magic (PCAPNG is not supported)"),
        };
        let snaplen = read_u32(&header[16..20], byte_order);
        let link_type = read_u32(&header[20..24], byte_order);
        if link_type != LINKTYPE_IEEE802_11_RADIOTAP {
            anyhow::bail!("PCAP link type {link_type} is not radiotap (127)");
        }
        Ok(Self {
            reader,
            byte_order,
            timestamp_divisor,
            snaplen,
        })
    }

    pub(crate) fn read_packet(&mut self) -> Result<Option<PcapPacket>> {
        let mut header = [0; PCAP_PACKET_HEADER_BYTES];
        let read = self.reader.read(&mut header[..1])?;
        if read == 0 {
            return Ok(None);
        }
        self.reader.read_exact(&mut header[1..])?;
        let seconds = read_u32(&header[..4], self.byte_order);
        let fraction = read_u32(&header[4..8], self.byte_order);
        let included_length = read_u32(&header[8..12], self.byte_order);
        if included_length > self.snaplen {
            anyhow::bail!(
                "packet length {included_length} exceeds PCAP snaplen {}",
                self.snaplen
            );
        }
        let mut bytes = vec![0; usize::try_from(included_length)?];
        self.reader.read_exact(&mut bytes)?;
        Ok(Some(PcapPacket {
            timestamp_seconds: f64::from(seconds) + f64::from(fraction) / self.timestamp_divisor,
            bytes,
        }))
    }
}

fn read_u32(bytes: &[u8], byte_order: ByteOrder) -> u32 {
    let bytes = [bytes[0], bytes[1], bytes[2], bytes[3]];
    match byte_order {
        ByteOrder::Little => u32::from_le_bytes(bytes),
        ByteOrder::Big => u32::from_be_bytes(bytes),
    }
}

pub(crate) struct PcapWriter {
    writer: BufWriter<File>,
    packets: u64,
    bytes: u64,
}

impl PcapWriter {
    pub(crate) fn create(path: &Path) -> Result<Self> {
        let file = File::create(path)
            .with_context(|| format!("failed to create capture {}", path.display()))?;
        let mut writer = BufWriter::new(file);
        writer.write_all(&0xa1b2_c3d4_u32.to_le_bytes())?;
        writer.write_all(&2_u16.to_le_bytes())?;
        writer.write_all(&4_u16.to_le_bytes())?;
        writer.write_all(&0_i32.to_le_bytes())?;
        writer.write_all(&0_u32.to_le_bytes())?;
        writer.write_all(&PCAP_SNAPLEN.to_le_bytes())?;
        writer.write_all(&LINKTYPE_IEEE802_11_RADIOTAP.to_le_bytes())?;
        Ok(Self {
            writer,
            packets: 0,
            bytes: 0,
        })
    }

    pub(crate) fn write_packet(&mut self, captured_at: SystemTime, packet: &[u8]) -> Result<()> {
        let since_epoch = captured_at
            .duration_since(UNIX_EPOCH)
            .context("packet timestamp is before the Unix epoch")?;
        let seconds = u32::try_from(since_epoch.as_secs())
            .context("packet timestamp exceeds classic PCAP range")?;
        let length = u32::try_from(packet.len()).context("packet is too large for PCAP")?;
        self.writer.write_all(&seconds.to_le_bytes())?;
        self.writer
            .write_all(&since_epoch.subsec_micros().to_le_bytes())?;
        self.writer.write_all(&length.to_le_bytes())?;
        self.writer.write_all(&length.to_le_bytes())?;
        self.writer.write_all(packet)?;
        self.packets += 1;
        self.bytes += u64::from(length);
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<(u64, u64)> {
        self.writer.flush()?;
        Ok((self.packets, self.bytes))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn writer_should_create_classic_radiotap_pcap() {
        let path = std::env::temp_dir().join(format!(
            "picto-relay-listener-{}-{}.pcap",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("test clock should be valid")
                .as_nanos()
        ));
        let mut writer = PcapWriter::create(&path).expect("fixture should open");
        writer
            .write_packet(UNIX_EPOCH + std::time::Duration::from_secs(1), &[1, 2, 3])
            .expect("fixture packet should write");
        writer.finish().expect("fixture should flush");
        let bytes = fs::read(&path).expect("fixture should read");
        assert_eq!(&bytes[..4], &[0xd4, 0xc3, 0xb2, 0xa1]);
        assert_eq!(
            u32::from_le_bytes(bytes[20..24].try_into().expect("link type")),
            127
        );

        let mut reader = PcapReader::open(&path).expect("fixture should reopen");
        let packet = reader
            .read_packet()
            .expect("fixture packet should read")
            .expect("fixture should contain one packet");
        assert!((packet.timestamp_seconds - 1.0).abs() < f64::EPSILON);
        assert_eq!(packet.bytes, [1, 2, 3]);
        assert!(reader.read_packet().expect("EOF should read").is_none());
        fs::remove_file(path).expect("fixture should clean up");
    }
}
