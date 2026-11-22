//! The host's side of a Picto Relay session as a WebAssembly module.
//!
//! A web page that drives the ESP32 over Web Serial reads and writes the port
//! and keeps time itself. For everything else it calls this module, which is
//! the code the relay server runs: packet framing from `picto-relay-link`,
//! and the room and drawings from `picto-relay-session`.
//!
//! The interface needs no generated bindings and no shared memory. The page
//! gives bytes one at a time ([`input_push`]), calls an operation, and takes
//! the answer one byte at a time ([`output_at`]). An operation returns 1 when
//! it worked. When it returns 0 the answer is a sentence saying why not.
//! Structured answers are JSON. At these sizes the cost of a call per byte
//! does not matter: a drawing is 12 KB and the serial line carries 92 KB a
//! second at most.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use picto_relay_codec::MacAddress;
use picto_relay_link as link;
use picto_relay_session::{
    Room, RoomEvent, draw_commands, join_payload, left_text, native_drawing, station_id,
    status_text,
};
use serde::Serialize;
use serde_json::json;
use std::sync::{Mutex, PoisonError};

/// Everything the module remembers between calls. A page drives one ESP32.
pub struct Host {
    input: Vec<u8>,
    output: Vec<u8>,
    decoder: link::Decoder,
    /// Our own radio address, once the firmware has said HELLO back.
    station: Option<MacAddress>,
    /// The room we are joining or in.
    room: Option<Room>,
}

/// What a firmware packet means, in the shapes the web client's session
/// events have where there is one.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
    /// The firmware's periodic report. `message_id` is the drawing being sent, if any.
    Status {
        message_id: u32,
        state: u8,
        phase: u8,
        sent: u8,
        total: u8,
        echoed: u8,
    },
    /// The room let us in.
    Ready {
        client_id: u32,
        slot: u8,
        channel: u8,
    },
    Participants {
        participants: Vec<Participant>,
    },
    Profile {
        station_id: u32,
        name: String,
        profile_message: String,
        profile_color: u16,
    },
    Message {
        station_id: u32,
        name: String,
        message_id: u32,
        palette_bitmap: String,
    },
    /// The host DS rebroadcast our drawing: everyone has it.
    Delivered {
        message_id: u32,
    },
    /// The session ended, and why.
    Left {
        detail: String,
    },
    /// The firmware dropped some of what it heard; drawings part-way in are lost.
    Gap,
    /// Members seen in rooms A to D by a scan. `null` where no room was heard.
    Rooms {
        counts: Vec<Option<u8>>,
    },
}

#[derive(Serialize)]
struct Participant {
    client_id: u32,
    name: String,
}

type Answer = Result<Vec<u8>, String>;

fn json_answer(value: &impl Serialize) -> Answer {
    serde_json::to_vec(value).map_err(|error| error.to_string())
}

