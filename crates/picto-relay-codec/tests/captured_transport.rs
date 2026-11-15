//! Application packets taken from captures of real DS hardware. The fixtures hold
//! whole radio frames; these tests skip the IEEE 802.11 header, which the
//! firmware handles, and decode what follows.

use picto_relay_codec::{
    MacAddress, MessageReassembler, NintendoFlow, PictoChatEnvelope, PictoChatFragment,
    PictoChatMessage, PictoChatProfile, ReassemblyKey, encode_fragment_envelope,
};
mod support;

use support::decode_hex;

/// Bytes in the IEEE 802.11 header of a captured data frame.
const MAC_HEADER_BYTES: usize = 24;
/// Bytes of reply window and client mask that open a host poll's body.
const POLL_HEADER_BYTES: usize = 4;

/// The application envelope carried by a captured frame, and its bytes.
fn envelope(hex: &str, flow: NintendoFlow) -> (PictoChatEnvelope, Vec<u8>) {
    let frame = decode_hex(hex);
    let start = match flow {
        NintendoFlow::ClientToHost => MAC_HEADER_BYTES,
        NintendoFlow::HostToClient => MAC_HEADER_BYTES + POLL_HEADER_BYTES,
    };
    let bytes = frame[start..].to_vec();
    let envelope = PictoChatEnvelope::parse(flow, &bytes).expect("PictoChat envelope should parse");
    (envelope, bytes)
}

#[test]
fn captured_client_fragment_should_decode() {
    let flow = NintendoFlow::ClientToHost;
    let (envelope, _) = envelope(include_str!("fixtures/client-first-fragment.hex"), flow);
    assert_eq!(envelope.packet_id, 0x8e56);
    assert_eq!(envelope.packet_sequence, 2);
    assert_eq!(envelope.declared_length, 172);
    assert_eq!(envelope.payload.len(), 170);

    let fragment =
        PictoChatFragment::parse(&envelope.payload, flow).expect("captured fragment should parse");
    assert_eq!(fragment.opaque_header, [0x01, 0x6c]);
    assert!(!fragment.final_fragment);
    assert_eq!(fragment.data_offset, 0);
    assert_eq!(fragment.data.len(), 160);
    assert_eq!(fragment.frame_number, 3);
    assert_eq!(fragment.host_trailer, None);
}

#[test]
fn captured_host_rebroadcast_should_decode_its_trailer() {
    let flow = NintendoFlow::HostToClient;
    let (envelope, _) = envelope(include_str!("fixtures/host-first-fragment.hex"), flow);
    assert_eq!(envelope.packet_id, 0x1e56);
    assert_eq!(envelope.declared_length, 172);
    let fragment =
        PictoChatFragment::parse(&envelope.payload, flow).expect("captured fragment should parse");
    assert_eq!(fragment.data.len(), 160);
    assert_eq!(fragment.frame_number, 6);
    assert_eq!(fragment.host_trailer, Some(2));
}

#[test]
fn captured_profile_should_trim_full_width_nickname_padding() {
    let bytes = decode_hex(include_str!("fixtures/host-client-profile-envelope.hex"));
    let envelope = PictoChatEnvelope::parse(NintendoFlow::HostToClient, &bytes)
        .expect("captured profile envelope should parse");
    let fragment = PictoChatFragment::parse(&envelope.payload, NintendoFlow::HostToClient)
        .expect("captured profile fragment should parse");

    let profile = PictoChatProfile::parse(&fragment.data).expect("captured profile should parse");

    assert_eq!(
        (profile.station, profile.nickname),
        (
            MacAddress([0, 0x25, 0xa0, 0xdc, 0x81, 0xdf]),
            "ちふみ".into(),
        )
    );
}

#[test]
fn captured_final_fragment_should_use_explicit_marker() {
    let flow = NintendoFlow::ClientToHost;
    let (envelope, _) = envelope(include_str!("fixtures/client-final-fragment.hex"), flow);
    assert_eq!(envelope.packet_id, 0x8e08);
    assert_eq!(envelope.declared_length, 16);
    let fragment = PictoChatFragment::parse(&envelope.payload, flow)
        .expect("captured final fragment should parse");
    assert!(fragment.final_fragment);
    assert_eq!(fragment.data_offset, 0x820);
    assert_eq!(fragment.data, [0, 0, 0, 0]);
    assert_eq!(fragment.frame_number, 16);
}

#[test]
fn encoded_client_final_fragment_should_match_the_capture() {
    let flow = NintendoFlow::ClientToHost;
    let (envelope, captured) = envelope(include_str!("fixtures/client-final-fragment.hex"), flow);
    let fragment = PictoChatFragment::parse(&envelope.payload, flow)
        .expect("captured final fragment should parse");
    let encoded =
        encode_fragment_envelope(flow, &fragment).expect("captured fragment should re-encode");
    assert_eq!(encoded, captured);
}

#[test]
fn captured_host_final_fragment_should_allow_fixed_slot_padding() {
    let flow = NintendoFlow::HostToClient;
    let (envelope, _) = envelope(
        include_str!("fixtures/host-padded-final-fragment.hex"),
        flow,
    );
    assert_eq!(envelope.packet_id, 0x9e56);
    assert_eq!(envelope.declared_length, 16);
    assert_eq!(envelope.payload.len(), 172);
    let fragment = PictoChatFragment::parse(&envelope.payload, flow)
        .expect("padded host final fragment should parse");
    assert!(fragment.final_fragment);
    assert_eq!(fragment.data_offset, 0x820);
    assert_eq!(fragment.data, [0, 0, 0, 0]);
    assert_eq!(fragment.frame_number, 43);
    assert_eq!(fragment.host_trailer, Some(2));
}

#[test]
fn complete_captured_client_message_should_reassemble_to_2084_bytes() {
    let source = MacAddress([0x00, 0x16, 0x56, 0x4d, 0x27, 0xca]);
    let key = ReassemblyKey {
        source,
        opaque_id: [0, 0],
    };
    let mut reassembler = MessageReassembler::new(160);
    let mut completed = None;
    for line in include_str!("fixtures/captured-client-transfer.hex").lines() {
        let payload = decode_hex(line);
        let fragment = PictoChatFragment::parse(&payload, NintendoFlow::ClientToHost)
            .expect("captured fragment should parse");
        completed = reassembler
            .push(key, &fragment)
            .expect("captured transfer should reassemble")
            .or(completed);
    }
    let message = completed.expect("captured transfer should complete");
    assert_eq!(message.len(), 2084);
    assert_eq!(
        &message[..36],
        &decode_hex("030216004d56ca270004000000000104090b101b00000000000000000000000000000000")
    );
    assert_eq!(message.iter().filter(|&&byte| byte != 0).count(), 19);
    let decoded = PictoChatMessage::parse(&message).expect("captured drawing should decode");
    assert_eq!(decoded.sender(), source);
    assert!(decoded.pixels().contains(&1));
}
