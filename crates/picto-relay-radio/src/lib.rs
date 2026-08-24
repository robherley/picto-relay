//! Blocking raw-radio boundary and Linux monitor backend for Picto Relay.

use std::{
    fs::File,
    io::{self, BufWriter, Write},
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use pnet::datalink::{self, Channel, DataLinkReceiver, DataLinkSender};
use thiserror::Error;

const MONITOR_READ_TIMEOUT: Duration = Duration::from_millis(20);
const MONITOR_BUFFER_SIZE: usize = 4 * 1024 * 1024;
const RADIOTAP_FCS: u8 = 0x10;
const INJECTION_RADIOTAP: [u8; 12] = [
    0x00, 0x00, 0x0c, 0x00, 0x06, 0x80, 0x00, 0x00, 0x02, 0x04, 0x00, 0x00,
];
const PCAP_SNAPLEN: u32 = 65_535;
const LINKTYPE_IEEE802_11_RADIOTAP: u32 = 127;

/// Runtime radio configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadioConfig {
    /// 2.4 GHz Wi-Fi channel used by the room.
    pub channel: u8,
}

/// A raw frame received from the radio.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceivedFrame {
    /// Monotonic capture time.
    pub received_at: Instant,
    /// Adapter TSF timestamp in microseconds when radiotap supplies one.
    pub tsft_us: Option<u64>,
    /// Raw frame bytes presented to the protocol codec.
    pub bytes: Vec<u8>,
    /// Received signal strength when available.
    pub rssi_dbm: Option<i8>,
    /// Reported receive antenna; custom Picto Relay firmware uses `0xd0..=0xdf`
    /// on targeted polls for transport diagnostics.
    pub antenna: Option<u8>,
}

/// A raw frame requested for transmission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransmitFrame {
    /// Raw bytes to transmit.
    pub bytes: Vec<u8>,
}

/// Errors produced by a radio backend.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum RadioError {
    /// A radio channel outside the 2.4 GHz range was requested.
    #[error("unsupported 2.4 GHz channel {0}")]
    InvalidChannel(u8),
    /// The requested monitor interface does not exist.
    #[error("monitor interface {0} was not found")]
    InterfaceNotFound(String),
    /// Linux could not open the monitor interface for raw packets.
    #[error("failed to open monitor interface: {0}")]
    OpenInterface(String),
    /// The raw monitor receiver failed.
    #[error("monitor receive failed: {0}")]
    Receive(String),
    /// The raw monitor transmitter failed or had no available buffer.
    #[error("monitor transmit failed: {0}")]
    Transmit(String),
    /// A diagnostic packet capture could not be created or written.
    #[error("packet capture failed: {0}")]
    Capture(String),
}

/// Blocking raw-radio operations owned by the radio thread.
pub trait RadioBackend: Send + 'static {
    /// Configures the radio before use.
    ///
    /// # Errors
    ///
    /// Returns [`RadioError`] when configuration fails.
    fn configure(&mut self, config: RadioConfig) -> Result<(), RadioError>;

    /// Receives the next frame available no later than `deadline`.
    ///
    /// # Errors
    ///
    /// Returns [`RadioError`] when reception fails.
    fn receive(&mut self, deadline: Instant) -> Result<Option<ReceivedFrame>, RadioError>;

    /// Transmits a raw frame.
    ///
    /// # Errors
    ///
    /// Returns [`RadioError`] when transmission fails.
    fn transmit(&mut self, frame: TransmitFrame) -> Result<(), RadioError>;
}

/// Linux monitor-interface backend for AR9271 reception and radiotap injection.
pub struct LinuxMonitorRadio {
    sender: Box<dyn DataLinkSender>,
    receiver: Box<dyn DataLinkReceiver>,
    configured_channel: Option<u8>,
    capture: Option<PcapWriter>,
}

impl LinuxMonitorRadio {
    /// Opens one existing monitor-mode interface.
    ///
    /// The caller must put the interface into monitor mode and select the channel.
    ///
    /// # Errors
    ///
    /// Returns [`RadioError`] when the interface is absent or the raw channel cannot open.
    pub fn open(interface_name: &str) -> Result<Self, RadioError> {
        Self::open_inner(interface_name, None)
    }

