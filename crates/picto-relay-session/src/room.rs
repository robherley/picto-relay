//! Who is in the room and what they send, worked out from what the host DS
//! rebroadcasts.
use crate::bitmap::native_message_to_api_bitmap;
use picto_relay_codec::{
    MAC_ADDRESS_LEN, MacAddress, MessageReassembler, NintendoFlow, PictoChatEnvelope,
    PictoChatFragment, PictoChatMessage, PictoChatProfile, PictoChatRoster, ReassemblyKey,
};
use std::collections::{BTreeMap, VecDeque};

/// Recently completed incoming drawings remembered to drop host repeats.
const MAX_COMPLETED_DRAWINGS: usize = 128;
/// Fragment sizes a DS host may use when rebroadcasting drawings, in bytes.
const HOST_FRAGMENT_SIZES: [usize; 3] = [80, 120, 160];
/// Mask removing the flag bit whose meaning is not established from a packet ID.
const PACKET_ID_MASK: u16 = 0x7fff;
/// Packet ID of the host's participant roster.
const ROSTER_PACKET_ID: u16 = 0x1c34;
/// Length of the opaque transfer tag that identifies a host rebroadcast.
const TRANSFER_TAG_LEN: usize = 2;

/// One station in the room.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Participant {
    /// MAC address carried by the host's roster.
    pub station: MacAddress,
    /// Profile nickname when observed, otherwise a stable MAC-derived label.
    pub name: String,
}

/// Something that happened in the room.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoomEvent {
    /// The roster changed, or someone in it became known by name.
    Participants(Vec<Participant>),
    /// A station's profile was first seen, or changed.
    Profile(PictoChatProfile),
    /// Someone else's drawing, as an API palette bitmap.
    Message {
        sender: MacAddress,
        name: String,
        message_id: u32,
        bitmap: Vec<u8>,
    },
}

/// The room as one station joined to it knows it.
pub struct Room {
    station: MacAddress,
    host: MacAddress,
    names: BTreeMap<MacAddress, String>,
    profiles: BTreeMap<MacAddress, PictoChatProfile>,
    members: Vec<MacAddress>,
    assemblers: [MessageReassembler; HOST_FRAGMENT_SIZES.len()],
    completed: VecDeque<(MacAddress, u16, [u8; TRANSFER_TAG_LEN])>,
}

/// One reassembler per fragment size a DS host may use.
fn host_reassemblers() -> [MessageReassembler; HOST_FRAGMENT_SIZES.len()] {
    HOST_FRAGMENT_SIZES.map(MessageReassembler::new)
}

impl Room {
    /// The room before anything is known of it but our own station and
    /// nickname. The member list is published when the host's first roster
    /// arrives.
    #[must_use]
    pub fn new(station: MacAddress, name: &str) -> Self {
        Self {
            station,
            host: MacAddress([0; MAC_ADDRESS_LEN]),
            names: BTreeMap::from([(station, name.into())]),
            profiles: BTreeMap::new(),
            members: vec![station],
            assemblers: host_reassemblers(),
            completed: VecDeque::new(),
        }
    }

    /// Our own station address.
    #[must_use]
    pub const fn station(&self) -> MacAddress {
        self.station
    }

    /// Notes which DS hosts the room, once the firmware has joined it.
    pub const fn set_host(&mut self, host: MacAddress) {
        self.host = host;
    }

    /// Everyone the host lists, with the names known so far.
    #[must_use]
    pub fn participants(&self) -> Vec<Participant> {
        self.members
            .iter()
            .map(|mac| Participant {
                station: *mac,
                name: self.name_of(*mac),
            })
            .collect()
    }

    fn name_of(&self, station: MacAddress) -> String {
        self.names
            .get(&station)
            .cloned()
            .unwrap_or_else(|| station.to_string())
    }

    /// Forgets drawings that were part-way through arriving, after the
    /// firmware reports that it dropped some of what it heard.
    pub fn discard_incoming(&mut self) {
        self.assemblers = host_reassemblers();
    }

