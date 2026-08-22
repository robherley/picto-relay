use picto_relay_codec::{
    FcsMode, MacAddress, MacFrame, MessageReassembler, MultiplayerHeader, NintendoFlow,
    PictoChatEnvelope, PictoChatFragment, PictoChatMessage, ReassemblyKey,
    build_association_request, build_authentication_request, build_client_reply_frame,
    build_fragment_frame, decode_nintendo_data,
};

fn decode_hex(text: &str) -> Vec<u8> {
    let compact = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    compact
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            u8::from_str_radix(
                std::str::from_utf8(pair).expect("fixture must be ASCII"),
                16,
            )
            .expect("fixture must contain valid hex")
        })
        .collect()
}

fn decode_data(hex: &str) -> (picto_relay_codec::NintendoData, PictoChatEnvelope) {
    let bytes = decode_hex(hex);
    let frame = MacFrame::parse(&bytes, FcsMode::Absent).expect("captured frame should parse");
    assert_eq!(frame.as_bytes(), bytes);
    let data = decode_nintendo_data(&frame).expect("Nintendo data should parse");
    let envelope = PictoChatEnvelope::parse(data.flow, &data.payload)
        .expect("PictoChat envelope should parse");
    (data, envelope)
}

#[test]
fn association_request_should_match_captured_nintendo_shape() {
    let station = MacAddress([0x00, 0x16, 0x56, 0x4d, 0x27, 0xca]);
    let host = MacAddress([0x00, 0x24, 0xf3, 0x67, 0x41, 0x87]);
    let frame = build_association_request(station, host, 0, 4, 1, false);
    assert_eq!(
        frame,
        decode_hex(
            "0010a2000024f36741870016564d27ca0024f36741871000210001000020\
             000000000400000000000000000000000000000000000000000000000000000001028284"
        )
    );
}

#[test]
fn authentication_request_should_use_open_system_sequence_one() {
    let station = MacAddress([0x02, 0x09, 0, 0, 0, 1]);
    let host = MacAddress([0x00, 0x24, 0xf3, 0x67, 0x41, 0x87]);
    let frame = build_authentication_request(station, host, 0, false);
    let parsed = MacFrame::parse(&frame, FcsMode::Absent).expect("request should parse");
    assert_eq!(parsed.header.frame_control.0, 0x10b0);
    let body = picto_relay_codec::parse_management(&parsed).expect("body should parse");
    let picto_relay_codec::ManagementBody::Authentication(authentication) = body else {
        panic!("request should be authentication");
    };
    assert_eq!((authentication.algorithm, authentication.sequence), (0, 1));
}

#[test]
fn idle_client_reply_should_match_captured_nintendo_shape() {
    let station = MacAddress([0x00, 0x16, 0x56, 0x4d, 0x27, 0xca]);
    let host = MacAddress([0x00, 0x24, 0xf3, 0x67, 0x41, 0x87]);
    assert_eq!(
        build_client_reply_frame(station, host, 3, &[0, 0x80]),
        decode_hex("1811fe030024f36741870016564d27ca0309bf00001030000080")
    );
}

#[test]
fn captured_client_fragment_should_decode_all_protocol_layers() {
    let bytes = include_str!("fixtures/client-first-fragment.hex");
    let raw = decode_hex(bytes);
    let frame = MacFrame::parse(&raw, FcsMode::Absent).expect("captured frame should parse");
    assert!(frame.header.frame_control.to_ds());
    assert_eq!(
        frame.header.address_1,
        MacAddress([0x00, 0x24, 0xf3, 0x67, 0x41, 0x87])
    );
    assert_eq!(
        frame.header.address_3,
        Some(MacAddress([0x03, 0x09, 0xbf, 0, 0, 0x10]))
    );

    let (data, envelope) = decode_data(bytes);
    assert_eq!(data.flow, NintendoFlow::ClientToHost);
    assert_eq!(data.multiplayer, MultiplayerHeader::ClientReply);
    assert_eq!(
        data.source,
        MacAddress([0x00, 0x16, 0x56, 0x4d, 0x27, 0xca])
    );
    assert_eq!(envelope.packet_id, 0x8e56);
    assert_eq!(envelope.packet_sequence, 2);
    assert_eq!(envelope.declared_length, 172);
    assert_eq!(envelope.payload.len(), 170);

    let fragment = PictoChatFragment::parse(&envelope.payload, data.flow)
        .expect("captured fragment should parse");
    assert_eq!(fragment.opaque_header, [0x01, 0x6c]);
    assert!(!fragment.final_fragment);
    assert_eq!(fragment.data_offset, 0);
    assert_eq!(fragment.data.len(), 160);
    assert_eq!(fragment.frame_number, 3);
    assert_eq!(fragment.host_trailer, None);
}

