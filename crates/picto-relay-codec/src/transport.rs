use std::collections::{BTreeMap, btree_map::Entry};

use crate::{
    CLIENT_ACK, CLIENT_REPLY, DecodeError, FrameType, HOST_TO_CLIENT, MAC_ADDRESS_LEN, MacAddress,
    MacFrame, SequenceControl, U16_BYTES, build_client_reply_frame, build_data_frame,
    frame::{DURATION_OFFSET, SUBTYPE_DATA_CF_ACK, SUBTYPE_DATA_CF_POLL},
    read_u16, read_word_swapped_mac, too_short,
};

// IEEE 802.11 header fields patched after the generic frame builders run.

/// Offset of the second frame-control byte, which carries the To-DS/From-DS/retry flags.
const FRAME_CONTROL_FLAGS_OFFSET: usize = 1;
/// From-DS flag within the second frame-control byte (frame-control bit 9).
const FRAME_CONTROL_FLAG_FROM_DS: u8 = 1 << 1;
/// Retry flag within the second frame-control byte (frame-control bit 11).
const FRAME_CONTROL_FLAG_RETRY: u8 = 1 << 3;
/// End of the IEEE 802.11 duration/ID field.
const DURATION_END: usize = DURATION_OFFSET + U16_BYTES;
/// Duration/ID value captured on host commands that invite at least one client.
const TARGETED_COMMAND_DURATION: u16 = 1_248;
/// Duration/ID value captured on host commands whose client mask is empty.
const BROADCAST_COMMAND_DURATION: u16 = 240;
/// Sequence number reported for frames that carry no sequence-control field.
const ABSENT_SEQUENCE_NUMBER: u16 = 0;

// Nintendo multiplayer prefix (host commands and host acknowledgements).

/// Offset of the client reply window in a host command body.
const HOST_COMMAND_CLIENT_TIME_OFFSET: usize = 0;
/// Offset of the invited-client bit mask in a host command body.
const HOST_COMMAND_CLIENT_MASK_OFFSET: usize = HOST_COMMAND_CLIENT_TIME_OFFSET + U16_BYTES;
/// Offset of the command count in a host acknowledgement body.
const ACK_COMMAND_COUNT_OFFSET: usize = 0;
/// Offset of the failed-client bit mask in a host acknowledgement body.
const ACK_FAILED_CLIENT_MASK_OFFSET: usize = ACK_COMMAND_COUNT_OFFSET + U16_BYTES;
/// Bytes in the two-word multiplayer prefix shared by both host-originated flows.
const MULTIPLAYER_HEADER_BYTES: usize = HOST_COMMAND_CLIENT_MASK_OFFSET + U16_BYTES;
/// Dominant client reply window observed in captured host commands.
const OBSERVED_CLIENT_TIME: u16 = 998;
/// Participant slot of the first non-host client.
const FIRST_CLIENT_SLOT: u16 = 1;
/// Client mask inviting only the first non-host participant slot.
const FIRST_CLIENT_MASK: u16 = 1 << FIRST_CLIENT_SLOT;

// Application envelope.

/// Offset of the packet ID in the application envelope.
const ENVELOPE_PACKET_ID_OFFSET: usize = 0;
/// Offset of the packet sequence in the application envelope.
const ENVELOPE_PACKET_SEQUENCE_OFFSET: usize = ENVELOPE_PACKET_ID_OFFSET + U16_BYTES;
/// Offset of the declared length in the application envelope.
const ENVELOPE_DECLARED_LENGTH_OFFSET: usize = ENVELOPE_PACKET_SEQUENCE_OFFSET + U16_BYTES;
/// Bytes in the application envelope header preceding the packet payload.
const ENVELOPE_HEADER_BYTES: usize = ENVELOPE_DECLARED_LENGTH_OFFSET + U16_BYTES;
/// Bytes by which a client envelope's declared length exceeds its wire payload.
const CLIENT_DECLARED_LENGTH_EXCESS: usize = U16_BYTES;
/// Bytes per unit of the packet ID's low-byte length, which counts 16-bit halfwords.
const HALFWORD_BYTES: usize = U16_BYTES;
/// Mask selecting the four-bit packet family from the packet ID's high byte.
const PACKET_FAMILY_MASK: u8 = 0x0f;
/// Packet family carrying fragmented transfers (identity records and messages).
const TRANSFER_PACKET_FAMILY: u8 = 0x0e;
/// Packet ID of a host transfer-family packet before its halfword length is added. The
/// meaning of the high nibble is not established; the value is replayed as observed.
const HOST_TRANSFER_PACKET_ID_BASE: u16 = 0x1e00;
/// Packet ID of a client transfer-family packet before its halfword length is added. The
/// meaning of the high nibble is not established; the value is replayed as observed.
const CLIENT_TRANSFER_PACKET_ID_BASE: u16 = 0x8e00;
/// Packet sequence observed on every transfer-family envelope.
const TRANSFER_PACKET_SEQUENCE: u16 = 2;

// Message-fragment packet body.

/// Bytes in the opaque per-fragment header.
const FRAGMENT_OPAQUE_HEADER_BYTES: usize = 2;
/// Bytes in the opaque transfer identifier.
const FRAGMENT_OPAQUE_ID_BYTES: usize = 2;
/// Offset of the opaque per-fragment header.
const FRAGMENT_OPAQUE_HEADER_OFFSET: usize = 0;
/// Offset of the one-byte fragment data length.
const FRAGMENT_DATA_LENGTH_OFFSET: usize =
    FRAGMENT_OPAQUE_HEADER_OFFSET + FRAGMENT_OPAQUE_HEADER_BYTES;
