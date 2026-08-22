use anyhow::{Result, bail};
use picto_relay_codec::FcsMode;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Radiotap {
    pub(crate) header_length: usize,
    pub(crate) fcs_mode: FcsMode,
    pub(crate) rssi_dbm: Option<i8>,
}

impl Radiotap {
    pub(crate) fn parse(packet: &[u8]) -> Result<Self> {
        if packet.len() < 8 || packet[0] != 0 {
            bail!("packet does not begin with a radiotap header");
        }
        let header_length = usize::from(u16::from_le_bytes([packet[2], packet[3]]));
        if !(8..=packet.len()).contains(&header_length) {
            bail!("invalid radiotap header length {header_length}");
        }

        let mut present_offset = 4;
        let first_present = read_u32(packet, present_offset, header_length)?;
        loop {
            let present = read_u32(packet, present_offset, header_length)?;
            present_offset += 4;
            if present & (1 << 31) == 0 {
                break;
            }
        }

        let mut field_offset = present_offset;
        let mut flags = None;
        let mut rssi_dbm = None;
        for field in 0..=5 {
            if first_present & (1 << field) == 0 {
                continue;
            }
            let (alignment, size) = match field {
                0 => (8, 8),
                1 | 2 | 5 => (1, 1),
                3 => (2, 4),
                4 => (2, 2),
                _ => unreachable!(),
            };
            field_offset = align(field_offset, alignment);
            let value = packet
                .get(field_offset..field_offset + size)
                .filter(|_| field_offset + size <= header_length)
                .ok_or_else(|| anyhow::anyhow!("truncated radiotap field {field}"))?;
            match field {
                1 => flags = Some(value[0]),
                5 => rssi_dbm = Some(i8::from_ne_bytes([value[0]])),
                _ => {}
            }
            field_offset += size;
        }

        Ok(Self {
            header_length,
            fcs_mode: if flags.is_some_and(|value| value & 0x10 != 0) {
                FcsMode::Present
            } else {
                FcsMode::Absent
            },
            rssi_dbm,
        })
    }
}

const fn align(offset: usize, alignment: usize) -> usize {
    (offset + alignment - 1) & !(alignment - 1)
}

fn read_u32(packet: &[u8], offset: usize, header_length: usize) -> Result<u32> {
    let bytes = packet
        .get(offset..offset + 4)
        .filter(|_| offset + 4 <= header_length)
        .ok_or_else(|| anyhow::anyhow!("truncated radiotap present bitmap"))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_radiotap_shape_should_find_fcs_and_signal() {
        let bytes = [
            0x00, 0x00, 0x24, 0x00, 0x2f, 0x40, 0x00, 0xa0, 0x20, 0x08, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x41, 0xf6, 0x34, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x02, 0x6c, 0x09,
            0xa0, 0x00, 0xa8, 0x00, 0x00, 0x00, 0xa8, 0x00,
        ];
        let decoded = Radiotap::parse(&bytes).expect("fixture should parse");
        assert_eq!(decoded.header_length, 36);
        assert_eq!(decoded.fcs_mode, FcsMode::Present);
        assert_eq!(decoded.rssi_dbm, Some(-88));
    }
}
