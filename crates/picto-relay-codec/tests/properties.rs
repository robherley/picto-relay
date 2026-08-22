use picto_relay_codec::{
    FcsMode, MacAddress, MacFrame, MessageReassembler, ReassemblyKey, SCRIPTED_FRAGMENT_PAYLOAD,
    fragment_payload,
};
use proptest::prelude::*;

proptest! {
    #[test]
    fn raw_parser_should_never_panic_or_discard_bytes(
        bytes in proptest::collection::vec(any::<u8>(), 0..4096),
        has_fcs in any::<bool>(),
    ) {
        let mode = if has_fcs { FcsMode::Present } else { FcsMode::Absent };
        if let Ok(frame) = MacFrame::parse(&bytes, mode) {
            prop_assert_eq!(frame.as_bytes(), bytes.as_slice());
        }
    }

    #[test]
    fn scripted_fragmentation_should_round_trip(
        payload in proptest::collection::vec(any::<u8>(), 0..8192),
        opaque_id in any::<[u8; 2]>(),
    ) {
        let fragments = fragment_payload(&payload, opaque_id, SCRIPTED_FRAGMENT_PAYLOAD)
            .expect("bounded generated payload should fragment");
        let key = ReassemblyKey {
            source: MacAddress([0x02, 0x09, 0, 0, 0, 1]),
            opaque_id,
        };
        let mut reassembler = MessageReassembler::new(SCRIPTED_FRAGMENT_PAYLOAD);
        let mut completed = None;
        for fragment in fragments.iter().rev() {
            if let Some(message) = reassembler
                .push(key, fragment)
                .expect("generated fragments should not conflict")
            {
                completed = Some(message);
            }
        }
        prop_assert_eq!(completed, Some(payload));
    }
}