/// Offset of the one-byte final marker.
const FRAGMENT_FINAL_MARKER_OFFSET: usize = FRAGMENT_DATA_LENGTH_OFFSET + size_of::<u8>();
/// Offset of the reassembled-message byte offset.
const FRAGMENT_DATA_OFFSET_OFFSET: usize = FRAGMENT_FINAL_MARKER_OFFSET + size_of::<u8>();
/// Offset of the opaque transfer identifier.
const FRAGMENT_OPAQUE_ID_OFFSET: usize = FRAGMENT_DATA_OFFSET_OFFSET + U16_BYTES;
/// Bytes in the fragment header preceding the fragment data.
const FRAGMENT_HEADER_BYTES: usize = FRAGMENT_OPAQUE_ID_OFFSET + FRAGMENT_OPAQUE_ID_BYTES;
/// Final-marker value on a fragment that is followed by more data.
const FINAL_MARKER_CONTINUES: u8 = 0;
/// Final-marker value on the last fragment of a message.
const FINAL_MARKER_FINAL: u8 = 1;
/// Trailer of a client fragment: the frame number only.
const CLIENT_FRAGMENT_TRAILER_BYTES: usize = U16_BYTES;
/// Trailer of a host fragment: the frame number followed by the host-only trailer word.
const HOST_FRAGMENT_TRAILER_BYTES: usize = CLIENT_FRAGMENT_TRAILER_BYTES + U16_BYTES;
/// Host-only trailer word seen on every captured host fragment; meaning unknown.
const OBSERVED_HOST_TRAILER: u16 = 2;
/// Opaque header captured on the first fragment of a client message; meaning unknown.
const OPAQUE_FIRST_FRAGMENT_HEADER: [u8; FRAGMENT_OPAQUE_HEADER_BYTES] = [0x01, 0x6c];
/// Opaque header captured on later fragments of a client message; meaning unknown.
const OPAQUE_CONTINUATION_FRAGMENT_HEADER: [u8; FRAGMENT_OPAQUE_HEADER_BYTES] = [0x01, 0x04];
/// Frame number given to the first fragment when the caller does not choose one.
const DEFAULT_FIRST_FRAME_NUMBER: u16 = 0;

/// Maximum message data carried by each observed non-final `PictoChat` fragment.
pub const MESSAGE_FRAGMENT_PAYLOAD_BYTES: usize = 160;

// Identity/profile record carried by an `e30` fragment.

/// Offset of the record marker.
const PROFILE_MARKER_OFFSET: usize = 0;
/// Marker value that opens every identity record.
const PROFILE_RECORD_MARKER: u8 = 3;
/// Offset of the identity subtype (pass number).
const PROFILE_SUBTYPE_OFFSET: usize = PROFILE_MARKER_OFFSET + size_of::<u8>();
/// Largest valid identity subtype: `0` is the initial pass and `1` the final pass.
const PROFILE_FINAL_SUBTYPE: u8 = 1;
/// Offset of the word-swapped station MAC.
const PROFILE_STATION_OFFSET: usize = PROFILE_SUBTYPE_OFFSET + size_of::<u8>();
/// Offset of the fixed-width UTF-16LE nickname.
const PROFILE_NICKNAME_OFFSET: usize = PROFILE_STATION_OFFSET + MAC_ADDRESS_LEN;
/// UTF-16 code units in the nickname field.
const PROFILE_NICKNAME_UNITS: usize = 10;
/// Offset of the fixed-width UTF-16LE personal message.
const PROFILE_MESSAGE_OFFSET: usize = PROFILE_NICKNAME_OFFSET + PROFILE_NICKNAME_UNITS * U16_BYTES;
/// UTF-16 code units in the personal-message field.
const PROFILE_MESSAGE_UNITS: usize = 26;
/// Offset of the little-endian profile color.
const PROFILE_COLOR_OFFSET: usize = PROFILE_MESSAGE_OFFSET + PROFILE_MESSAGE_UNITS * U16_BYTES;
/// Offset of the two birthday bytes, whose month/day order is unresolved.
const PROFILE_BIRTHDAY_OFFSET: usize = PROFILE_COLOR_OFFSET + U16_BYTES;
/// Bytes in the birthday field.
const PROFILE_BIRTHDAY_BYTES: usize = 2;
/// Bytes in one identity/profile record carried by an `e30` fragment.
pub const PROFILE_BYTES: usize = PROFILE_BIRTHDAY_OFFSET + PROFILE_BIRTHDAY_BYTES;
/// UTF-16 code unit terminating a fixed-width profile string.
const UTF16_NUL: u16 = 0;
/// U+3000 ideographic space, used as trailing padding by Japanese-region consoles.
const IDEOGRAPHIC_SPACE: char = '\u{3000}';
/// UTF-16 code unit of [`IDEOGRAPHIC_SPACE`].
const IDEOGRAPHIC_SPACE_UNIT: u16 = 0x3000;

// Member/session roster carried by a host `c34` packet.

/// Participant slots carried by one host `c34` member/session record.
pub const ROSTER_SLOTS: usize = 16;
/// Opaque bytes preceding the roster's station entries.
const ROSTER_OPAQUE_PREFIX_BYTES: usize = 4;
/// Opaque bytes following the roster's station entries.
const ROSTER_OPAQUE_SUFFIX_BYTES: usize = 4;
/// Bytes in one roster entry: a word-swapped station MAC.
const ROSTER_ENTRY_BYTES: usize = MAC_ADDRESS_LEN;
/// End of the roster's station entries.
const ROSTER_ENTRIES_END: usize = ROSTER_OPAQUE_PREFIX_BYTES + ROSTER_SLOTS * ROSTER_ENTRY_BYTES;
/// Bytes in one host `c34` member/session payload.
const ROSTER_BYTES: usize = ROSTER_ENTRIES_END + ROSTER_OPAQUE_SUFFIX_BYTES;

/// Direction/role inferred from Nintendo's three multiplayer destination addresses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NintendoFlow {
    /// Host command/data sent to clients.
    HostToClient,
    /// Client reply sent to the host.
    ClientToHost,
    /// Host acknowledgement of client replies.
    HostAcknowledgement,
}

/// Direction-specific header immediately following the IEEE 802.11 header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MultiplayerHeader {
    /// Host command timing and intended-client bit mask.
    HostCommand {
        /// Client reply window observed in the command.
        client_time: u16,
        /// Bit mask of clients invited to reply.
        client_mask: u16,
    },
    /// Client replies do not carry a multiplayer prefix.
    ClientReply,
    /// Host result for one client-reply collection round.
    HostAcknowledgement {
        /// Host command counter being acknowledged.
        command_count: u16,
        /// Bit mask of clients whose replies were not received.
        failed_client_mask: u16,
    },
}

