use crate::{
    BROADCAST, DecodeError, FrameType, MacAddress, MacFrame, build_management_frame,
    frame::{
        ASSOCIATION_SLOT_MASK, BASIC_RATES_1_2_MBPS, CAPABILITY_ESS_SHORT_PREAMBLE,
        ELEMENT_SUPPORTED_RATES, HEADER_LEN, SUBTYPE_ASSOCIATION_REQUEST,
        SUBTYPE_ASSOCIATION_RESPONSE, SUBTYPE_AUTHENTICATION, SUBTYPE_BEACON,
    },
    read_u16, read_u32, too_short,
};

// ---- Information elements --------------------------------------------------

/// Octets preceding an element's data: identifier and length.
const ELEMENT_HEADER_LEN: usize = 2;
/// Information element: DS parameter set (current channel).
const ELEMENT_DS_PARAMETER: u8 = 3;
/// Information element: traffic indication map.
const ELEMENT_TIM: u8 = 5;
/// Information element: vendor specific.
const ELEMENT_VENDOR: u8 = 0xdd;
/// TIM a DS host sends: DTIM count 1, DTIM period 2, no buffered traffic.
const HOST_TIM: [u8; 5] = [1, 2, 0, 0, 0];

// ---- Management frame bodies -----------------------------------------------

/// Beacon body: octets in the timestamp.
const BEACON_TIMESTAMP_LEN: usize = 8;
/// Beacon body: offset of the beacon interval.
const BEACON_INTERVAL_OFFSET: usize = BEACON_TIMESTAMP_LEN;
/// Beacon body: offset of the capability flags.
const BEACON_CAPABILITIES_OFFSET: usize = BEACON_INTERVAL_OFFSET + 2;
/// Beacon body: octets before the information elements.
const BEACON_FIXED_LEN: usize = BEACON_CAPABILITIES_OFFSET + 2;

/// Authentication body: offset of the algorithm.
const AUTHENTICATION_ALGORITHM_OFFSET: usize = 0;
/// Authentication body: offset of the transaction sequence number.
const AUTHENTICATION_SEQUENCE_OFFSET: usize = 2;
/// Authentication body: offset of the status code.
const AUTHENTICATION_STATUS_OFFSET: usize = 4;
/// Authentication body: octets before the information elements.
const AUTHENTICATION_FIXED_LEN: usize = 6;

/// Association request: offset of the capability flags.
const ASSOCIATION_REQUEST_CAPABILITIES_OFFSET: usize = 0;
/// Association request: offset of the listen interval.
const ASSOCIATION_REQUEST_LISTEN_INTERVAL_OFFSET: usize = 2;
/// Association request: octets before the information elements.
const ASSOCIATION_REQUEST_FIXED_LEN: usize = 4;

/// Association response: offset of the capability flags.
const ASSOCIATION_RESPONSE_CAPABILITIES_OFFSET: usize = 0;
/// Association response: offset of the status code.
const ASSOCIATION_RESPONSE_STATUS_OFFSET: usize = 2;
/// Association response: offset of the association ID.
const ASSOCIATION_RESPONSE_ID_OFFSET: usize = 4;
/// Association response: octets before the information elements.
const ASSOCIATION_RESPONSE_FIXED_LEN: usize = 6;

// ---- Nintendo vendor element -----------------------------------------------

/// Nintendo's OUI (00:09:bf) followed by vendor type zero.
const NINTENDO_VENDOR_ID: [u8; 4] = [0x00, 0x09, 0xbf, 0x00];
/// Offset of the stepping offset.
const NINTENDO_STEPPING_OFFSET: usize = NINTENDO_VENDOR_ID.len();
/// Offset of the video synchronization value.
const NINTENDO_VIDEO_SYNC_OFFSET: usize = 6;
/// Offset of the fixed identifier.
const NINTENDO_FIXED_ID_OFFSET: usize = 8;
/// Offset of the game identifier.
const NINTENDO_GAME_ID_OFFSET: usize = 12;
/// Offset of the stream code.
const NINTENDO_STREAM_CODE_OFFSET: usize = 16;
/// Offset of the custom payload length.
const NINTENDO_CUSTOM_LENGTH_OFFSET: usize = 18;
/// Offset of the beacon type.
const NINTENDO_BEACON_TYPE_OFFSET: usize = 19;
/// Offset of the host command payload capacity.
const NINTENDO_COMMAND_SIZE_OFFSET: usize = 20;
/// Offset of the client reply payload capacity.
const NINTENDO_REPLY_SIZE_OFFSET: usize = 22;
/// Octets in the fixed header, before the custom payload.
const NINTENDO_HEADER_LEN: usize = 24;

