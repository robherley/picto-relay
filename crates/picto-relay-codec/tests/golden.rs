use picto_relay_codec::{
    FcsMode, MacAddress, MacFrame, ManagementBody, NintendoBeacon, parse_management,
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