/// Nintendo multiplayer payload extracted from a raw data frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NintendoData {
    /// Multiplayer direction inferred from destination addresses.
    pub flow: NintendoFlow,
    /// Transmitting station.
    pub source: MacAddress,
    /// Basic service set identifier.
    pub bssid: Option<MacAddress>,
    /// MAC sequence number used for duplicate/retry correlation.
    pub sequence_number: u16,
    /// Retry flag from the MAC header.
    pub retry: bool,
    /// Direction-specific Nintendo multiplayer header.
    pub multiplayer: MultiplayerHeader,
    /// Remaining application payload.
    pub payload: Vec<u8>,
}

/// Six-byte application envelope surrounding Nintendo payload packets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PictoChatEnvelope {
    /// Packet kind, including direction and completion state.
    pub packet_id: u16,
    /// Application sequence observed in the capture.
    pub packet_sequence: u16,
    /// Length value carried on the wire.
    pub declared_length: u16,
    /// Packet payload after the envelope.
    pub payload: Vec<u8>,
}

impl PictoChatEnvelope {
    /// Parses an application envelope and validates its direction-specific length convention.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the envelope is truncated or its declared length does not
    /// match the captured host/client convention.
    pub fn parse(flow: NintendoFlow, bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < ENVELOPE_HEADER_BYTES {
            return Err(too_short(
                "PictoChat envelope",
                ENVELOPE_HEADER_BYTES,
                bytes.len(),
            ));
        }
        let declared_length =
            read_u16(bytes, ENVELOPE_DECLARED_LENGTH_OFFSET, "PictoChat envelope")?;
        let payload = bytes[ENVELOPE_HEADER_BYTES..].to_vec();
        let packet_id = read_u16(bytes, ENVELOPE_PACKET_ID_OFFSET, "PictoChat envelope")?;
        let expected = match flow {
            NintendoFlow::ClientToHost => payload.len().checked_add(CLIENT_DECLARED_LENGTH_EXCESS),
            NintendoFlow::HostToClient | NintendoFlow::HostAcknowledgement => Some(payload.len()),
        };
        let physical_length = usize::from(packet_id_length_halfwords(packet_id)) * HALFWORD_BYTES;
        let padded_host_fragment = flow == NintendoFlow::HostToClient
            && packet_id_family(packet_id) == TRANSFER_PACKET_FAMILY
            && physical_length == payload.len()
            && usize::from(declared_length) <= payload.len();
        if expected != Some(usize::from(declared_length)) && !padded_host_fragment {
            return Err(DecodeError::InvalidEnvelope(
                "application envelope length does not match its flow",
            ));
        }
        let envelope = Self {
            packet_id,
            packet_sequence: read_u16(
                bytes,
                ENVELOPE_PACKET_SEQUENCE_OFFSET,
                "PictoChat envelope",
            )?,
            declared_length,
            payload,
        };
        if !envelope.packet_length_matches() {
            return Err(DecodeError::InvalidEnvelope(
                "packet ID length does not match the declared length",
            ));
        }
        Ok(envelope)
    }

    /// Returns the four-bit application family carried in the packet ID's high byte.
    #[must_use]
    pub const fn packet_family(&self) -> u8 {
        packet_id_family(self.packet_id)
    }

    /// Returns the physical payload length in 16-bit words carried in the packet ID's low byte.
    #[must_use]
    pub const fn packet_length_halfwords(&self) -> u8 {
        packet_id_length_halfwords(self.packet_id)
    }

    /// Reports whether the packet ID agrees with the logical or padded physical payload length.
    #[must_use]
    pub fn packet_length_matches(&self) -> bool {
        let packet_length = usize::from(self.packet_length_halfwords()) * HALFWORD_BYTES;
        packet_length == usize::from(self.declared_length) || packet_length == self.payload.len()
    }

    /// Reports whether the packet belongs to the captured `0x0e` transfer family.
    #[must_use]
    pub fn is_message_fragment(&self) -> bool {
        self.packet_family() == TRANSFER_PACKET_FAMILY && self.packet_length_matches()
    }

    fn encode(&self, flow: NintendoFlow) -> Result<Vec<u8>, DecodeError> {
        let declared = match flow {
            NintendoFlow::ClientToHost => self
                .payload
                .len()
                .checked_add(CLIENT_DECLARED_LENGTH_EXCESS),
            NintendoFlow::HostToClient | NintendoFlow::HostAcknowledgement => {
                Some(self.payload.len())
            }
        }
        .ok_or(DecodeError::FragmentTooLarge(self.payload.len()))?;
        let declared = u16::try_from(declared)
            .map_err(|_| DecodeError::FragmentTooLarge(self.payload.len()))?;
        if usize::from(self.packet_length_halfwords()) * HALFWORD_BYTES != usize::from(declared) {
            return Err(DecodeError::InvalidEnvelope(
                "packet ID length does not match the encoded payload",
            ));
        }
        let mut bytes = Vec::with_capacity(ENVELOPE_HEADER_BYTES + self.payload.len());
        bytes.extend_from_slice(&self.packet_id.to_le_bytes());
        bytes.extend_from_slice(&self.packet_sequence.to_le_bytes());
        bytes.extend_from_slice(&declared.to_le_bytes());
        bytes.extend_from_slice(&self.payload);
        Ok(bytes)
    }
}

/// Returns the four-bit application family from a packet ID's high byte.
const fn packet_id_family(packet_id: u16) -> u8 {
    let [_length_halfwords, high] = packet_id.to_le_bytes();
    high & PACKET_FAMILY_MASK
}

/// Returns a packet ID's low byte: the payload length in 16-bit halfwords.
const fn packet_id_length_halfwords(packet_id: u16) -> u8 {
    let [length_halfwords, _high] = packet_id.to_le_bytes();
    length_halfwords
}