    /// Opens a monitor interface and records all received and requested transmitted packets.
    ///
    /// # Errors
    ///
    /// Returns [`RadioError`] when the interface or capture file cannot be opened.
    pub fn open_with_capture(
        interface_name: &str,
        capture_path: &Path,
    ) -> Result<Self, RadioError> {
        let capture = PcapWriter::create(capture_path)?;
        Self::open_inner(interface_name, Some(capture))
    }

    fn open_inner(interface_name: &str, capture: Option<PcapWriter>) -> Result<Self, RadioError> {
        let interface = datalink::interfaces()
            .into_iter()
            .find(|interface| interface.name == interface_name)
            .ok_or_else(|| RadioError::InterfaceNotFound(interface_name.to_owned()))?;
        let config = datalink::Config {
            read_timeout: Some(MONITOR_READ_TIMEOUT),
            read_buffer_size: MONITOR_BUFFER_SIZE,
            write_buffer_size: MONITOR_BUFFER_SIZE,
            promiscuous: true,
            ..datalink::Config::default()
        };
        let channel = datalink::channel(&interface, config)
            .map_err(|error| RadioError::OpenInterface(error.to_string()))?;
        let Channel::Ethernet(sender, receiver) = channel else {
            return Err(RadioError::OpenInterface(
                "interface returned an unsupported channel type".into(),
            ));
        };
        Ok(Self {
            sender,
            receiver,
            configured_channel: None,
            capture,
        })
    }

    /// Flushes the optional packet capture and returns its packet and byte counts.
    ///
    /// # Errors
    ///
    /// Returns [`RadioError`] when the capture cannot be flushed.
    pub fn finish_capture(&mut self) -> Result<Option<CaptureStats>, RadioError> {
        self.capture.take().map(PcapWriter::finish).transpose()
    }
}

impl RadioBackend for LinuxMonitorRadio {
    fn configure(&mut self, config: RadioConfig) -> Result<(), RadioError> {
        validate_channel(config.channel)?;
        self.configured_channel = Some(config.channel);
        Ok(())
    }

    fn receive(&mut self, deadline: Instant) -> Result<Option<ReceivedFrame>, RadioError> {
        loop {
            if Instant::now() >= deadline {
                return Ok(None);
            }
            match self.receiver.next() {
                Ok(packet) => {
                    let received_at = Instant::now();
                    if let Some(capture) = &mut self.capture {
                        capture.write_packet(SystemTime::now(), packet)?;
                    }
                    let Ok(decoded) = decode_monitor_packet(packet) else {
                        continue;
                    };
                    return Ok(Some(ReceivedFrame {
                        received_at,
                        tsft_us: decoded.tsft_us,
                        bytes: decoded.bytes,
                        rssi_dbm: decoded.rssi_dbm,
                        antenna: decoded.antenna,
                    }));
                }
                Err(error) if is_receive_timeout(&error) => {}
                Err(error) => return Err(RadioError::Receive(error.to_string())),
            }
        }
    }

    fn transmit(&mut self, frame: TransmitFrame) -> Result<(), RadioError> {
        let mut packet = Vec::with_capacity(INJECTION_RADIOTAP.len() + frame.bytes.len());
        packet.extend_from_slice(&INJECTION_RADIOTAP);
        packet.extend_from_slice(&frame.bytes);
        match self.sender.send_to(&packet, None) {
            Some(Ok(())) => {
                if let Some(capture) = &mut self.capture {
                    capture.write_packet(SystemTime::now(), &packet)?;
                }
                Ok(())
            }
            Some(Err(error)) => Err(RadioError::Transmit(error.to_string())),
            None => Err(RadioError::Transmit(
                "driver did not provide a transmit buffer".into(),
            )),
        }
    }
}

/// Size summary for a completed radio capture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureStats {
    /// Number of packets written.
    pub packets: u64,
    /// Sum of captured packet lengths.
    pub bytes: u64,
}

struct PcapWriter {
    writer: BufWriter<File>,
    stats: CaptureStats,
}

