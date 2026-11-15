/* USB framing between the host server and this firmware.
 * The host side is crates/picto-relay-link; keep both in step. See docs/esp32-usb.md. */
#pragma once
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define LINK_VERSION 2
#define LINK_BAUD 921600
#define LINK_FRAGMENT_BYTES 120
#define LINK_MAX_BODY (LINK_FRAGMENT_BYTES + 16)
#define LINK_MAX_PAYLOAD 256
#define LINK_MAX_RAW (LINK_MAX_PAYLOAD + 12)
#define LINK_MAX_ENCODED (LINK_MAX_RAW * 2 + 2)
#define LINK_JOIN_LEN 75
#define LINK_STATUS_LEN 44

enum {
    LINK_HELLO = 1,
    LINK_JOIN = 2,
    LINK_LEAVE = 3,
    LINK_PING = 4,
    LINK_BEGIN = 5,
    LINK_BODIES = 6,
    LINK_COMMIT = 7,
    LINK_DISCOVER = 8,
    LINK_RESPONSE = 128,
    LINK_STATUS = 129,
    LINK_RX = 130,
    LINK_GAP = 131,
    LINK_DIAG = 132,
};

typedef struct {
    uint8_t kind;
    uint32_t id;
    size_t len;
    uint8_t payload[LINK_MAX_PAYLOAD];
} link_packet_t;

typedef struct {
    uint8_t raw[LINK_MAX_RAW];
    size_t len;
    bool escape, active;
} link_decoder_t;

uint32_t link_crc32(const uint8_t *bytes, size_t len);

/* Returns the encoded length, or 0 when the payload is too large. */
size_t link_encode(uint8_t kind, uint32_t id, const uint8_t *data, size_t len,
                   uint8_t out[LINK_MAX_ENCODED]);

/* Feed one received byte; returns true when `out` holds a complete valid packet. */
bool link_decode(link_decoder_t *decoder, uint8_t byte, link_packet_t *out);

/* Wire length of the first complete packed record, or 0 if invalid or truncated. */
size_t link_record_len(const uint8_t *records, size_t len);

/* Decodes exactly one packed record into a body; returns its length or 0. */
size_t link_unpack_body(const uint8_t *record, size_t len, uint8_t out[LINK_MAX_BODY]);