/// Identity advertised by one `PictoChat` participant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PictoChatProfile {
    /// Identity pass (`0` for the initial pass and `1` for the final pass).
    pub subtype: u8,
    /// Station address encoded as three little-endian words on the wire.
    pub station: MacAddress,
    /// Fixed-width, null-terminated UTF-16LE nickname.
    pub nickname: String,
    /// Personal message from the fixed-width UTF-16LE profile field.
    pub message: String,
    /// Raw profile color identifier.
    pub color: u16,
}

impl PictoChatProfile {
    /// Parses one 84-byte identity/profile record.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::InvalidProfile`] when the record length, marker, or
    /// identity subtype does not match the capture-backed layout.
    pub fn parse(record: &[u8]) -> Result<Self, DecodeError> {
        if record.len() != PROFILE_BYTES {
            return Err(DecodeError::InvalidProfile(
                "identity record must contain exactly 84 bytes",
            ));
        }
        if record[PROFILE_MARKER_OFFSET] != PROFILE_RECORD_MARKER {
            return Err(DecodeError::InvalidProfile(
                "identity record marker is not 3",
            ));
        }
        let subtype = record[PROFILE_SUBTYPE_OFFSET];
        if subtype > PROFILE_FINAL_SUBTYPE {
            return Err(DecodeError::InvalidProfile(
                "identity subtype is not 0 or 1",
            ));
        }
        let station = read_word_swapped_mac(record, PROFILE_STATION_OFFSET);
        let mut nickname_units =
            utf16_units(&record[PROFILE_NICKNAME_OFFSET..PROFILE_MESSAGE_OFFSET])
                .collect::<Vec<_>>();
        if let Some(terminator) = nickname_units.iter().position(|&unit| unit == UTF16_NUL) {
            nickname_units.truncate(terminator);
        }
        while nickname_units.last() == Some(&IDEOGRAPHIC_SPACE_UNIT) {
            nickname_units.pop();
        }
        Ok(Self {
            subtype,
            station,
            color: read_u16(record, PROFILE_COLOR_OFFSET, "PictoChat profile")?,
            nickname: String::from_utf16_lossy(&nickname_units),
            message: String::from_utf16_lossy(
                &utf16_units(&record[PROFILE_MESSAGE_OFFSET..PROFILE_COLOR_OFFSET])
                    .take_while(|&unit| unit != UTF16_NUL)
                    .collect::<Vec<_>>(),
            )
            .trim_end_matches(IDEOGRAPHIC_SPACE)
            .to_owned(),
        })
    }
}

/// Slot-to-station mapping carried by a host `c34` member/session record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PictoChatRoster {
    /// Slots zero through fifteen; an all-zero MAC marks an unused slot.
    pub members: [Option<MacAddress>; ROSTER_SLOTS],
}

impl PictoChatRoster {
    /// Parses the 104-byte host roster payload inside a `c34` envelope.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::InvalidRoster`] when the payload does not have the
    /// capture-backed fixed length.
    pub fn parse(payload: &[u8]) -> Result<Self, DecodeError> {
        if payload.len() != ROSTER_BYTES {
            return Err(DecodeError::InvalidRoster(
                "member/session payload must contain exactly 104 bytes",
            ));
        }
        let mut members = [None; ROSTER_SLOTS];
        for (slot, encoded) in payload[ROSTER_OPAQUE_PREFIX_BYTES..ROSTER_ENTRIES_END]
            .chunks_exact(ROSTER_ENTRY_BYTES)
            .enumerate()
        {
            if encoded.iter().any(|&byte| byte != 0) {
                members[slot] = Some(read_word_swapped_mac(encoded, 0));
            }
        }
        Ok(Self { members })
    }
}

/// Decodes a fixed-width UTF-16LE field into code units, ignoring a trailing odd byte.
fn utf16_units(field: &[u8]) -> impl Iterator<Item = u16> + '_ {
    let (units, _odd_byte) = field.as_chunks::<U16_BYTES>();
    units.iter().copied().map(u16::from_le_bytes)
}

/// Copies a two-byte opaque field out of a fragment payload.
fn opaque_pair(payload: &[u8], offset: usize) -> [u8; U16_BYTES] {
    [payload[offset], payload[offset + 1]]
}

/// Capture-backed `PictoChat` message fragment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PictoChatFragment {
    /// Two opaque per-fragment header bytes.
    pub opaque_header: [u8; FRAGMENT_OPAQUE_HEADER_BYTES],
    /// Explicit completion marker (`0x01` on the final fragment).
    pub final_fragment: bool,
    /// Offset in the reassembled message.
    pub data_offset: u16,
    /// Opaque transfer identifier retained verbatim.
    pub opaque_id: [u8; FRAGMENT_OPAQUE_ID_BYTES],
    /// Fragment data.
    pub data: Vec<u8>,
    /// Trailing frame counter.
    pub frame_number: u16,
    /// Additional host-only trailer word.
    pub host_trailer: Option<u16>,
}

impl PictoChatFragment {
    /// Parses the eight-byte fragment header, data, and direction-specific trailer.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] for truncated data or an invalid completion flag/length.
    pub fn parse(payload: &[u8], flow: NintendoFlow) -> Result<Self, DecodeError> {
        let trailer_length = match flow {
            NintendoFlow::HostToClient => HOST_FRAGMENT_TRAILER_BYTES,
            NintendoFlow::ClientToHost => CLIENT_FRAGMENT_TRAILER_BYTES,
            NintendoFlow::HostAcknowledgement => {
                return Err(DecodeError::InvalidFragment(
                    "host acknowledgements do not contain message fragments",
                ));
            }
        };
        let minimum = FRAGMENT_HEADER_BYTES + trailer_length;
        if payload.len() < minimum {
            return Err(too_short("PictoChat fragment", minimum, payload.len()));
        }
        let data_length = usize::from(payload[FRAGMENT_DATA_LENGTH_OFFSET]);
        let data_end = FRAGMENT_HEADER_BYTES + data_length;
        let trailer_start = payload.len() - trailer_length;
        if data_end > trailer_start {
            return Err(DecodeError::InvalidFragment(
                "declared fragment length overlaps its trailer",
            ));
        }
        let padding = &payload[data_end..trailer_start];
        if flow != NintendoFlow::HostToClient && !padding.is_empty() {
            return Err(DecodeError::InvalidFragment(
                "client fragment contains padding before its trailer",
            ));
        }
        let final_fragment = match payload[FRAGMENT_FINAL_MARKER_OFFSET] {
            FINAL_MARKER_CONTINUES => false,
            FINAL_MARKER_FINAL => true,
            _ => return Err(DecodeError::InvalidFragment("invalid completion marker")),
        };
        Ok(Self {
            opaque_header: opaque_pair(payload, FRAGMENT_OPAQUE_HEADER_OFFSET),
            final_fragment,
            data_offset: read_u16(payload, FRAGMENT_DATA_OFFSET_OFFSET, "PictoChat fragment")?,
            opaque_id: opaque_pair(payload, FRAGMENT_OPAQUE_ID_OFFSET),
            data: payload[FRAGMENT_HEADER_BYTES..data_end].to_vec(),
            frame_number: read_u16(payload, trailer_start, "PictoChat fragment")?,
            host_trailer: (flow == NintendoFlow::HostToClient)
                .then(|| {
                    read_u16(
                        payload,
                        trailer_start + CLIENT_FRAGMENT_TRAILER_BYTES,
                        "PictoChat fragment",
                    )
                })
                .transpose()?,
        })
    }