/// Beacon type used by multicart sessions and `PictoChat`.
const NINTENDO_BEACON_TYPE_MULTICART: u8 = 1;

// ---- PictoChat room payload ------------------------------------------------

/// Octets in the `PictoChat` custom payload.
const PICTOCHAT_PAYLOAD_LEN: u8 = 8;
/// Offset of the rotating opaque state.
const PICTOCHAT_OPAQUE_STATE_OFFSET: usize = 0;
/// Offset of the room number.
const PICTOCHAT_ROOM_OFFSET: usize = 4;
/// Offset of the user count.
const PICTOCHAT_USER_COUNT_OFFSET: usize = 5;
/// Offset of the fixed suffix.
const PICTOCHAT_SUFFIX_OFFSET: usize = 6;
/// Highest room number: rooms A through D are zero through three.
const PICTOCHAT_MAX_ROOM: u8 = 3;

// ---- Values a DS PictoChat host advertises ---------------------------------

/// Beacon interval, in time units of 1024 microseconds.
const HOST_BEACON_INTERVAL: u16 = 100;
/// Stepping offset observed in real host beacons.
const HOST_STEPPING_OFFSET: u16 = 10;
/// Video synchronization value in the template; the radio does not update it.
const HOST_VIDEO_SYNC: u16 = 0;
/// Fixed identifier observed in real host beacons.
const HOST_FIXED_ID: u32 = 0x0001_0001;
/// Command and reply payload capacity a host advertises, in bytes.
const HOST_DATA_SIZE: u16 = 192;
/// Fixed suffix observed at the end of the room payload.
const PICTOCHAT_FIXED_SUFFIX: u16 = 0x0004;

/// A losslessly decoded IEEE 802.11 information element.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InformationElement {
    /// Element identifier.
    pub id: u8,
    /// Opaque element contents.
    pub data: Vec<u8>,
}

/// Fixed and tagged fields from an IEEE 802.11 beacon.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Beacon {
    /// Host timestamp in microseconds.
    pub timestamp: u64,
    /// Beacon interval in time units.
    pub interval: u16,
    /// Capability flags.
    pub capabilities: u16,
    /// Tagged information elements in their original order.
    pub elements: Vec<InformationElement>,
}

/// Open-system/shared-key authentication body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Authentication {
    /// Authentication algorithm identifier.
    pub algorithm: u16,
    /// Authentication exchange sequence number.
    pub sequence: u16,
    /// IEEE status code.
    pub status: u16,
    /// Optional tagged fields.
    pub elements: Vec<InformationElement>,
}

/// IEEE 802.11 association request body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssociationRequest {
    /// Requested capability flags.
    pub capabilities: u16,
    /// Beacon listen interval.
    pub listen_interval: u16,
    /// Tagged fields, including the Nintendo binary SSID when present.
    pub elements: Vec<InformationElement>,
}

/// IEEE 802.11 association response body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssociationResponse {
    /// Accepted capability flags.
    pub capabilities: u16,
    /// IEEE status code; zero indicates success.
    pub status: u16,
    /// Association identifier. Nintendo local multiplayer uses its low nibble as a slot.
    pub association_id: u16,
    /// Optional tagged fields.
    pub elements: Vec<InformationElement>,
}

impl AssociationResponse {
    /// Returns the Nintendo local-multiplayer participant slot.
    #[must_use]
    pub const fn participant_slot(&self) -> u8 {
        (self.association_id & ASSOCIATION_SLOT_MASK) as u8
    }
}

