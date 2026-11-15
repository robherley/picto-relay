/* USB framing between the host server and this firmware. The protocol itself
 * (packet kinds, payload layouts, sizes) is defined once in the Rust crate
 * crates/picto-relay-link and generated into link_protocol.h. See docs/esp32-usb.md. */
#pragma once
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include "link_protocol.h"

/* Header plus CRC: the size of a packet with an empty payload. */
#define LINK_OVERHEAD (LINK_PAYLOAD_OFFSET + LINK_CRC_LEN)
/* Largest unescaped command the decoder must hold. */
#define LINK_MAX_COMMAND_RAW (LINK_MAX_COMMAND_PAYLOAD + LINK_OVERHEAD)
/* Wire size of a packet with `payload` bytes when every byte needs escaping. */
#define LINK_ENCODED_CAPACITY(payload) (((payload) + LINK_OVERHEAD) * 2 + 2)
/* Largest encoded response or event. */
#define LINK_MAX_EVENT_ENCODED LINK_ENCODED_CAPACITY(LINK_MAX_EVENT_PAYLOAD)

/* A decoded packet. `payload` points into the decoder and stays valid until
 * the next byte is fed to it. */
typedef struct {
    uint8_t kind;
    uint32_t id;
    size_t len;
    const uint8_t *payload;
} link_packet_t;

typedef struct {
    uint8_t raw[LINK_MAX_COMMAND_RAW];
    size_t len;
    bool escape, active;
} link_decoder_t;

uint32_t link_crc32(const uint8_t *bytes, size_t len);

/* Frames a packet into `out`. Returns the encoded length, or 0 when the
 * payload is too large or does not fit in `capacity` bytes. */
size_t link_encode(uint8_t kind, uint32_t id, const uint8_t *data, size_t len, uint8_t *out,
                   size_t capacity);

/* Feed one received byte; returns true when `out` holds a complete valid packet. */
bool link_decode(link_decoder_t *decoder, uint8_t byte, link_packet_t *out);

/* Little-endian field access for payloads. */
static inline uint16_t link_get16(const uint8_t *p) { return (uint16_t)(p[0] | p[1] << 8); }
static inline uint32_t link_get32(const uint8_t *p) {
    return p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}
static inline void link_put16(uint8_t *p, uint16_t v) {
    p[0] = (uint8_t)v;
    p[1] = (uint8_t)(v >> 8);
}
static inline void link_put32(uint8_t *p, uint32_t v) {
    link_put16(p, (uint16_t)v);
    link_put16(p + 2, (uint16_t)(v >> 16));
}