    /// Takes the body of one frame the host sent us and returns what it
    /// changes. Most frames change nothing: a drawing arrives over many.
    pub fn receive(&mut self, body: &[u8]) -> Vec<RoomEvent> {
        let mut events = Vec::new();
        let Ok(env) = PictoChatEnvelope::parse(NintendoFlow::HostToClient, body) else {
            return events;
        };
        if env.packet_id & PACKET_ID_MASK == ROSTER_PACKET_ID {
            if let Ok(roster) = PictoChatRoster::parse(&env.payload) {
                let members: Vec<_> = roster.members.into_iter().flatten().collect();
                if members != self.members {
                    self.members = members;
                    events.push(RoomEvent::Participants(self.participants()));
                }
            }
            return events;
        }
        if !env.is_message_fragment() {
            return events;
        }
        let Ok(fragment) = PictoChatFragment::parse(&env.payload, NintendoFlow::HostToClient)
        else {
            return events;
        };
        if let Ok(profile) = PictoChatProfile::parse(&fragment.data) {
            let changed = self.profiles.get(&profile.station).is_none_or(|old| {
                old.nickname != profile.nickname
                    || old.message != profile.message
                    || old.color != profile.color
            });
            if changed {
                self.profiles.insert(profile.station, profile.clone());
                events.push(RoomEvent::Profile(profile.clone()));
            }
            if self.names.get(&profile.station) != Some(&profile.nickname) {
                self.names.insert(profile.station, profile.nickname);
                events.push(RoomEvent::Participants(self.participants()));
            }
            return events;
        }
        for index in 0..self.assemblers.len() {
            let key = ReassemblyKey {
                source: self.host,
                opaque_id: fragment.opaque_id,
            };
            let Ok(Some(bytes)) = self.assemblers[index].push(key, &fragment) else {
                continue;
            };
            let Ok(message) = PictoChatMessage::parse(&bytes) else {
                continue;
            };
            let sender = message.sender();
            let key = (sender, fragment.frame_number, fragment.opaque_id);
            if self.completed.contains(&key) {
                continue;
            }
            self.completed.push_back(key);
            if self.completed.len() > MAX_COMPLETED_DRAWINGS {
                self.completed.pop_front();
            }
            // The device's exact echo tracking publishes our own message once.
            if sender == self.station {
                continue;
            }
            events.push(RoomEvent::Message {
                sender,
                name: self.name_of(sender),
                message_id: u32::from(fragment.frame_number),
                bitmap: native_message_to_api_bitmap(&message),
            });
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitmap::{api_bitmap_to_native, palette_bitmap};
    use crate::surface::PALETTE_BITMAP_BYTES;
    use picto_relay_codec::{
        encode_fragment_envelope, encode_message, fragment_payload_starting_at,
    };

    #[test]
    fn incoming_fragments_reassemble_once_for_both_host_fragment_sizes() {
        let sender = MacAddress([0, 2, 3, 4, 5, 6]);
        let station = MacAddress([2, 1, 2, 3, 4, 5]);
        let mut bitmap = vec![0; 3072];
        bitmap[32 * 50 + 20] = 0x81;
        let mut palette = vec![0; PALETTE_BITMAP_BYTES];
        palette[128 * 50 + 20] = 0xf3;
        for bitmap in [bitmap, palette] {
            let native = encode_message(sender, &api_bitmap_to_native(&bitmap).unwrap()).unwrap();
            for size in [80, 120, 160] {
                let mut room = Room::new(station, "Relay");
                room.set_host(sender);
                room.names.insert(sender, "DS".into());
                room.members = vec![sender, station];
                let mut events = Vec::new();
                for _ in 0..2 {
                    for fragment in fragment_payload_starting_at(&native, [1, 2], size, 50).unwrap()
                    {
                        let body = encode_fragment_envelope(NintendoFlow::HostToClient, &fragment)
                            .unwrap();
                        events.extend(room.receive(&body));
                    }
                }
                assert_eq!(events.len(), 1);
                let RoomEvent::Message {
                    bitmap: actual,
                    name,
                    sender: from,
                    ..
                } = &events[0]
                else {
                    panic!("expected drawing")
                };
                // Received drawings are always reported as palette bitmaps.
                assert_eq!(actual, &palette_bitmap(&bitmap));
                assert_eq!(name, "DS");
                assert_eq!(*from, sender);
            }
        }
    }

    #[test]
    fn our_own_drawing_coming_back_is_not_reported() {
        let host = MacAddress([0, 2, 3, 4, 5, 6]);
        let station = MacAddress([2, 1, 2, 3, 4, 5]);
        let mut bitmap = vec![0; 3072];
        bitmap[32 * 50 + 20] = 0x81;
        let native = encode_message(station, &api_bitmap_to_native(&bitmap).unwrap()).unwrap();
        let mut room = Room::new(station, "Relay");
        room.set_host(host);
        let mut events = Vec::new();
        for fragment in fragment_payload_starting_at(&native, [1, 2], 120, 50).unwrap() {
            let body = encode_fragment_envelope(NintendoFlow::HostToClient, &fragment).unwrap();
            events.extend(room.receive(&body));
        }
        assert_eq!(events, []);
    }
}