/// Typed management body for the frame subtypes relevant to local multiplayer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ManagementBody {
    /// Beacon announcement.
    Beacon(Beacon),
    /// Authentication request or response.
    Authentication(Authentication),
    /// Association request.
    AssociationRequest(AssociationRequest),
    /// Association response.
    AssociationResponse(AssociationResponse),
    /// Other management subtype preserved as opaque bytes.
    Unknown {
        /// Four-bit IEEE subtype.
        subtype: u8,
        /// Original body bytes.
        body: Vec<u8>,
    },
}

/// Verified Nintendo vendor information from a beacon.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NintendoBeacon {
    /// Stepping offset used by Nintendo's post-beacon window.
    pub stepping_offset: u16,
    /// LCD/video synchronization value.
    pub video_sync: u16,
    /// Fixed or title-specific identifier retained verbatim.
    pub fixed_id: u32,
    /// Game/application identifier.
    pub game_id: u32,
    /// Random stream code used by the session.
    pub stream_code: u16,
    /// Declared custom payload length.
    pub custom_length: u8,
    /// Nintendo beacon type; one denotes multicart or `PictoChat`.
    pub beacon_type: u8,
    /// Host command payload capacity.
    pub command_data_size: u16,
    /// Client reply payload capacity.
    pub reply_data_size: u16,
    /// Custom bytes following the fixed Nintendo header.
    pub custom_data: Vec<u8>,
    /// Entire vendor element, retained for byte-level investigations.
    pub raw: Vec<u8>,
}

impl NintendoBeacon {
    /// Extracts Nintendo beacon information from an IEEE beacon.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the vendor element is absent or malformed.
    pub fn from_beacon(beacon: &Beacon) -> Result<Self, DecodeError> {
        let element = beacon
            .elements
            .iter()
            .find(|element| {
                element.id == ELEMENT_VENDOR && element.data.starts_with(&NINTENDO_VENDOR_ID)
            })
            .ok_or(DecodeError::MissingNintendoBeacon)?;
        Self::parse(&element.data)
    }

    /// Parses the contents of a Nintendo vendor information element.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] for truncated or inconsistent data.
    pub fn parse(data: &[u8]) -> Result<Self, DecodeError> {
        if data.len() < NINTENDO_HEADER_LEN {
            return Err(DecodeError::InvalidNintendoBeacon(
                "fixed header is shorter than 24 bytes",
            ));
        }
        if !data.starts_with(&NINTENDO_VENDOR_ID) {
            return Err(DecodeError::InvalidNintendoBeacon(
                "vendor OUI/type does not match Nintendo",
            ));
        }
        let custom_length = data[NINTENDO_CUSTOM_LENGTH_OFFSET];
        let custom_end = NINTENDO_HEADER_LEN + usize::from(custom_length);
        if data.len() < custom_end {
            return Err(DecodeError::InvalidNintendoBeacon(
                "custom payload is shorter than declared",
            ));
        }
        Ok(Self {
            stepping_offset: read_u16(data, NINTENDO_STEPPING_OFFSET, "Nintendo beacon")?,
            video_sync: read_u16(data, NINTENDO_VIDEO_SYNC_OFFSET, "Nintendo beacon")?,
            fixed_id: read_u32(data, NINTENDO_FIXED_ID_OFFSET, "Nintendo beacon")?,
            game_id: read_u32(data, NINTENDO_GAME_ID_OFFSET, "Nintendo beacon")?,
            stream_code: read_u16(data, NINTENDO_STREAM_CODE_OFFSET, "Nintendo beacon")?,
            custom_length,
            beacon_type: data[NINTENDO_BEACON_TYPE_OFFSET],
            command_data_size: read_u16(data, NINTENDO_COMMAND_SIZE_OFFSET, "Nintendo beacon")?,
            reply_data_size: read_u16(data, NINTENDO_REPLY_SIZE_OFFSET, "Nintendo beacon")?,
            custom_data: data[NINTENDO_HEADER_LEN..custom_end].to_vec(),
            raw: data.to_vec(),
        })
    }