#[test]
fn captured_host_rebroadcast_should_decode_multiplayer_prefix_and_trailer() {
    let (data, envelope) = decode_data(include_str!("fixtures/host-first-fragment.hex"));
    assert_eq!(data.flow, NintendoFlow::HostToClient);
    assert_eq!(
        data.multiplayer,
        MultiplayerHeader::HostCommand {
            client_time: 998,
            client_mask: 2
        }
    );
    assert_eq!(envelope.packet_id, 0x1e56);
    assert_eq!(envelope.declared_length, 172);
    let fragment = PictoChatFragment::parse(&envelope.payload, data.flow)
        .expect("captured fragment should parse");
    assert_eq!(fragment.data.len(), 160);
    assert_eq!(fragment.frame_number, 6);
    assert_eq!(fragment.host_trailer, Some(2));
}

#[test]
fn captured_ack_should_decode_command_counter() {
    let bytes = decode_hex(include_str!("fixtures/host-ack.hex"));
    let frame = MacFrame::parse(&bytes, FcsMode::Absent).expect("captured ack should parse");
    let data = decode_nintendo_data(&frame).expect("captured ack should decode");
    assert_eq!(data.flow, NintendoFlow::HostAcknowledgement);
    assert_eq!(
        data.multiplayer,
        MultiplayerHeader::HostAcknowledgement {
            command_count: 221,
            failed_client_mask: 0,
        }
    );
    assert!(data.payload.is_empty());
}

#[test]
fn captured_final_fragment_should_use_explicit_marker() {
    let (data, envelope) = decode_data(include_str!("fixtures/client-final-fragment.hex"));
    assert_eq!(envelope.packet_id, 0x8e08);
    assert_eq!(envelope.declared_length, 16);
    let fragment = PictoChatFragment::parse(&envelope.payload, data.flow)
        .expect("captured final fragment should parse");
    assert!(fragment.final_fragment);
    assert_eq!(fragment.data_offset, 0x820);
    assert_eq!(fragment.data, [0, 0, 0, 0]);
    assert_eq!(fragment.frame_number, 16);
}

#[test]
fn encoded_client_final_fragment_should_match_capture_body() {
    let captured = include_str!("fixtures/client-final-fragment.hex");
    let (data, envelope) = decode_data(captured);
    let fragment = PictoChatFragment::parse(&envelope.payload, data.flow)
        .expect("captured final fragment should parse");
    let encoded = build_fragment_frame(
        NintendoFlow::ClientToHost,
        data.source,
        data.bssid.expect("client reply should name its host"),
        data.sequence_number,
        data.retry,
        &fragment,
    )
    .expect("captured final fragment should re-encode");
    let encoded = MacFrame::parse(&encoded, FcsMode::Absent).expect("encoded frame should parse");
    let captured = MacFrame::parse(&decode_hex(captured), FcsMode::Absent)
        .expect("captured frame should parse");
    assert_eq!(encoded.body(), captured.body());
}

#[test]
fn captured_host_final_fragment_should_allow_fixed_slot_padding() {
    let (data, envelope) = decode_data(include_str!("fixtures/host-padded-final-fragment.hex"));
    assert_eq!(envelope.packet_id, 0x9e56);
    assert_eq!(envelope.declared_length, 16);
    assert_eq!(envelope.payload.len(), 172);
    let fragment = PictoChatFragment::parse(&envelope.payload, data.flow)
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
    assert_eq!(decoded.encoded_height(), 16);
    assert!(decoded.pixels().contains(&1));
}
