pub fn decode_hex(text: &str) -> Vec<u8> {
    let compact = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let chunks = compact.as_bytes().chunks_exact(2);
    assert!(
        chunks.remainder().is_empty(),
        "fixture must contain complete hexadecimal bytes"
    );
    chunks
        .map(|pair| {
            u8::from_str_radix(
                std::str::from_utf8(pair).expect("fixture must be ASCII"),
                16,
            )
            .expect("fixture must contain valid hexadecimal digits")
        })
        .collect()
}