    /// Decodes the eight-byte `PictoChat` custom payload when present.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] when a type-one beacon carries malformed room data.
    pub fn pictochat(&self) -> Result<Option<PictoChatBeacon>, DecodeError> {
        if self.beacon_type != NINTENDO_BEACON_TYPE_MULTICART
            || self.custom_length != PICTOCHAT_PAYLOAD_LEN
        {
            return Ok(None);
        }
        if self.custom_data.len() != usize::from(PICTOCHAT_PAYLOAD_LEN) {
            return Err(DecodeError::InvalidNintendoBeacon(
                "PictoChat payload is not eight bytes",
            ));
        }
        let room = self.custom_data[PICTOCHAT_ROOM_OFFSET];
        if room > PICTOCHAT_MAX_ROOM {
            return Err(DecodeError::InvalidRoom(room));
        }
        Ok(Some(PictoChatBeacon {
            opaque_state: read_u32(
                &self.custom_data,
                PICTOCHAT_OPAQUE_STATE_OFFSET,
                "PictoChat beacon",
            )?,
            room,
            user_count: self.custom_data[PICTOCHAT_USER_COUNT_OFFSET],
            fixed_suffix: read_u16(
                &self.custom_data,
                PICTOCHAT_SUFFIX_OFFSET,
                "PictoChat beacon",
            )?,
        }))
    }
}

/// The eight-byte room payload advertised by a `PictoChat` host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PictoChatBeacon {
    /// Opaque value that rotates while the room and membership remain unchanged.
    pub opaque_state: u32,
    /// Room number: zero through three correspond to rooms A through D.
    pub room: u8,
    /// Connected users including the host.
    pub user_count: u8,
    /// Observed fixed value `0x0004`.
    pub fixed_suffix: u16,
}

/// Values that vary between `PictoChat` room beacon transmissions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PictoChatBeaconConfig {
    /// Host radio address used as both transmitter and BSSID.
    pub host: MacAddress,
    /// Host clock value carried in the IEEE beacon body.
    pub timestamp: u64,
    /// Twelve-bit IEEE sequence number.
    pub sequence_number: u16,
    /// Current Nintendo application identifier copied by joining clients.
    pub game_id: u32,
    /// Current Nintendo session stream code copied by joining clients.
    pub stream_code: u16,
    /// Rotating room-state value whose semantics remain unknown.
    pub opaque_state: u32,
    /// Room number from zero (A) through three (D).
    pub room: u8,
    /// Connected users including the host.
    pub user_count: u8,
    /// 2.4 GHz channel advertised in the DS parameter element.
    pub channel: u8,
}

/// Beacon template sections supplied to a Linux AP interface.
///
/// The Wi-Fi hardware inserts the TIM element between these sections and replaces
/// the zero timestamp in `head` with its TSF clock when each beacon is transmitted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PictoChatBeaconTemplate {
    /// IEEE management header, fixed beacon fields, rates, and channel element.
    pub head: Vec<u8>,
    /// Nintendo vendor element placed after the hardware-generated TIM element.
    pub tail: Vec<u8>,
}

/// Builds the management frame advertised by a software `PictoChat` host.
///
/// Constants reproduce the recent real-DS beacon shape: 1 and 2 Mbps basic rates,
/// Nintendo type one, 192-byte command/reply capacities, and the observed room suffix.
///
/// # Errors
///
/// Returns [`DecodeError::InvalidRoom`] when `config.room` is outside A through D.
pub fn build_pictochat_beacon(config: PictoChatBeaconConfig) -> Result<Vec<u8>, DecodeError> {
    let timestamp = config.timestamp;
    let mut template = build_pictochat_beacon_template(config)?;
    template.head[HEADER_LEN..HEADER_LEN + BEACON_TIMESTAMP_LEN]
        .copy_from_slice(&timestamp.to_le_bytes());
    let mut frame = template.head;
    append_element(&mut frame, ELEMENT_TIM, &HOST_TIM)?;
    frame.extend_from_slice(&template.tail);
    Ok(frame)
}

