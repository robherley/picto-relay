use picto_relay_codec::{
    FcsMode, MacAddress, MacFrame, ManagementBody, NintendoBeacon, PictoChatBeaconConfig,
    build_pictochat_beacon, build_pictochat_beacon_template, parse_management,
};
mod support;

use support::decode_hex;

#[test]
fn captured_room_a_beacon_should_decode_and_round_trip_losslessly() {
    let bytes = decode_hex(include_str!("fixtures/pictochat-room-a-beacon.hex"));
    let frame = MacFrame::parse(&bytes, FcsMode::Absent).expect("captured beacon should parse");
    assert_eq!(frame.as_bytes(), bytes);
    assert_eq!(
        frame.header.address_2,
        Some(MacAddress([0x00, 0x09, 0xbf, 0x34, 0xdb, 0x3c]))
    );
    let ManagementBody::Beacon(beacon) =
        parse_management(&frame).expect("management body should parse")
    else {
        panic!("fixture should be a beacon");
    };
    let nintendo = NintendoBeacon::from_beacon(&beacon).expect("Nintendo element should parse");
    let room = nintendo
        .pictochat()
        .expect("room payload should parse")
        .expect("fixture should advertise PictoChat");
    assert_eq!(room.room, 0);
    assert_eq!(room.user_count, 1);
    assert_eq!(room.opaque_state, 0x473e_2348);
    assert_eq!(room.fixed_suffix, 4);
    assert_eq!(nintendo.command_data_size, 0x00c0);
    assert_eq!(nintendo.reply_data_size, 0x00c0);
}

#[test]
fn software_host_beacon_should_round_trip_room_fields() {
    let host = MacAddress([0, 9, 0xbf, 0x52, 0, 0]);
    let bytes = build_pictochat_beacon(PictoChatBeaconConfig {
        host,
        timestamp: 123_456,
        sequence_number: 7,
        game_id: 0,
        stream_code: 4,
        opaque_state: 0x1234_5678,
        room: 2,
        user_count: 3,
        channel: 1,
    })
    .expect("room beacon should encode");
    let frame = MacFrame::parse(&bytes, FcsMode::Absent).expect("room beacon should parse");
    let ManagementBody::Beacon(beacon) =
        parse_management(&frame).expect("beacon body should parse")
    else {
        panic!("encoded management frame should be a beacon");
    };
    let room = NintendoBeacon::from_beacon(&beacon)
        .expect("Nintendo element should parse")
        .pictochat()
        .expect("room data should parse")
        .expect("type one should contain room data");

    assert_eq!((room.room, room.user_count), (2, 3));
}

#[test]
fn hardware_beacon_template_should_leave_tsf_and_tim_to_the_adapter() {
    let config = PictoChatBeaconConfig {
        host: MacAddress([0x00, 0x09, 0xbf, 0x52, 0x00, 0x00]),
        timestamp: 0x1234_5678,
        sequence_number: 7,
        game_id: 0,
        stream_code: 1,
        opaque_state: 0x473e_2348,
        room: 0,
        user_count: 1,
        channel: 1,
    };
    let template = build_pictochat_beacon_template(config).expect("template should encode");

    assert_eq!(&template.head[24..32], &[0; 8]);
    assert_eq!(&template.head[36..], &[1, 2, 0x82, 0x84, 3, 1, 1]);
    assert_eq!(template.tail[0], 0xdd);
    assert!(!template.head.windows(2).any(|element| element == [5, 5]));

    let frame = build_pictochat_beacon(config).expect("software beacon should encode");
    let parsed = MacFrame::parse(&frame, FcsMode::Absent).expect("beacon should parse");
    let ManagementBody::Beacon(beacon) =
        parse_management(&parsed).expect("beacon body should decode")
    else {
        panic!("expected beacon body");
    };
    assert_eq!(beacon.timestamp, config.timestamp);
}

#[test]
fn captured_joined_beacon_should_report_two_users_and_opaque_state() {
    let bytes = decode_hex(include_str!("fixtures/joined-room-a-beacon.hex"));
    let frame = MacFrame::parse(&bytes, FcsMode::Absent).expect("captured beacon should parse");
    let ManagementBody::Beacon(beacon) =
        parse_management(&frame).expect("management body should parse")
    else {
        panic!("fixture should be a beacon");
    };
    let room = NintendoBeacon::from_beacon(&beacon)
        .expect("Nintendo element should parse")
        .pictochat()
        .expect("room payload should parse")
        .expect("fixture should advertise PictoChat");
    assert_eq!(room.room, 0);
    assert_eq!(room.user_count, 2);
    assert_eq!(room.opaque_state, 0x2fe6_bd8a);
    assert_eq!(room.fixed_suffix, 4);
}
