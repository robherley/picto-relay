//! What the host puts in the commands it sends, and how it words the answers.
use crate::bitmap::api_bitmap_to_native;
use anyhow::{Context, Result, bail};
use picto_relay_codec::{MacAddress, encode_message};
use picto_relay_link as link;

/// Derives a participant identifier from the low four octets of a MAC,
/// skipping the two leading vendor octets.
#[must_use]
pub fn station_id(mac: MacAddress) -> u32 {
    let [_, _, device_octets @ ..] = mac.octets();
    u32::from_be_bytes(device_octets)
}

/// Appends `text` as UTF-16LE, stopping before a character that would not fit
/// in `max_bytes`, then zero-pads the field to exactly `max_bytes`.
fn push_utf16_field(output: &mut Vec<u8>, text: &str, max_bytes: usize) {
    let end = output.len() + max_bytes;
    for unit in text.encode_utf16() {
        let bytes = unit.to_le_bytes();
        if output.len() + bytes.len() > end {
            break;
        }
        output.extend_from_slice(&bytes);
    }
    output.resize(end, 0);
}

/// Builds a JOIN command's payload, following the `link::JOIN_*_OFFSET` layout.
/// A nickname too long for its field is cut short.
/// # Errors
/// Rejects a room, profile message or profile color the firmware would refuse.
pub fn join_payload(room: u8, name: &str, message: &str, color: u8) -> Result<Vec<u8>> {
    if usize::from(room) >= link::ROOM_COUNT {
        bail!("invalid room");
    }
    if message.encode_utf16().count() * size_of::<u16>() > link::PROFILE_MESSAGE_LEN
        || message.contains('\0')
        || color > link::MAX_PROFILE_COLOR
    {
        bail!("invalid profile message or color");
    }
    let mut join = vec![room];
    push_utf16_field(&mut join, name, link::NICKNAME_LEN);
    push_utf16_field(&mut join, message, link::PROFILE_MESSAGE_LEN);
    join.push(color);
    Ok(join)
}

/// Encodes an API bitmap as the native drawing the DS displays. The firmware
/// fragments it and wraps it for the radio.
/// # Errors
/// Rejects a bitmap of the wrong size and a completely blank one.
pub fn native_drawing(station: MacAddress, bitmap: &[u8]) -> Result<Vec<u8>> {
    Ok(encode_message(station, &api_bitmap_to_native(bitmap)?)?)
}

/// Splits a native drawing into DRAW payloads following the `link::DRAW_*` layout.
/// # Errors
/// Rejects a drawing too large for the layout's 16-bit positions.
pub fn draw_commands(id: u32, native: &[u8]) -> Result<Vec<Vec<u8>>> {
    let total = u16::try_from(native.len()).context("drawing too large")?;
    native
        .chunks(link::MAX_DRAW_CHUNK)
        .enumerate()
        .map(|(index, chunk)| {
            let position = u16::try_from(index * link::MAX_DRAW_CHUNK)?;
            let mut command = id.to_le_bytes().to_vec();
            command.extend_from_slice(&position.to_le_bytes());
            command.extend_from_slice(&total.to_le_bytes());
            command.extend_from_slice(chunk);
            Ok(command)
        })
        .collect()
}

/// Names a RESPONSE status.
#[must_use]
pub fn status_text(status: u8) -> &'static str {
    match status {
        link::STATUS_OK => "ok",
        link::STATUS_BUSY => "busy",
        link::STATUS_INVALID => "malformed command",
        link::STATUS_WRONG_STATE => "not possible in the current session state",
        link::STATUS_VERSION_MISMATCH => "protocol version mismatch",
        link::STATUS_NOT_ATTACHED => "no HELLO was sent",
        _ => "unknown status",
    }
}