/// Builds the head and tail of a hardware-generated `PictoChat` beacon.
///
/// Linux's AP API supplies the TIM element between these sections. The timestamp
/// is zero so ath9k can replace it with the adapter's TSF clock.
///
/// # Errors
///
/// Returns [`DecodeError::InvalidRoom`] when `config.room` is outside A through D.
pub fn build_pictochat_beacon_template(
    mut config: PictoChatBeaconConfig,
) -> Result<PictoChatBeaconTemplate, DecodeError> {
    if config.room > PICTOCHAT_MAX_ROOM {
        return Err(DecodeError::InvalidRoom(config.room));
    }
    // The radio overwrites the timestamp with its own clock on transmission.
    config.timestamp = 0;

    let mut body = Vec::new();
    body.extend_from_slice(&config.timestamp.to_le_bytes());
    body.extend_from_slice(&HOST_BEACON_INTERVAL.to_le_bytes());
    body.extend_from_slice(&CAPABILITY_ESS_SHORT_PREAMBLE.to_le_bytes());
    append_element(&mut body, ELEMENT_SUPPORTED_RATES, &BASIC_RATES_1_2_MBPS)?;
    append_element(&mut body, ELEMENT_DS_PARAMETER, &[config.channel])?;

    // Field order matches the NINTENDO_*_OFFSET and PICTOCHAT_*_OFFSET layouts.
    let mut nintendo = Vec::new();
    nintendo.extend_from_slice(&NINTENDO_VENDOR_ID);
    nintendo.extend_from_slice(&HOST_STEPPING_OFFSET.to_le_bytes());
    nintendo.extend_from_slice(&HOST_VIDEO_SYNC.to_le_bytes());
    nintendo.extend_from_slice(&HOST_FIXED_ID.to_le_bytes());
    nintendo.extend_from_slice(&config.game_id.to_le_bytes());
    nintendo.extend_from_slice(&config.stream_code.to_le_bytes());
    nintendo.extend_from_slice(&[PICTOCHAT_PAYLOAD_LEN, NINTENDO_BEACON_TYPE_MULTICART]);
    nintendo.extend_from_slice(&HOST_DATA_SIZE.to_le_bytes());
    nintendo.extend_from_slice(&HOST_DATA_SIZE.to_le_bytes());
    nintendo.extend_from_slice(&config.opaque_state.to_le_bytes());
    nintendo.extend_from_slice(&[config.room, config.user_count]);
    nintendo.extend_from_slice(&PICTOCHAT_FIXED_SUFFIX.to_le_bytes());
    let mut tail = Vec::new();
    append_element(&mut tail, ELEMENT_VENDOR, &nintendo)?;

    Ok(PictoChatBeaconTemplate {
        head: build_management_frame(
            SUBTYPE_BEACON,
            BROADCAST,
            config.host,
            config.host,
            config.sequence_number,
            &body,
        ),
        tail,
    })
}

fn append_element(output: &mut Vec<u8>, id: u8, data: &[u8]) -> Result<(), DecodeError> {
    let length = u8::try_from(data.len()).map_err(|_| {
        DecodeError::InvalidNintendoBeacon("encoded information element exceeds 255 bytes")
    })?;
    output.push(id);
    output.push(length);
    output.extend_from_slice(data);
    Ok(())
}

/// Decodes the body of a management frame.
///
/// # Errors
///
/// Returns [`DecodeError`] when a known management subtype is truncated.
pub fn parse_management(frame: &MacFrame) -> Result<ManagementBody, DecodeError> {
    if frame.header.frame_control.frame_type() != FrameType::Management {
        return Err(DecodeError::NotManagementFrame);
    }
    let body = frame.body();
    match frame.header.frame_control.subtype() {
        SUBTYPE_BEACON => parse_beacon(body).map(ManagementBody::Beacon),
        SUBTYPE_AUTHENTICATION => parse_authentication(body).map(ManagementBody::Authentication),
        SUBTYPE_ASSOCIATION_REQUEST => {
            parse_association_request(body).map(ManagementBody::AssociationRequest)
        }
        SUBTYPE_ASSOCIATION_RESPONSE => {
            parse_association_response(body).map(ManagementBody::AssociationResponse)
        }
        subtype => Ok(ManagementBody::Unknown {
            subtype,
            body: body.to_vec(),
        }),
    }
}

