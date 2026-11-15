//! Commands and events shared by the ESP32 worker and API broker.
use picto_relay_codec::{MAC_ADDRESS_LEN, MacAddress};

/// Default USB serial device of the ESP32 radio.
pub const DEFAULT_SERIAL_PORT: &str = "/dev/cu.usbserial-0001";

/// Default preferred 2.4 GHz Wi-Fi channel, scanned first during discovery.
pub const DEFAULT_WIFI_CHANNEL: u8 = 1;

/// Placeholder station address used until the USB radio reports its own MAC.
const UNASSIGNED_STATION: MacAddress = MacAddress([0; MAC_ADDRESS_LEN]);

#[derive(Clone, Debug)]
pub(crate) struct Esp32Config {
    pub port: String,
    pub channel: u8,
    pub station: MacAddress,
}
impl Default for Esp32Config {
    fn default() -> Self {
        Self {
            port: DEFAULT_SERIAL_PORT.into(),
            channel: DEFAULT_WIFI_CHANNEL,
            station: UNASSIGNED_STATION,
        }
    }
}

/// Commands accepted by a live physical station worker.
#[derive(Debug)]
pub enum LiveCommand {
    /// Queue one API-format 256x96 packed monochrome or indexed color drawing.
    SendMessage { message_id: u32, bitmap: Vec<u8> },
    /// Leave the room and stop the worker.
    Shutdown,
}

/// One station advertised by the active physical `PictoChat` room.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveParticipant {
    /// MAC address carried by the host's member/session roster.
    pub station: MacAddress,
    /// Profile nickname when observed, otherwise a stable MAC-derived label.
    pub name: String,
}

/// Events emitted by a live physical station worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LiveEvent {
    /// Full profile observed in the physical room.
    Profile(picto_relay_codec::PictoChatProfile),
    /// Physical station address supplied by a USB radio.
    Station { station: MacAddress },
    /// Human-readable progress for the API client and logs.
    State { state: &'static str, detail: String },
    /// The profile handshake completed and the virtual nickname is live.
    Ready {
        host: MacAddress,
        room: u8,
        slot: u8,
    },
    /// Replaces the known physical participant roster for the active room.
    Participants {
        room: u8,
        participants: Vec<LiveParticipant>,
    },
    /// A drawing was rebroadcast by the physical host.
    Message {
        station_id: u32,
        name: String,
        message_id: u32,
        bitmap: Vec<u8>,
    },
    /// The physical host rebroadcast the submitted drawing.
    Delivered { message_id: u32 },
    /// The physical host did not rebroadcast the drawing before the deadline.
    DeliveryTimedOut { message_id: u32 },
    /// A recoverable live-backend command failed.
    Error { code: &'static str, message: String },
    /// The station deauthenticated and released its participant slot.
    Left,
}

/// Personal profile options passed to the serial worker.
#[derive(Clone, Debug, Default)]
pub struct ProfileOptions {
    /// UTF-16 personal message (at most 26 code units).
    pub message: String,
    /// DS profile color index (0–15).
    pub color: u8,
}