impl Host {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            input: Vec::new(),
            output: Vec::new(),
            decoder: link::Decoder::new(),
            station: None,
            room: None,
        }
    }

    /// The numbers of the USB protocol the page needs, by their names in
    /// `picto-relay-link`.
    fn constants() -> Answer {
        json_answer(&json!({
            "PROTOCOL_VERSION": link::PROTOCOL_VERSION,
            "BAUD": link::BAUD,
            "HOST_TIMEOUT_MS": link::HOST_TIMEOUT_MS,
            "HELLO": link::HELLO,
            "JOIN": link::JOIN,
            "LEAVE": link::LEAVE,
            "PING": link::PING,
            "SCAN": link::SCAN,
            "DRAW": link::DRAW,
            "RESPONSE": link::RESPONSE,
            "STATUS_OK": link::STATUS_OK,
            "STATUS_BUSY": link::STATUS_BUSY,
            "STATUS_VERSION_MISMATCH": link::STATUS_VERSION_MISMATCH,
            "STATUS": link::STATUS,
            "RX": link::RX,
            "READY": link::READY,
            "DELIVERED": link::DELIVERED,
            "LEFT": link::LEFT,
            "ROOMS": link::ROOMS,
            "STATUS_LEN": link::STATUS_LEN,
            "LEFT_KICKED": link::LEFT_KICKED,
            "NO_BEACON": link::NO_BEACON,
        }))
    }

    /// Frames the input as a packet for the wire.
    fn encode(&self, kind: u8, id: u32) -> Answer {
        link::encode(kind, id, &self.input).ok_or_else(|| "the command is too large".into())
    }

    /// Takes bytes as they came off the wire. Answers with the packets they
    /// complete, each as kind, ID (u32 LE), payload length (u16 LE), payload.
    fn decode(&mut self) -> Vec<u8> {
        let mut packets = Vec::new();
        for &byte in &self.input {
            let Some(packet) = self.decoder.push(byte) else {
                continue;
            };
            let Ok(length) = u16::try_from(packet.payload.len()) else {
                continue;
            };
            packets.push(packet.kind);
            packets.extend_from_slice(&packet.id.to_le_bytes());
            packets.extend_from_slice(&length.to_le_bytes());
            packets.extend_from_slice(&packet.payload);
        }
        packets
    }

    /// Reads the data of the firmware's answer to HELLO, and remembers our
    /// radio address from it.
    fn hello(&mut self) -> Answer {
        let version = self.input.get(link::HELLO_VERSION_OFFSET).copied();
        let build =
            String::from_utf8_lossy(self.input.get(link::HELLO_BUILD_OFFSET..).unwrap_or(&[]))
                .into_owned();
        let station = self
            .input
            .get(link::HELLO_MAC_OFFSET..link::HELLO_MAC_OFFSET + link::MAC_LEN)
            .and_then(|mac| mac.try_into().ok())
            .map(MacAddress);
        self.station = station;
        json_answer(&json!({
            "version": version,
            "build": build,
            "station": station.map(|mac| mac.to_string()),
            "client_id": station.map(station_id),
        }))
    }

    /// Builds a JOIN payload and starts a room for it. The input is the
    /// nickname, a zero byte, and the profile message, in UTF-8.
    fn join(&mut self, room: u8, color: u8) -> Answer {
        let station = self.station.ok_or("the ESP32 has not said hello")?;
        let text = std::str::from_utf8(&self.input).map_err(|_| "the profile is not text")?;
        let (name, message) = text.split_once('\0').ok_or("the profile is incomplete")?;
        let payload =
            join_payload(room, name, message, color).map_err(|error| error.to_string())?;
        self.room = Some(Room::new(station, name));
        Ok(payload)
    }

    /// Turns a drawing, as an API palette bitmap, into the payloads of the
    /// DRAW commands that upload it: each as length (u16 LE), then payload.
    fn draw(&self, id: u32) -> Answer {
        let station = self.station.ok_or("the ESP32 has not said hello")?;
        let native = native_drawing(station, &self.input).map_err(|error| error.to_string())?;
        let commands = draw_commands(id, &native).map_err(|error| error.to_string())?;
        let mut answer = Vec::new();
        for command in commands {
            let length = u16::try_from(command.len()).map_err(|_| "a command is too large")?;
            answer.extend_from_slice(&length.to_le_bytes());
            answer.extend_from_slice(&command);
        }
        Ok(answer)
    }

    /// Says what a packet from the firmware means: a JSON list of events.
    /// The input is the packet's payload. Responses to commands are the
    /// page's to match up, and are not for this.
    fn packet(&mut self, kind: u8, id: u32) -> Answer {
        json_answer(&self.events(kind, id))
    }

    fn events(&mut self, kind: u8, id: u32) -> Vec<Event> {
        let payload = &self.input;
        match kind {
            link::STATUS if payload.len() == link::STATUS_LEN => vec![Event::Status {
                message_id: id,
                state: payload[link::STATUS_STATE_OFFSET],
                phase: payload[link::STATUS_PHASE_OFFSET],
                sent: payload[link::STATUS_SENT_OFFSET],
                total: payload[link::STATUS_TOTAL_OFFSET],
                echoed: payload[link::STATUS_ECHOED_OFFSET],
            }],
            link::READY if payload.len() == link::READY_LEN => {
                let Some(room) = &mut self.room else {
                    return Vec::new();
                };
                let Ok(host) = payload[link::READY_HOST_OFFSET..][..link::MAC_LEN].try_into()
                else {
                    return Vec::new();
                };
                room.set_host(MacAddress(host));
                vec![Event::Ready {
                    client_id: station_id(room.station()),
                    slot: payload[link::READY_SLOT_OFFSET],
                    channel: payload[link::READY_CHANNEL_OFFSET],
                }]
            }
            link::RX => {
                let Some(room) = &mut self.room else {
                    return Vec::new();
                };
                room.receive(payload).into_iter().map(Event::from).collect()
            }
            link::GAP => {
                if let Some(room) = &mut self.room {
                    room.discard_incoming();
                }
                vec![Event::Gap]
            }
            link::DELIVERED => vec![Event::Delivered { message_id: id }],
            link::LEFT => {
                self.room = None;
                vec![Event::Left {
                    detail: left_text(payload),
                }]
            }
            link::ROOMS if payload.len() == link::ROOM_COUNT => vec![Event::Rooms {
                counts: payload
                    .iter()
                    .map(|&count| (count != link::NO_BEACON).then_some(count))
                    .collect(),
            }],
            _ => Vec::new(),
        }
    }

    /// Runs an operation and keeps its answer for the page to take.
    fn answer(&mut self, operation: impl FnOnce(&mut Self) -> Answer) -> u32 {
        let (ok, output) = match operation(self) {
            Ok(output) => (1, output),
            Err(why) => (0, why.into_bytes()),
        };
        self.output = output;
        self.input.clear();
        ok
    }
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}