fn parse_beacon(body: &[u8]) -> Result<Beacon, DecodeError> {
    const CONTEXT: &str = "IEEE 802.11 beacon body";
    let timestamp = body
        .get(..BEACON_TIMESTAMP_LEN)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u64::from_le_bytes)
        .filter(|_| body.len() >= BEACON_FIXED_LEN)
        .ok_or_else(|| too_short(CONTEXT, BEACON_FIXED_LEN, body.len()))?;
    Ok(Beacon {
        timestamp,
        interval: read_u16(body, BEACON_INTERVAL_OFFSET, CONTEXT)?,
        capabilities: read_u16(body, BEACON_CAPABILITIES_OFFSET, CONTEXT)?,
        elements: parse_elements(&body[BEACON_FIXED_LEN..], BEACON_FIXED_LEN)?,
    })
}

fn parse_authentication(body: &[u8]) -> Result<Authentication, DecodeError> {
    const CONTEXT: &str = "IEEE 802.11 authentication body";
    if body.len() < AUTHENTICATION_FIXED_LEN {
        return Err(too_short(CONTEXT, AUTHENTICATION_FIXED_LEN, body.len()));
    }
    Ok(Authentication {
        algorithm: read_u16(body, AUTHENTICATION_ALGORITHM_OFFSET, CONTEXT)?,
        sequence: read_u16(body, AUTHENTICATION_SEQUENCE_OFFSET, CONTEXT)?,
        status: read_u16(body, AUTHENTICATION_STATUS_OFFSET, CONTEXT)?,
        elements: parse_elements(&body[AUTHENTICATION_FIXED_LEN..], AUTHENTICATION_FIXED_LEN)?,
    })
}

fn parse_association_request(body: &[u8]) -> Result<AssociationRequest, DecodeError> {
    const CONTEXT: &str = "IEEE 802.11 association request";
    if body.len() < ASSOCIATION_REQUEST_FIXED_LEN {
        return Err(too_short(
            CONTEXT,
            ASSOCIATION_REQUEST_FIXED_LEN,
            body.len(),
        ));
    }
    Ok(AssociationRequest {
        capabilities: read_u16(body, ASSOCIATION_REQUEST_CAPABILITIES_OFFSET, CONTEXT)?,
        listen_interval: read_u16(body, ASSOCIATION_REQUEST_LISTEN_INTERVAL_OFFSET, CONTEXT)?,
        elements: parse_elements(
            &body[ASSOCIATION_REQUEST_FIXED_LEN..],
            ASSOCIATION_REQUEST_FIXED_LEN,
        )?,
    })
}

fn parse_association_response(body: &[u8]) -> Result<AssociationResponse, DecodeError> {
    const CONTEXT: &str = "IEEE 802.11 association response";
    if body.len() < ASSOCIATION_RESPONSE_FIXED_LEN {
        return Err(too_short(
            CONTEXT,
            ASSOCIATION_RESPONSE_FIXED_LEN,
            body.len(),
        ));
    }
    Ok(AssociationResponse {
        capabilities: read_u16(body, ASSOCIATION_RESPONSE_CAPABILITIES_OFFSET, CONTEXT)?,
        status: read_u16(body, ASSOCIATION_RESPONSE_STATUS_OFFSET, CONTEXT)?,
        association_id: read_u16(body, ASSOCIATION_RESPONSE_ID_OFFSET, CONTEXT)?,
        elements: parse_elements(
            &body[ASSOCIATION_RESPONSE_FIXED_LEN..],
            ASSOCIATION_RESPONSE_FIXED_LEN,
        )?,
    })
}

fn parse_elements(data: &[u8], base_offset: usize) -> Result<Vec<InformationElement>, DecodeError> {
    let mut elements = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        if data.len() - offset < ELEMENT_HEADER_LEN {
            return Err(DecodeError::TruncatedInformationElement {
                offset: base_offset + offset,
                declared: 0,
                remaining: data.len() - offset,
            });
        }
        let id = data[offset];
        let length = usize::from(data[offset + 1]);
        let remaining = data.len() - offset - ELEMENT_HEADER_LEN;
        if length > remaining {
            return Err(DecodeError::TruncatedInformationElement {
                offset: base_offset + offset,
                declared: length,
                remaining,
            });
        }
        elements.push(InformationElement {
            id,
            data: data[offset + ELEMENT_HEADER_LEN..offset + ELEMENT_HEADER_LEN + length].to_vec(),
        });
        offset += ELEMENT_HEADER_LEN + length;
    }
    Ok(elements)
}
