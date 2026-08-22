use crate::{DecodeError, FrameType, MacFrame, read_u16, read_u32, too_short};

const NINTENDO_VENDOR_ID: [u8; 4] = [0x00, 0x09, 0xbf, 0x00];

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
        (self.association_id & 0x0f) as u8
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
            .find(|element| element.id == 0xdd && element.data.starts_with(&NINTENDO_VENDOR_ID))
            .ok_or(DecodeError::MissingNintendoBeacon)?;
        Self::parse(&element.data)
    }

    /// Parses the contents of a Nintendo vendor information element.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] for truncated or inconsistent data.
    pub fn parse(data: &[u8]) -> Result<Self, DecodeError> {
        if data.len() < 24 {
            return Err(DecodeError::InvalidNintendoBeacon(
                "fixed header is shorter than 24 bytes",
            ));
        }
        if !data.starts_with(&NINTENDO_VENDOR_ID) {
            return Err(DecodeError::InvalidNintendoBeacon(
                "vendor OUI/type does not match Nintendo",
            ));
        }
        let custom_length = data[18];
        let custom_end = 24 + usize::from(custom_length);
        if data.len() < custom_end {
            return Err(DecodeError::InvalidNintendoBeacon(
                "custom payload is shorter than declared",
            ));
        }
        Ok(Self {
            stepping_offset: read_u16(data, 4, "Nintendo beacon")?,
            video_sync: read_u16(data, 6, "Nintendo beacon")?,
            fixed_id: read_u32(data, 8, "Nintendo beacon")?,
            game_id: read_u32(data, 12, "Nintendo beacon")?,
            stream_code: read_u16(data, 16, "Nintendo beacon")?,
            custom_length,
            beacon_type: data[19],
            command_data_size: read_u16(data, 20, "Nintendo beacon")?,
            reply_data_size: read_u16(data, 22, "Nintendo beacon")?,
            custom_data: data[24..custom_end].to_vec(),
            raw: data.to_vec(),
        })
    }

    /// Decodes the eight-byte `PictoChat` custom payload when present.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] when a type-one beacon carries malformed room data.
    pub fn pictochat(&self) -> Result<Option<PictoChatBeacon>, DecodeError> {
        if self.beacon_type != 1 || self.custom_length != 8 {
            return Ok(None);
        }
        if self.custom_data.len() != 8 {
            return Err(DecodeError::InvalidNintendoBeacon(
                "PictoChat payload is not eight bytes",
            ));
        }
        let room = self.custom_data[4];
        if room > 3 {
            return Err(DecodeError::InvalidRoom(room));
        }
        Ok(Some(PictoChatBeacon {
            opaque_state: read_u32(&self.custom_data, 0, "PictoChat beacon")?,
            room,
            user_count: self.custom_data[5],
            fixed_suffix: read_u16(&self.custom_data, 6, "PictoChat beacon")?,
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
        8 => parse_beacon(body).map(ManagementBody::Beacon),
        11 => parse_authentication(body).map(ManagementBody::Authentication),
        0 => parse_association_request(body).map(ManagementBody::AssociationRequest),
        1 => parse_association_response(body).map(ManagementBody::AssociationResponse),
        subtype => Ok(ManagementBody::Unknown {
            subtype,
            body: body.to_vec(),
        }),
    }
}

fn parse_beacon(body: &[u8]) -> Result<Beacon, DecodeError> {
    if body.len() < 12 {
        return Err(too_short("IEEE 802.11 beacon body", 12, body.len()));
    }
    let timestamp = u64::from_le_bytes([
        body[0], body[1], body[2], body[3], body[4], body[5], body[6], body[7],
    ]);
    Ok(Beacon {
        timestamp,
        interval: read_u16(body, 8, "IEEE 802.11 beacon body")?,
        capabilities: read_u16(body, 10, "IEEE 802.11 beacon body")?,
        elements: parse_elements(&body[12..], 12)?,
    })
}

fn parse_authentication(body: &[u8]) -> Result<Authentication, DecodeError> {
    if body.len() < 6 {
        return Err(too_short("IEEE 802.11 authentication body", 6, body.len()));
    }
    Ok(Authentication {
        algorithm: read_u16(body, 0, "IEEE 802.11 authentication body")?,
        sequence: read_u16(body, 2, "IEEE 802.11 authentication body")?,
        status: read_u16(body, 4, "IEEE 802.11 authentication body")?,
        elements: parse_elements(&body[6..], 6)?,
    })
}

fn parse_association_request(body: &[u8]) -> Result<AssociationRequest, DecodeError> {
    if body.len() < 4 {
        return Err(too_short("IEEE 802.11 association request", 4, body.len()));
    }
    Ok(AssociationRequest {
        capabilities: read_u16(body, 0, "IEEE 802.11 association request")?,
        listen_interval: read_u16(body, 2, "IEEE 802.11 association request")?,
        elements: parse_elements(&body[4..], 4)?,
    })
}

fn parse_association_response(body: &[u8]) -> Result<AssociationResponse, DecodeError> {
    if body.len() < 6 {
        return Err(too_short("IEEE 802.11 association response", 6, body.len()));
    }
    Ok(AssociationResponse {
        capabilities: read_u16(body, 0, "IEEE 802.11 association response")?,
        status: read_u16(body, 2, "IEEE 802.11 association response")?,
        association_id: read_u16(body, 4, "IEEE 802.11 association response")?,
        elements: parse_elements(&body[6..], 6)?,
    })
}

fn parse_elements(data: &[u8], base_offset: usize) -> Result<Vec<InformationElement>, DecodeError> {
    let mut elements = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        if data.len() - offset < 2 {
            return Err(DecodeError::TruncatedInformationElement {
                offset: base_offset + offset,
                declared: 0,
                remaining: data.len() - offset,
            });
        }
        let id = data[offset];
        let length = usize::from(data[offset + 1]);
        let remaining = data.len() - offset - 2;
        if length > remaining {
            return Err(DecodeError::TruncatedInformationElement {
                offset: base_offset + offset,
                declared: length,
                remaining,
            });
        }
        elements.push(InformationElement {
            id,
            data: data[offset + 2..offset + 2 + length].to_vec(),
        });
        offset += 2 + length;
    }
    Ok(elements)
}