impl PcapWriter {
    fn create(path: &Path) -> Result<Self, RadioError> {
        let file = File::create(path).map_err(|error| RadioError::Capture(error.to_string()))?;
        let mut writer = BufWriter::new(file);
        writer
            .write_all(&0xa1b2_c3d4_u32.to_le_bytes())
            .and_then(|()| writer.write_all(&2_u16.to_le_bytes()))
            .and_then(|()| writer.write_all(&4_u16.to_le_bytes()))
            .and_then(|()| writer.write_all(&0_i32.to_le_bytes()))
            .and_then(|()| writer.write_all(&0_u32.to_le_bytes()))
            .and_then(|()| writer.write_all(&PCAP_SNAPLEN.to_le_bytes()))
            .and_then(|()| writer.write_all(&LINKTYPE_IEEE802_11_RADIOTAP.to_le_bytes()))
            .map_err(|error| RadioError::Capture(error.to_string()))?;
        Ok(Self {
            writer,
            stats: CaptureStats {
                packets: 0,
                bytes: 0,
            },
        })
    }

    fn write_packet(&mut self, captured_at: SystemTime, packet: &[u8]) -> Result<(), RadioError> {
        let since_epoch = captured_at.duration_since(UNIX_EPOCH).map_err(|error| {
            RadioError::Capture(format!(
                "packet timestamp is before the Unix epoch: {error}"
            ))
        })?;
        let seconds = u32::try_from(since_epoch.as_secs())
            .map_err(|error| RadioError::Capture(error.to_string()))?;
        let length =
            u32::try_from(packet.len()).map_err(|error| RadioError::Capture(error.to_string()))?;
        self.writer
            .write_all(&seconds.to_le_bytes())
            .and_then(|()| {
                self.writer
                    .write_all(&since_epoch.subsec_micros().to_le_bytes())
            })
            .and_then(|()| self.writer.write_all(&length.to_le_bytes()))
            .and_then(|()| self.writer.write_all(&length.to_le_bytes()))
            .and_then(|()| self.writer.write_all(packet))
            .map_err(|error| RadioError::Capture(error.to_string()))?;
        self.stats.packets += 1;
        self.stats.bytes += u64::from(length);
        Ok(())
    }

    fn finish(mut self) -> Result<CaptureStats, RadioError> {
        self.writer
            .flush()
            .map_err(|error| RadioError::Capture(error.to_string()))?;
        Ok(self.stats)
    }
}

fn validate_channel(channel: u8) -> Result<(), RadioError> {
    if (1..=14).contains(&channel) {
        Ok(())
    } else {
        Err(RadioError::InvalidChannel(channel))
    }
}

fn is_receive_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}

#[derive(Debug, Eq, PartialEq)]
struct DecodedMonitorPacket {
    bytes: Vec<u8>,
    tsft_us: Option<u64>,
    rssi_dbm: Option<i8>,
    antenna: Option<u8>,
}

fn decode_monitor_packet(packet: &[u8]) -> Result<DecodedMonitorPacket, RadioError> {
    if packet.len() < 8 || packet[0] != 0 {
        return Err(RadioError::Receive("invalid radiotap header".into()));
    }
    let header_length = usize::from(u16::from_le_bytes([packet[2], packet[3]]));
    if !(8..=packet.len()).contains(&header_length) {
        return Err(RadioError::Receive("invalid radiotap length".into()));
    }
    let mut present_offset = 4;
    let mut present_words = 0_usize;
    loop {
        let present = read_radiotap_u32(packet, present_offset, header_length)?;
        present_offset += 4;
        present_words += 1;
        if present & (1 << 31) == 0 {
            break;
        }
    }
    let mut field_offset = present_offset;
    let mut flags = None;
    let mut tsft_us = None;
    let mut rssi_dbm = None;
    let mut antenna = None;
    let mut field_base = 0_usize;
    for word_index in 0..present_words {
        let present = read_radiotap_u32(packet, 4 + word_index * 4, header_length)?;
        for bit in 0..29 {
            if present & (1 << bit) == 0 {
                continue;
            }
            let field = field_base + bit;
            let (alignment, size) = radiotap_field_layout(field)?;
            field_offset = align(field_offset, alignment);
            let value = packet
                .get(field_offset..field_offset + size)
                .filter(|_| field_offset + size <= header_length)
                .ok_or_else(|| RadioError::Receive("truncated radiotap field".into()))?;
            match field {
                0 if tsft_us.is_none() => {
                    tsft_us = Some(u64::from_le_bytes(value.try_into().map_err(|_| {
                        RadioError::Receive("invalid radiotap TSFT field".into())
                    })?));
                }
                1 if flags.is_none() => flags = Some(value[0]),
                5 if rssi_dbm.is_none() => {
                    rssi_dbm = Some(i8::from_ne_bytes([value[0]]));
                }
                11 => antenna = Some(value[0]),
                _ => {}
            }
            field_offset += size;
        }
        if present & (1 << 30) != 0 {
            return Err(RadioError::Receive(
                "vendor radiotap namespaces are unsupported".into(),
            ));
        }
        field_base = if present & (1 << 29) != 0 {
            0
        } else {
            field_base + 32
        };
    }
    let fcs_length = usize::from(flags.is_some_and(|value| value & RADIOTAP_FCS != 0)) * 4;
    let frame_end = packet
        .len()
        .checked_sub(fcs_length)
        .filter(|&end| end >= header_length)
        .ok_or_else(|| RadioError::Receive("truncated monitor frame".into()))?;
    Ok(DecodedMonitorPacket {
        bytes: packet[header_length..frame_end].to_vec(),
        tsft_us,
        rssi_dbm,
        antenna,
    })
}