    /// Encodes a fragment using the observed direction-specific trailer.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::FragmentTooLarge`] when data exceeds the one-byte field.
    pub fn encode_for_flow(&self, flow: NintendoFlow) -> Result<Vec<u8>, DecodeError> {
        let data_length = u8::try_from(self.data.len())
            .map_err(|_| DecodeError::FragmentTooLarge(self.data.len()))?;
        if flow == NintendoFlow::HostAcknowledgement {
            return Err(DecodeError::InvalidFragment(
                "host acknowledgements do not contain message fragments",
            ));
        }
        let trailer_length = if flow == NintendoFlow::HostToClient {
            HOST_FRAGMENT_TRAILER_BYTES
        } else {
            CLIENT_FRAGMENT_TRAILER_BYTES
        };
        let mut payload =
            Vec::with_capacity(FRAGMENT_HEADER_BYTES + self.data.len() + trailer_length);
        payload.extend_from_slice(&self.opaque_header);
        payload.push(data_length);
        payload.push(if self.final_fragment {
            FINAL_MARKER_FINAL
        } else {
            FINAL_MARKER_CONTINUES
        });
        payload.extend_from_slice(&self.data_offset.to_le_bytes());
        payload.extend_from_slice(&self.opaque_id);
        payload.extend_from_slice(&self.data);
        payload.extend_from_slice(&self.frame_number.to_le_bytes());
        if flow == NintendoFlow::HostToClient {
            payload.extend_from_slice(
                &self
                    .host_trailer
                    .unwrap_or(OBSERVED_HOST_TRAILER)
                    .to_le_bytes(),
            );
        }
        Ok(payload)
    }
}

