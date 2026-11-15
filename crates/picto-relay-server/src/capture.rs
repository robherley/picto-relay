//! Writes radio frames reported by the firmware to a pcap file for Wireshark.
use anyhow::{Context, Result};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

/// pcap magic for little-endian files with microsecond timestamps.
const PCAP_MAGIC: u32 = 0xa1b2_c3d4;
/// pcap format version written.
const PCAP_VERSION: (u16, u16) = (2, 4);
/// Largest frame length a reader should expect.
const PCAP_SNAP_LENGTH: u32 = 65_535;
/// pcap link type for IEEE 802.11 frames without a radiotap header or FCS.
const LINKTYPE_IEEE802_11: u32 = 105;
/// Microseconds in one second.
const MICROSECONDS: u64 = 1_000_000;
/// The firmware's 32-bit microsecond clock wraps after this many microseconds.
const RADIO_CLOCK_SPAN: u64 = 1 << u32::BITS;

/// An open capture file. Frames carry the radio's own clock, so the spacing
/// between frames is exact; the first frame is pinned to the computer's time.
pub(crate) struct Capture {
    file: File,
    /// Wall-clock microseconds corresponding to radio time zero.
    origin: Option<u64>,
    /// Radio clock extended beyond 32 bits, as of the latest frame.
    latest: u64,
}

impl Capture {
    /// Creates the file, or appends to a capture this program wrote earlier.
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("open capture file {}", path.display()))?;
        if file.metadata()?.len() == 0 {
            let mut header = Vec::new();
            header.extend_from_slice(&PCAP_MAGIC.to_le_bytes());
            header.extend_from_slice(&PCAP_VERSION.0.to_le_bytes());
            header.extend_from_slice(&PCAP_VERSION.1.to_le_bytes());
            header.extend_from_slice(&0u32.to_le_bytes()); // Timestamps are UTC.
            header.extend_from_slice(&0u32.to_le_bytes()); // Timestamp accuracy: unspecified.
            header.extend_from_slice(&PCAP_SNAP_LENGTH.to_le_bytes());
            header.extend_from_slice(&LINKTYPE_IEEE802_11.to_le_bytes());
            file.write_all(&header)?;
        }
        Ok(Self {
            file,
            origin: None,
            latest: 0,
        })
    }

    /// Appends one frame. `radio_time` is the firmware's microsecond clock,
    /// `air_length` the frame's length on the air, `frame` the captured bytes.
    pub(crate) fn write(&mut self, radio_time: u32, air_length: u16, frame: &[u8]) -> Result<()> {
        let record = self.record(radio_time, air_length, frame, wall_clock_microseconds());
        self.file.write_all(&record)?;
        Ok(())
    }

    fn record(&mut self, radio_time: u32, air_length: u16, frame: &[u8], now: u64) -> Vec<u8> {
        // Frames arrive slightly out of order (sent and received frames are
        // stamped at different points), so only a large step back is a wrap.
        // Place the 32-bit reading in the wrap period nearest the latest frame.
        let mut time = (self.latest & !(RADIO_CLOCK_SPAN - 1)) | u64::from(radio_time);
        if time + RADIO_CLOCK_SPAN / 2 < self.latest {
            time += RADIO_CLOCK_SPAN;
        } else if time > self.latest + RADIO_CLOCK_SPAN / 2 && time >= RADIO_CLOCK_SPAN {
            time -= RADIO_CLOCK_SPAN;
        }
        self.latest = self.latest.max(time);
        let origin = *self.origin.get_or_insert(now.saturating_sub(time));
        let timestamp = origin + time;

        let captured = u32::try_from(frame.len()).unwrap_or(u32::MAX);
        let mut record = Vec::with_capacity(frame.len() + 4 * size_of::<u32>());
        let seconds = u32::try_from(timestamp / MICROSECONDS).unwrap_or(u32::MAX);
        let microseconds = u32::try_from(timestamp % MICROSECONDS).unwrap_or(0);
        record.extend_from_slice(&seconds.to_le_bytes());
        record.extend_from_slice(&microseconds.to_le_bytes());
        record.extend_from_slice(&captured.to_le_bytes());
        record.extend_from_slice(&u32::from(air_length).max(captured).to_le_bytes());
        record.extend_from_slice(frame);
        record
    }
}

fn wall_clock_microseconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(record: &[u8], index: usize) -> u32 {
        u32::from_le_bytes(record[index * 4..index * 4 + 4].try_into().unwrap())
    }

    #[test]
    fn capture_file_has_a_pcap_header_and_keeps_radio_spacing_across_clock_wrap() {
        let path = std::env::temp_dir().join(format!("picto-capture-{}.pcap", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut capture = Capture::open(&path).unwrap();
        let wall = 1_700_000_000 * MICROSECONDS;
        let first = capture.record(u32::MAX - 500, 300, &[1, 2, 3], wall);
        let second = capture.record(499, 3, &[4, 5, 6], wall + 99);
        let earlier = capture.record(u32::MAX - 100, 3, &[7], wall + 99);
        // The first frame is pinned to wall-clock time; later ones keep radio spacing.
        assert_eq!(field(&first, 0), 1_700_000_000);
        assert_eq!(field(&first, 1), 0);
        assert_eq!(field(&second, 1), 1000);
        assert_eq!(field(&earlier, 1), 400);
        assert_eq!((field(&first, 2), field(&first, 3)), (3, 300));
        assert_eq!(&first[16..], [1, 2, 3]);

        capture.write(1000, 3, &[9, 9, 9]).unwrap();
        drop(capture);
        drop(Capture::open(&path).unwrap()); // Reopening appends; no second header.
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(field(&bytes, 0), PCAP_MAGIC);
        assert_eq!(field(&bytes, 5), LINKTYPE_IEEE802_11);
        assert_eq!(bytes.len(), 24 + 16 + 3);
    }
}