impl From<RoomEvent> for Event {
    fn from(event: RoomEvent) -> Self {
        match event {
            RoomEvent::Participants(participants) => Self::Participants {
                participants: participants
                    .into_iter()
                    .map(|participant| Participant {
                        client_id: station_id(participant.station),
                        name: participant.name,
                    })
                    .collect(),
            },
            RoomEvent::Profile(profile) => Self::Profile {
                station_id: station_id(profile.station),
                name: profile.nickname,
                profile_message: profile.message,
                profile_color: profile.color,
            },
            RoomEvent::Message {
                sender,
                name,
                message_id,
                bitmap,
            } => Self::Message {
                station_id: station_id(sender),
                name,
                message_id,
                palette_bitmap: STANDARD.encode(bitmap),
            },
        }
    }
}

static HOST: Mutex<Host> = Mutex::new(Host::new());

fn with_host<T>(run: impl FnOnce(&mut Host) -> T) -> T {
    run(&mut HOST.lock().unwrap_or_else(PoisonError::into_inner))
}

/// A number from the page that must fit in a byte; anything else is refused.
fn byte(value: u32) -> Result<u8, String> {
    u8::try_from(value).map_err(|_| format!("{value} is not a byte"))
}

/// Declares a function the page can call, under its own name.
macro_rules! export {
    ($(#[$doc:meta])* fn $name:ident($($argument:ident: $kind:ty),*) $(-> $answer:ty)? $body:block) => {
        $(#[$doc])*
        #[allow(unsafe_code, reason = "an exported name is how the page finds the function")]
        #[unsafe(no_mangle)]
        pub extern "C" fn $name($($argument: $kind),*) $(-> $answer)? $body
    };
}

export! {
    /// Starts the input again from nothing.
    fn input_reset() {
        with_host(|host| host.input.clear());
    }
}
export! {
    /// Adds a byte to the input of the next operation.
    fn input_push(value: u32) {
        with_host(|host| host.input.extend(u8::try_from(value)));
    }
}
export! {
    /// Length of the last operation's answer, in bytes.
    fn output_len() -> u32 {
        with_host(|host| u32::try_from(host.output.len()).unwrap_or(u32::MAX))
    }
}
export! {
    /// One byte of the last operation's answer.
    fn output_at(index: u32) -> u32 {
        with_host(|host| {
            usize::try_from(index)
                .ok()
                .and_then(|index| host.output.get(index))
                .map_or(0, |&value| u32::from(value))
        })
    }
}
export! {
    /// Answers with the protocol's numbers, as a JSON object.
    fn link_constants() -> u32 {
        with_host(|host| host.answer(|_| Host::constants()))
    }
}
export! {
    /// Frames the input as a packet of this kind and ID.
    fn link_encode(kind: u32, id: u32) -> u32 {
        with_host(|host| host.answer(|host| host.encode(byte(kind)?, id)))
    }
}
export! {
    /// Decodes bytes from the wire into the packets they complete.
    fn link_decode() -> u32 {
        with_host(|host| host.answer(|host| Ok(host.decode())))
    }
}
export! {
    /// Reads the firmware's answer to HELLO.
    fn host_hello() -> u32 {
        with_host(|host| host.answer(Host::hello))
    }
}
export! {
    /// Builds the JOIN payload for a room and profile.
    fn host_join(room: u32, color: u32) -> u32 {
        with_host(|host| host.answer(|host| host.join(byte(room)?, byte(color)?)))
    }
}
export! {
    /// Builds the DRAW payloads that upload a drawing.
    fn host_draw(id: u32) -> u32 {
        with_host(|host| host.answer(|host| host.draw(id)))
    }
}
export! {
    /// Says what a packet from the firmware means.
    fn host_packet(kind: u32, id: u32) -> u32 {
        with_host(|host| host.answer(|host| host.packet(byte(kind)?, id)))
    }
}
export! {
    /// Names a RESPONSE status.
    fn host_status_text(status: u32) -> u32 {
        with_host(|host| host.answer(|_| Ok(status_text(byte(status)?).as_bytes().to_vec())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use picto_relay_codec::{
        NintendoFlow, encode_fragment_envelope, encode_message, fragment_payload_starting_at,
    };
    use picto_relay_session::bitmap::api_bitmap_to_native;
    use serde_json::Value;

    const STATION: [u8; 6] = [2, 1, 2, 3, 4, 5];

    fn call(host: &mut Host, input: &[u8], operation: impl FnOnce(&mut Host) -> Answer) -> Answer {
        host.input = input.to_vec();
        match host.answer(operation) {
            1 => Ok(host.output.clone()),
            _ => Err(String::from_utf8(host.output.clone()).unwrap()),
        }
    }

    fn json(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).unwrap()
    }

    /// A host that has heard HELLO answered and asked to join room 1.
    fn joined() -> Host {
        let mut host = Host::new();
        let mut hello = vec![link::PROTOCOL_VERSION];
        hello.extend_from_slice(&STATION);
        hello.extend_from_slice(b"abc1234");
        let answer = json(&call(&mut host, &hello, Host::hello).unwrap());
        assert_eq!(answer["version"], link::PROTOCOL_VERSION);
        assert_eq!(answer["build"], "abc1234");
        assert_eq!(answer["station"], "02:01:02:03:04:05");
        assert_eq!(answer["client_id"], 0x0203_0405);
        let join = call(&mut host, b"Rob\0hello", |host| host.join(1, 11)).unwrap();
        assert_eq!(join, join_payload(1, "Rob", "hello", 11).unwrap());
        host
    }

    #[test]
    fn packets_survive_the_wire() {
        let mut host = Host::new();
        let payload = [link::DELIMITER, 1, link::ESCAPE, 2];
        let wire = call(&mut host, &payload, |host| host.encode(link::PING, 7)).unwrap();
        // Split across reads, with noise before it, as a serial port delivers.
        let (first, second) = wire.split_at(5);
        let mut noisy = vec![0x55, 0xaa];
        noisy.extend_from_slice(first);
        assert_eq!(
            call(&mut host, &noisy, |host| Ok(host.decode())).unwrap(),
            Vec::<u8>::new()
        );
        let mut expected = vec![link::PING, 7, 0, 0, 0, 4, 0];
        expected.extend_from_slice(&payload);
        assert_eq!(
            call(&mut host, second, |host| Ok(host.decode())).unwrap(),
            expected
        );
        let large = vec![0; link::MAX_COMMAND_PAYLOAD + 1];
        assert!(call(&mut host, &large, |host| host.encode(link::DRAW, 1)).is_err());
    }

    #[test]
    fn joining_needs_hello_and_a_whole_profile() {
        let mut host = Host::new();
        assert!(call(&mut host, b"Rob\0", |host| host.join(0, 0)).is_err());
        let mut host = joined();
        assert!(call(&mut host, b"Rob", |host| host.join(0, 0)).is_err());
        assert!(call(&mut host, b"Rob\0", |host| host.join(4, 0)).is_err());
        assert!(call(&mut host, b"Rob\0", |host| host.join(0, 16)).is_err());
    }

    #[test]
    fn firmware_packets_become_events() {
        let mut host = joined();
        let events = |host: &mut Host, kind, id, payload: &[u8]| {
            json(&call(host, payload, |host| host.packet(kind, id)).unwrap())
        };

        let mut ready = vec![0, 9, 8, 7, 6, 5];
        ready.extend_from_slice(&[3, 1, 7]);
        assert_eq!(
            events(&mut host, link::READY, 0, &ready),
            serde_json::json!([{"type": "ready", "client_id": 0x0203_0405, "slot": 3, "channel": 7}])
        );

        let mut status = vec![0; link::STATUS_LEN];
        status[link::STATUS_STATE_OFFSET] = link::STATE_CONNECTED;
        status[link::STATUS_SENT_OFFSET] = 4;
        status[link::STATUS_TOTAL_OFFSET] = 9;
        status[link::STATUS_ECHOED_OFFSET] = 2;
        assert_eq!(
            events(&mut host, link::STATUS, 12, &status)[0],
            serde_json::json!({
                "type": "status", "message_id": 12, "state": link::STATE_CONNECTED,
                "phase": 0, "sent": 4, "total": 9, "echoed": 2,
            })
        );

        assert_eq!(
            events(&mut host, link::DELIVERED, 12, &[]),
            serde_json::json!([{"type": "delivered", "message_id": 12}])
        );
        assert_eq!(
            events(&mut host, link::ROOMS, 0, &[2, link::NO_BEACON, 0, 16]),
            serde_json::json!([{"type": "rooms", "counts": [2, null, 0, 16]}])
        );
        assert_eq!(
            events(&mut host, link::GAP, 0, &[]),
            serde_json::json!([{"type": "gap"}])
        );
        assert_eq!(
            events(&mut host, link::DIAG, 0, &[1, 2, 3, 4]),
            serde_json::json!([])
        );
        // Short or unknown packets mean nothing rather than something wrong.
        assert_eq!(
            events(&mut host, link::STATUS, 0, &[1]),
            serde_json::json!([])
        );
        assert_eq!(events(&mut host, 250, 0, &[]), serde_json::json!([]));

        let left = events(&mut host, link::LEFT, 0, &[link::LEFT_NO_HOST, 0, 0]);
        assert_eq!(left[0]["type"], "left");
        assert!(
            left[0]["detail"]
                .as_str()
                .unwrap()
                .contains("open the room")
        );
        assert!(byte(300).is_err());
    }

    #[test]
    fn a_drawing_goes_out_as_draw_commands_and_comes_in_as_a_message() {
        let mut host = joined();
        let mut palette = vec![0; 12288];
        palette[128 * 50 + 20] = 0xf3;

        // Out: the commands carry the native drawing, in order.
        let commands = call(&mut host, &palette, |host| host.draw(9)).unwrap();
        let native = native_drawing(MacAddress(STATION), &palette).unwrap();
        let mut rebuilt = Vec::new();
        let mut rest = commands.as_slice();
        while let [low, high, tail @ ..] = rest {
            let (command, tail) = tail.split_at(usize::from(u16::from_le_bytes([*low, *high])));
            assert_eq!(
                command[link::DRAW_ID_OFFSET..link::DRAW_POSITION_OFFSET],
                9u32.to_le_bytes()
            );
            rebuilt.extend_from_slice(&command[link::DRAW_DATA_OFFSET..]);
            rest = tail;
        }
        assert_eq!(rebuilt, native);
        assert!(call(&mut host, &[0; 12288], |host| host.draw(10)).is_err());
        assert!(call(&mut host, &[1, 2, 3], |host| host.draw(10)).is_err());

        // In: someone else's drawing, fragment by fragment, is one message.
        let sender = MacAddress([0, 9, 8, 7, 6, 5]);
        let mut ready = sender.octets().to_vec();
        ready.extend_from_slice(&[1, 1, 7]);
        call(&mut host, &ready, |host| host.packet(link::READY, 0)).unwrap();
        let theirs = encode_message(sender, &api_bitmap_to_native(&palette).unwrap()).unwrap();
        let mut messages = Vec::new();
        for fragment in fragment_payload_starting_at(&theirs, [1, 2], 120, 50).unwrap() {
            let body = encode_fragment_envelope(NintendoFlow::HostToClient, &fragment).unwrap();
            let events = json(&call(&mut host, &body, |host| host.packet(link::RX, 0)).unwrap());
            messages.extend(events.as_array().unwrap().clone());
        }
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["type"], "message");
        assert_eq!(messages[0]["station_id"], 0x0807_0605);
        assert_eq!(messages[0]["palette_bitmap"], STANDARD.encode(&palette));
    }

    /// Writes or checks the fixture the web client's driver tests replay
    /// (`web/src/serial/fixtures/incoming-drawing.json`): the frames in which
    /// a host DS rebroadcasts someone's drawing, and the drawing they carry.
    /// Regenerate with `just protocol-header`.
    #[test]
    fn web_fixture_matches_the_codec() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/src/serial/fixtures/incoming-drawing.json");
        let sender = MacAddress([0, 9, 8, 7, 6, 5]);
        let mut palette = vec![0; 12288];
        for row in 20..60 {
            palette[128 * row + 30 + row % 16] = 0xf3;
        }
        let native = encode_message(sender, &api_bitmap_to_native(&palette).unwrap()).unwrap();
        let bodies: Vec<String> = fragment_payload_starting_at(&native, [1, 2], 120, 50)
            .unwrap()
            .iter()
            .map(|fragment| {
                STANDARD
                    .encode(encode_fragment_envelope(NintendoFlow::HostToClient, fragment).unwrap())
            })
            .collect();
        let mut expected = serde_json::to_string_pretty(&serde_json::json!({
            "host": sender.octets(),
            "station_id": station_id(sender),
            "palette_bitmap": STANDARD.encode(&palette),
            "bodies": bodies,
        }))
        .unwrap();
        expected.push('\n');
        if std::env::var_os("PICTO_WRITE_FIXTURES").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &expected).unwrap();
        }
        assert!(
            std::fs::read_to_string(&path).unwrap() == expected,
            "incoming-drawing.json is out of date; run `just protocol-header`"
        );
    }

    #[test]
    fn the_exported_functions_pass_bytes_through() {
        input_reset();
        for value in [1, 2, 3] {
            input_push(value);
        }
        assert_eq!(link_encode(u32::from(link::PING), 5), 1);
        let wire: Vec<u32> = (0..output_len()).map(|index| output_at(index)).collect();
        assert_eq!(wire.first(), Some(&u32::from(link::DELIMITER)));
        for value in wire {
            input_push(value);
        }
        assert_eq!(link_decode(), 1);
        let packet: Vec<u32> = (0..output_len()).map(|index| output_at(index)).collect();
        assert_eq!(packet, [u32::from(link::PING), 5, 0, 0, 0, 3, 0, 1, 2, 3]);
        assert_eq!(output_at(1000), 0);
        assert_eq!(link_encode(999, 0), 0);
        assert_eq!(link_constants(), 1);
        assert_eq!(host_status_text(u32::from(link::STATUS_BUSY)), 1);
    }
}