fn radiotap_field_layout(field: usize) -> Result<(usize, usize), RadioError> {
    match field {
        0 => Ok((8, 8)),
        1 | 2 | 5 | 6 | 10..=13 | 16 | 17 => Ok((1, 1)),
        3 => Ok((2, 4)),
        4 | 7..=9 | 14 | 15 => Ok((2, 2)),
        _ => Err(RadioError::Receive(format!(
            "unsupported radiotap field {field}"
        ))),
    }
}

const fn align(offset: usize, alignment: usize) -> usize {
    (offset + alignment - 1) & !(alignment - 1)
}

fn read_radiotap_u32(
    packet: &[u8],
    offset: usize,
    header_length: usize,
) -> Result<u32, RadioError> {
    let bytes = packet
        .get(offset..offset + 4)
        .filter(|_| offset + 4 <= header_length)
        .ok_or_else(|| RadioError::Receive("truncated radiotap bitmap".into()))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
mod monitor_tests {
    use super::*;

    #[test]
    fn monitor_packet_should_strip_radiotap_and_fcs() {
        let mut packet = vec![0x00, 0x00, 0x0a, 0x00, 0x22, 0x00, 0x00, 0x00, 0x10, 0xd8];
        packet.extend_from_slice(&[0x80, 0x00, 0x01, 0x02]);
        packet.extend_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd]);
        let decoded = decode_monitor_packet(&packet).expect("packet should decode");
        assert_eq!(
            decoded,
            DecodedMonitorPacket {
                bytes: vec![0x80, 0x00, 0x01, 0x02],
                tsft_us: None,
                rssi_dbm: Some(-40),
                antenna: None,
            }
        );
    }

    #[test]
    fn monitor_packet_should_decode_extended_antenna_namespace() {
        let mut packet = vec![
            0x00, 0x00, 0x24, 0x00, 0x2f, 0x40, 0x00, 0xa0, 0x20, 0x08, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x49, 0x3b, 0x73, 0x03, 0x00, 0x00, 0x00, 0x00, 0x10, 0x02, 0x6c, 0x09,
            0xa0, 0x00, 0xa7, 0x00, 0x00, 0x00, 0xa7, 0xd7,
        ];
        packet.extend_from_slice(&[0x88, 0x01, 0x02, 0x03]);
        packet.extend_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd]);
        let decoded = decode_monitor_packet(&packet).expect("packet should decode");
        assert_eq!(
            (decoded.tsft_us, decoded.rssi_dbm, decoded.antenna),
            (Some(57_883_465), Some(-89), Some(0xd7))
        );
    }

    #[test]
    fn capture_writer_should_create_a_radiotap_pcap() {
        let path = std::env::temp_dir().join(format!(
            "picto-relay-radio-{}-{}.pcap",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("test clock should be valid")
                .as_nanos()
        ));
        let mut writer = PcapWriter::create(&path).expect("capture should open");
        writer
            .write_packet(UNIX_EPOCH + Duration::from_secs(1), &[1, 2, 3])
            .expect("packet should write");
        writer.finish().expect("capture should flush");
        let bytes = std::fs::read(&path).expect("capture should read");
        std::fs::remove_file(path).expect("capture should be removable");

        assert_eq!(u32::from_le_bytes(bytes[20..24].try_into().unwrap()), 127);
    }
}