/// Stable key for one in-progress fragmented message.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ReassemblyKey {
    /// Transmitting MAC address.
    pub source: MacAddress,
    /// Opaque two-byte identifier from the partially understood header.
    pub opaque_id: [u8; FRAGMENT_OPAQUE_ID_BYTES],
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StoredFragment {
    data: Vec<u8>,
    frame_number: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FinalFragment {
    end: usize,
    frame_number: u16,
}

#[derive(Clone, Debug, Default)]
struct PartialMessage {
    fragments: BTreeMap<u16, StoredFragment>,
    start_frame: Option<u16>,
    final_fragment: Option<FinalFragment>,
}

/// Bounded, offset-aware fragment reassembler.
#[derive(Debug)]
pub struct MessageReassembler {
    partial: BTreeMap<ReassemblyKey, PartialMessage>,
    fragment_payload_limit: usize,
}

impl Default for MessageReassembler {
    fn default() -> Self {
        Self::new(MESSAGE_FRAGMENT_PAYLOAD_BYTES)
    }
}

impl MessageReassembler {
    /// Creates a reassembler using the supplied full-fragment payload size for frame correlation.
    #[must_use]
    pub fn new(fragment_payload_limit: usize) -> Self {
        Self {
            partial: BTreeMap::new(),
            fragment_payload_limit: fragment_payload_limit.max(1),
        }
    }

    /// Adds a fragment and returns a complete message once offsets are contiguous.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::ConflictingFragment`] for inconsistent overlap.
    pub fn push(
        &mut self,
        key: ReassemblyKey,
        fragment: &PictoChatFragment,
    ) -> Result<Option<Vec<u8>>, DecodeError> {
        let partial = self.partial.entry(key).or_default();
        if fragment.data_offset == 0 {
            align_transfer_start(partial, fragment.frame_number, self.fragment_payload_limit);
        }
        if let Some(start_frame) = partial.start_frame {
            let expected = frame_for_offset(
                start_frame,
                fragment.data_offset,
                self.fragment_payload_limit,
            );
            if expected != fragment.frame_number {
                return Ok(None);
            }
        }
        match partial.fragments.entry(fragment.data_offset) {
            Entry::Vacant(entry) => {
                entry.insert(StoredFragment {
                    data: fragment.data.clone(),
                    frame_number: fragment.frame_number,
                });
            }
            Entry::Occupied(entry)
                if entry.get().data == fragment.data
                    && entry.get().frame_number == fragment.frame_number => {}
            Entry::Occupied(_) => return Err(DecodeError::ConflictingFragment),
        }
        if fragment.final_fragment {
            partial.final_fragment = Some(FinalFragment {
                end: usize::from(fragment.data_offset) + fragment.data.len(),
                frame_number: fragment.frame_number,
            });
        }
        let Some(final_fragment) = partial.final_fragment else {
            return Ok(None);
        };
        let mut message = Vec::with_capacity(final_fragment.end);
        let mut expected_offset = 0;
        for (&offset, stored) in &partial.fragments {
            if usize::from(offset) != expected_offset {
                return Ok(None);
            }
            message.extend_from_slice(&stored.data);
            expected_offset += stored.data.len();
        }
        if expected_offset != final_fragment.end {
            return Ok(None);
        }
        self.partial.remove(&key);
        Ok(Some(message))
    }
}

fn align_transfer_start(partial: &mut PartialMessage, start_frame: u16, payload_limit: usize) {
    if partial.start_frame == Some(start_frame) {
        return;
    }
    if partial.start_frame.is_some() {
        *partial = PartialMessage::default();
    } else {
        partial.fragments.retain(|&offset, stored| {
            frame_for_offset(start_frame, offset, payload_limit) == stored.frame_number
        });
        partial.final_fragment = partial.final_fragment.filter(|final_fragment| {
            partial
                .fragments
                .values()
                .any(|stored| stored.frame_number == final_fragment.frame_number)
        });
    }
    partial.start_frame = Some(start_frame);
}

fn frame_for_offset(start_frame: u16, offset: u16, payload_limit: usize) -> u16 {
    let fragment_index = usize::from(offset) / payload_limit;
    let fragment_index = u16::try_from(fragment_index).unwrap_or(u16::MAX);
    start_frame.wrapping_add(fragment_index)
}

/// Extracts Nintendo multiplayer headers and application payload.
///
/// # Errors
///
/// Returns [`DecodeError`] for non-data frames, unknown flows, or truncated multiplayer headers.
pub fn decode_nintendo_data(frame: &MacFrame) -> Result<NintendoData, DecodeError> {
    if frame.header.frame_control.frame_type() != FrameType::Data {
        return Err(DecodeError::NotNintendoDataFrame);
    }
    let flow = if frame.header.address_1 == HOST_TO_CLIENT {
        NintendoFlow::HostToClient
    } else if frame.header.address_1 == CLIENT_ACK {
        NintendoFlow::HostAcknowledgement
    } else if frame.header.address_1 == CLIENT_REPLY || frame.header.address_3 == Some(CLIENT_REPLY)
    {
        NintendoFlow::ClientToHost
    } else {
        return Err(DecodeError::NotNintendoDataFrame);
    };
    let body = frame.body();
    let (multiplayer, payload) = match flow {
        NintendoFlow::HostToClient => {
            if body.len() < MULTIPLAYER_HEADER_BYTES {
                return Err(too_short(
                    "host multiplayer command",
                    MULTIPLAYER_HEADER_BYTES,
                    body.len(),
                ));
            }
            (
                MultiplayerHeader::HostCommand {
                    client_time: read_u16(
                        body,
                        HOST_COMMAND_CLIENT_TIME_OFFSET,
                        "host multiplayer command",
                    )?,
                    client_mask: read_u16(
                        body,
                        HOST_COMMAND_CLIENT_MASK_OFFSET,
                        "host multiplayer command",
                    )?,
                },
                body[MULTIPLAYER_HEADER_BYTES..].to_vec(),
            )
        }
        NintendoFlow::ClientToHost => (MultiplayerHeader::ClientReply, body.to_vec()),
        NintendoFlow::HostAcknowledgement => {
            if body.len() < MULTIPLAYER_HEADER_BYTES {
                return Err(too_short(
                    "host multiplayer acknowledgement",
                    MULTIPLAYER_HEADER_BYTES,
                    body.len(),
                ));
            }
            (
                MultiplayerHeader::HostAcknowledgement {
                    command_count: read_u16(
                        body,
                        ACK_COMMAND_COUNT_OFFSET,
                        "host multiplayer acknowledgement",
                    )?,
                    failed_client_mask: read_u16(
                        body,
                        ACK_FAILED_CLIENT_MASK_OFFSET,
                        "host multiplayer acknowledgement",
                    )?,
                },
                body[MULTIPLAYER_HEADER_BYTES..].to_vec(),
            )
        }
    };
    let source = frame.header.address_2.unwrap_or(frame.header.address_1);
    let bssid = if flow == NintendoFlow::ClientToHost && frame.header.frame_control.to_ds() {
        Some(frame.header.address_1)
    } else {
        frame.header.address_3
    };
    Ok(NintendoData {
        flow,
        source,
        bssid,
        sequence_number: frame
            .header
            .sequence_control
            .map_or(ABSENT_SEQUENCE_NUMBER, SequenceControl::sequence_number),
        retry: frame.header.frame_control.retry(),
        multiplayer,
        payload,
    })
}

/// Splits one completed client payload into captured-layout message fragments.
///
/// # Errors
///
/// Returns [`DecodeError`] when offsets or fragment sizes exceed observed fields.
pub fn fragment_payload(
    payload: &[u8],
    opaque_id: [u8; FRAGMENT_OPAQUE_ID_BYTES],
    max_payload: usize,
) -> Result<Vec<PictoChatFragment>, DecodeError> {
    fragment_payload_starting_at(payload, opaque_id, max_payload, DEFAULT_FIRST_FRAME_NUMBER)
}

/// Splits a completed payload into captured-layout fragments with a chosen first frame number.
///
/// Real `PictoChat` drawing transfers continue the application frame counter established by the
/// identity exchange. The first observed drawing fragment therefore uses frame number three.
///
/// # Errors
///
/// Returns [`DecodeError`] when offsets, fragment sizes, or frame numbers exceed wire fields.
pub fn fragment_payload_starting_at(
    payload: &[u8],
    opaque_id: [u8; FRAGMENT_OPAQUE_ID_BYTES],
    max_payload: usize,
    first_frame_number: u16,
) -> Result<Vec<PictoChatFragment>, DecodeError> {
    if max_payload == 0 || max_payload > usize::from(u8::MAX) {
        return Err(DecodeError::FragmentTooLarge(max_payload));
    }
    if payload.is_empty() {
        return Ok(vec![PictoChatFragment {
            opaque_header: OPAQUE_FIRST_FRAGMENT_HEADER,
            final_fragment: true,
            data_offset: 0,
            opaque_id,
            data: Vec::new(),
            frame_number: first_frame_number,
            host_trailer: None,
        }]);
    }
    payload
        .chunks(max_payload)
        .enumerate()
        .map(|(index, data)| {
            let offset = index
                .checked_mul(max_payload)
                .ok_or(DecodeError::OffsetTooLarge(usize::MAX))?;
            Ok(PictoChatFragment {
                opaque_header: if index == 0 {
                    OPAQUE_FIRST_FRAGMENT_HEADER
                } else {
                    OPAQUE_CONTINUATION_FRAGMENT_HEADER
                },
                final_fragment: offset + data.len() == payload.len(),
                data_offset: u16::try_from(offset)
                    .map_err(|_| DecodeError::OffsetTooLarge(offset))?,
                opaque_id,
                data: data.to_vec(),
                frame_number: first_frame_number
                    .checked_add(
                        u16::try_from(index).map_err(|_| DecodeError::OffsetTooLarge(index))?,
                    )
                    .ok_or(DecodeError::OffsetTooLarge(index))?,
                host_trailer: None,
            })
        })
        .collect()
}

/// Builds one Nintendo client-reply or host-command data frame around a fragment.
///
/// # Errors
///
/// Returns [`DecodeError`] when the fragment or envelope cannot be encoded.
pub fn build_fragment_frame(
    flow: NintendoFlow,
    source: MacAddress,
    bssid: MacAddress,
    sequence_number: u16,
    retry: bool,
    fragment: &PictoChatFragment,
) -> Result<Vec<u8>, DecodeError> {
    let envelope = encode_fragment_envelope(flow, fragment)?;
    match flow {
        NintendoFlow::HostToClient => Ok(build_host_command_frame(
            source,
            bssid,
            sequence_number,
            OBSERVED_CLIENT_TIME,
            FIRST_CLIENT_MASK,
            &envelope,
        )),
        NintendoFlow::ClientToHost => {
            let mut frame = build_client_reply_frame(source, bssid, sequence_number, &envelope);
            if retry {
                frame[FRAME_CONTROL_FLAGS_OFFSET] |= FRAME_CONTROL_FLAG_RETRY;
            }
            Ok(frame)
        }
        NintendoFlow::HostAcknowledgement => unreachable!(),
    }
}

/// Builds one Nintendo host CMD/CF-Poll frame.
///
/// The client mask controls both the reply invitation and the captured 802.11
/// reservation duration. Nintendo hosts transmit these frames with From-DS set.
#[must_use]
pub fn build_host_command_frame(
    source: MacAddress,
    bssid: MacAddress,
    sequence_number: u16,
    client_time: u16,
    client_mask: u16,
    payload: &[u8],
) -> Vec<u8> {
    let mut body = Vec::with_capacity(MULTIPLAYER_HEADER_BYTES + payload.len());
    body.extend_from_slice(&client_time.to_le_bytes());
    body.extend_from_slice(&client_mask.to_le_bytes());
    body.extend_from_slice(payload);
    let mut frame = build_data_frame(
        SUBTYPE_DATA_CF_POLL,
        HOST_TO_CLIENT,
        source,
        bssid,
        sequence_number,
        false,
        &body,
    );
    frame[FRAME_CONTROL_FLAGS_OFFSET] |= FRAME_CONTROL_FLAG_FROM_DS;
    let duration = if client_mask == 0 {
        BROADCAST_COMMAND_DURATION
    } else {
        TARGETED_COMMAND_DURATION
    };
    frame[DURATION_OFFSET..DURATION_END].copy_from_slice(&duration.to_le_bytes());
    frame
}

/// Encodes a fragment into its direction-specific `PictoChat` envelope.
///
/// Unlike [`build_fragment_frame`], this function does not construct an IEEE 802.11
/// header and therefore does not require a host BSSID.
///
/// # Errors
///
/// Returns [`DecodeError`] when the fragment or envelope cannot be encoded, or when
/// called for a host acknowledgement flow.
pub fn encode_fragment_envelope(
    flow: NintendoFlow,
    fragment: &PictoChatFragment,
) -> Result<Vec<u8>, DecodeError> {
    if flow == NintendoFlow::HostAcknowledgement {
        return Err(DecodeError::InvalidFragment(
            "use build_ack_frame for host acknowledgements",
        ));
    }
    let fragment_payload = fragment.encode_for_flow(flow)?;
    let packet_id = transfer_packet_id(flow, fragment_payload.len())?;
    let envelope = PictoChatEnvelope {
        packet_id,
        packet_sequence: TRANSFER_PACKET_SEQUENCE,
        declared_length: 0,
        payload: fragment_payload,
    }
    .encode(flow)?;
    Ok(envelope)
}

fn transfer_packet_id(flow: NintendoFlow, wire_payload_length: usize) -> Result<u16, DecodeError> {
    let declared_length = match flow {
        NintendoFlow::ClientToHost => {
            wire_payload_length.checked_add(CLIENT_DECLARED_LENGTH_EXCESS)
        }
        NintendoFlow::HostToClient => Some(wire_payload_length),
        NintendoFlow::HostAcknowledgement => None,
    }
    .ok_or(DecodeError::InvalidFragment(
        "host acknowledgements do not contain transfer packets",
    ))?;
    if !declared_length.is_multiple_of(HALFWORD_BYTES) {
        return Err(DecodeError::InvalidEnvelope(
            "transfer payload length is not a whole number of halfwords",
        ));
    }
    let halfwords = u8::try_from(declared_length / HALFWORD_BYTES)
        .map_err(|_| DecodeError::FragmentTooLarge(wire_payload_length))?;
    let family = match flow {
        NintendoFlow::HostToClient => HOST_TRANSFER_PACKET_ID_BASE,
        NintendoFlow::ClientToHost => CLIENT_TRANSFER_PACKET_ID_BASE,
        NintendoFlow::HostAcknowledgement => unreachable!(),
    };
    Ok(family | u16::from(halfwords))
}

/// Builds a Nintendo host acknowledgement carrying command and failure counters.
#[must_use]
pub fn build_ack_frame(
    source: MacAddress,
    bssid: MacAddress,
    sequence_number: u16,
    command_count: u16,
    failed_client_mask: u16,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(MULTIPLAYER_HEADER_BYTES);
    body.extend_from_slice(&command_count.to_le_bytes());
    body.extend_from_slice(&failed_client_mask.to_le_bytes());
    let mut frame = build_data_frame(
        SUBTYPE_DATA_CF_ACK,
        CLIENT_ACK,
        source,
        bssid,
        sequence_number,
        false,
        &body,
    );
    frame[FRAME_CONTROL_FLAGS_OFFSET] |= FRAME_CONTROL_FLAG_FROM_DS;
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FcsMode;

    #[test]
    fn fragmented_payload_should_reassemble_out_of_order() {
        let source = MacAddress([0, 9, 0xbf, 1, 2, 3]);
        let fragments =
            fragment_payload(&vec![0x55; 512], [7, 0], 160).expect("fixture should fragment");
        let key = ReassemblyKey {
            source,
            opaque_id: [7, 0],
        };
        let mut reassembler = MessageReassembler::new(160);
        let mut completed = None;
        for fragment in fragments.iter().rev() {
            completed = reassembler
                .push(key, fragment)
                .expect("fixture should reassemble")
                .or(completed);
        }
        assert_eq!(completed, Some(vec![0x55; 512]));
    }

    #[test]
    fn host_small_final_packet_should_be_recognized_as_fragment() {
        let envelope = PictoChatEnvelope {
            packet_id: 0x9e08,
            packet_sequence: 2,
            declared_length: 16,
            payload: Vec::new(),
        };
        assert!(envelope.is_message_fragment());
    }

    #[test]
    fn profile_should_decode_word_swapped_mac_and_utf16_nickname() {
        let mut record = [0_u8; PROFILE_BYTES];
        record[..8].copy_from_slice(&[3, 1, 0x24, 0, 0x67, 0xf3, 0x87, 0x41]);
        for (target, unit) in record[8..28].chunks_exact_mut(2).zip("Rób".encode_utf16()) {
            target.copy_from_slice(&unit.to_le_bytes());
        }

        let profile = PictoChatProfile::parse(&record).expect("profile should decode");

        assert_eq!(
            (profile.station, profile.nickname, profile.subtype),
            (
                MacAddress([0, 0x24, 0xf3, 0x67, 0x41, 0x87]),
                "Rób".into(),
                1,
            )
        );
    }

    #[test]
    fn roster_should_decode_captured_slot_layout() {
        let mut payload = [0_u8; ROSTER_BYTES];
        payload[4..16].copy_from_slice(&[
            0x24, 0, 0x67, 0xf3, 0x87, 0x41, 0x25, 0, 0xdc, 0xa0, 0xdf, 0x81,
        ]);

        let roster = PictoChatRoster::parse(&payload).expect("roster should decode");

        assert_eq!(
            &roster.members[..3],
            &[
                Some(MacAddress([0, 0x24, 0xf3, 0x67, 0x41, 0x87])),
                Some(MacAddress([0, 0x25, 0xa0, 0xdc, 0x81, 0xdf])),
                None,
            ]
        );
    }

    #[test]
    fn profile_should_reject_an_unknown_record_marker() {
        let record = [0_u8; PROFILE_BYTES];

        let error = PictoChatProfile::parse(&record).expect_err("marker should be required");

        assert_eq!(
            error,
            DecodeError::InvalidProfile("identity record marker is not 3")
        );
    }

    #[test]
    fn host_132_byte_final_fragment_should_encode_e48_packet_id() {
        let source = MacAddress([0, 9, 0xbf, 1, 2, 3]);
        let fragment = PictoChatFragment {
            opaque_header: [1, 4],
            final_fragment: true,
            data_offset: 4_000,
            opaque_id: [0, 0],
            data: vec![0; 132],
            frame_number: 25,
            host_trailer: Some(2),
        };
        let bytes = build_fragment_frame(
            NintendoFlow::HostToClient,
            source,
            source,
            1,
            false,
            &fragment,
        )
        .expect("fragment should encode");
        let frame = MacFrame::parse(&bytes, FcsMode::Absent).expect("frame should parse");
        let data = decode_nintendo_data(&frame).expect("Nintendo data should decode");
        let envelope =
            PictoChatEnvelope::parse(data.flow, &data.payload).expect("envelope should decode");
        assert_eq!(envelope.packet_id, 0x1e48);
    }

    #[test]
    fn reassembler_should_reset_when_identical_start_arrives_with_new_frame_number() {
        let source = MacAddress([0, 9, 0xbf, 1, 2, 3]);
        let key = ReassemblyKey {
            source,
            opaque_id: [0, 0],
        };
        let old = fragment_payload(&[vec![1; 160], vec![2; 160]].concat(), [0, 0], 160)
            .expect("old transfer should fragment");
        let mut new = fragment_payload(&[vec![1; 160], vec![3; 160]].concat(), [0, 0], 160)
            .expect("new transfer should fragment");
        for fragment in &mut new {
            fragment.frame_number += 10;
        }
        let mut reassembler = MessageReassembler::new(160);
        reassembler
            .push(key, &old[0])
            .expect("old start should buffer");
        let mut completed = None;
        for fragment in &new {
            completed = reassembler
                .push(key, fragment)
                .expect("new transfer should replace old transfer")
                .or(completed);
        }
        assert_eq!(completed, Some([vec![1; 160], vec![3; 160]].concat()));
    }

    #[test]
    fn client_frame_should_use_to_ds_address_layout() {
        let source = MacAddress([0, 9, 0xbf, 1, 2, 3]);
        let bssid = MacAddress([0, 9, 0xbf, 4, 5, 6]);
        let fragment = fragment_payload(b"ping", [1, 0], 160)
            .expect("fixture should fragment")
            .remove(0);
        let bytes = build_fragment_frame(
            NintendoFlow::ClientToHost,
            source,
            bssid,
            42,
            false,
            &fragment,
        )
        .expect("fixture should encode");
        let frame = MacFrame::parse(&bytes, FcsMode::Absent).expect("fixture should parse");
        let data = decode_nintendo_data(&frame).expect("Nintendo flow should decode");
        assert!(frame.header.frame_control.to_ds());
        assert_eq!(frame.header.address_1, bssid);
        assert_eq!(frame.header.address_3, Some(CLIENT_REPLY));
        assert_eq!(data.sequence_number, 42);
        let envelope =
            PictoChatEnvelope::parse(data.flow, &data.payload).expect("envelope should decode");
        assert_eq!(envelope.packet_id, 0x8e08);
    }
}