/// Explains a LEFT event to the user.
#[must_use]
pub fn left_text(payload: &[u8]) -> String {
    let reason = payload.get(link::LEFT_REASON_OFFSET).copied();
    let code = payload
        .get(link::LEFT_CODE_OFFSET..link::LEFT_LEN)
        .and_then(|code| code.try_into().ok())
        .map_or(0, u16::from_le_bytes);
    match reason {
        Some(link::LEFT_REQUESTED) => "the relay left the room".into(),
        Some(link::LEFT_NO_HOST) => {
            "no DS is hosting that room nearby; open the room on a DS first".into()
        }
        Some(link::LEFT_JOIN_TIMEOUT) => "the DS did not answer the join request".into(),
        Some(link::LEFT_REJECTED) => format!("the DS refused the join (status {code})"),
        Some(link::LEFT_KICKED) => format!("the DS closed the session (reason {code})"),
        Some(link::LEFT_HOST_LOST) => "the DS stopped responding".into(),
        Some(link::LEFT_RADIO_STALLED) => "the ESP32 radio stalled".into(),
        _ => "the session ended for an unknown reason".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitmap::{native_message_to_api_bitmap, palette_bitmap};
    use picto_relay_codec::PictoChatMessage;

    #[test]
    fn draw_commands_cover_the_drawing_in_order() {
        let native: Vec<u8> = (0..=u8::MAX)
            .cycle()
            .take(link::MAX_DRAWING_BYTES)
            .collect();
        let commands = draw_commands(7, &native).unwrap();
        assert_eq!(commands.len(), native.len().div_ceil(link::MAX_DRAW_CHUNK));
        let mut rebuilt = Vec::new();
        for command in &commands {
            assert!(command.len() <= link::MAX_COMMAND_PAYLOAD);
            assert_eq!(
                command[link::DRAW_ID_OFFSET..link::DRAW_POSITION_OFFSET],
                7u32.to_le_bytes()
            );
            let position = u16::from_le_bytes(
                command[link::DRAW_POSITION_OFFSET..link::DRAW_TOTAL_OFFSET]
                    .try_into()
                    .unwrap(),
            );
            let total = u16::from_le_bytes(
                command[link::DRAW_TOTAL_OFFSET..link::DRAW_DATA_OFFSET]
                    .try_into()
                    .unwrap(),
            );
            assert_eq!(usize::from(position), rebuilt.len());
            assert_eq!(usize::from(total), native.len());
            rebuilt.extend_from_slice(&command[link::DRAW_DATA_OFFSET..]);
        }
        assert_eq!(rebuilt, native);
    }

    #[test]
    fn native_drawings_fit_the_firmware_limits_and_reject_blank_input() {
        let station = MacAddress([2, 1, 2, 3, 4, 5]);
        let mut bitmap = vec![0; 3072];
        bitmap[32 * 79 + 30] = 0xa5;
        let tall = native_drawing(station, &bitmap).unwrap();
        assert_eq!(tall.len(), link::MAX_DRAWING_BYTES);
        let mut bitmap = vec![0; 3072];
        bitmap[30] = 0xa5;
        let short = native_drawing(station, &bitmap).unwrap();
        assert_eq!(short.len(), link::MIN_DRAWING_BYTES);
        let decoded = PictoChatMessage::parse(&short).unwrap();
        assert_eq!(
            native_message_to_api_bitmap(&decoded),
            palette_bitmap(&bitmap)
        );
        assert!(native_drawing(station, &[0; 3072]).is_err());
    }

    #[test]
    fn join_payloads_follow_the_layout() {
        let join = join_payload(2, "Rob", "hi", 11).unwrap();
        assert_eq!(join.len(), link::JOIN_LEN);
        assert_eq!(join[link::JOIN_ROOM_OFFSET], 2);
        assert_eq!(
            join[link::JOIN_NICKNAME_OFFSET..][..8],
            [b'R', 0, b'o', 0, b'b', 0, 0, 0]
        );
        assert_eq!(
            join[link::JOIN_MESSAGE_OFFSET..][..6],
            [b'h', 0, b'i', 0, 0, 0]
        );
        assert_eq!(join[link::JOIN_COLOR_OFFSET], 11);
        // A nickname longer than the DS holds is cut, not refused.
        let long = join_payload(0, "ElevenChars", "", 0).unwrap();
        assert_eq!(long.len(), link::JOIN_LEN);
        assert_eq!(long[link::JOIN_MESSAGE_OFFSET - 2], b'r');
        assert!(join_payload(4, "Rob", "", 0).is_err());
        assert!(join_payload(0, "Rob", &"m".repeat(27), 0).is_err());
        assert!(join_payload(0, "Rob", "a\0b", 0).is_err());
        assert!(join_payload(0, "Rob", "", 16).is_err());
    }

    #[test]
    fn left_reasons_are_explained() {
        assert!(left_text(&[link::LEFT_NO_HOST, 0, 0]).contains("open the room"));
        assert!(left_text(&[link::LEFT_KICKED, 3, 0]).contains("reason 3"));
        assert!(left_text(&[]).contains("unknown"));
    }
}
