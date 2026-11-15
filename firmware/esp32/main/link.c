#include "link.h"
#include <string.h>

/* Reflected polynomial of the IEEE 802.3 CRC32. */
#define CRC32_POLYNOMIAL 0xedb88320u

uint32_t link_crc32(const uint8_t *bytes, size_t len) {
    uint32_t crc = UINT32_MAX;
    for (size_t i = 0; i < len; i++) {
        crc ^= bytes[i];
        for (int bit = 0; bit < 8; bit++) crc = (crc >> 1) ^ (CRC32_POLYNOMIAL & (0u - (crc & 1)));
    }
    return ~crc;
}

/* Appends one byte to the wire form, escaping it if needed. */
static bool put(uint8_t byte, uint8_t *out, size_t capacity, size_t *n) {
    bool escape = byte == LINK_DELIMITER || byte == LINK_ESCAPE;
    if (*n + (escape ? 2 : 1) > capacity) return false;
    if (escape) {
        out[(*n)++] = LINK_ESCAPE;
        byte ^= LINK_ESCAPE_XOR;
    }
    out[(*n)++] = byte;
    return true;
}

size_t link_encode(uint8_t kind, uint32_t id, const uint8_t *data, size_t len, uint8_t *out,
                   size_t capacity) {
    if (len > LINK_MAX_COMMAND_PAYLOAD || capacity < 2) return 0;
    uint8_t header[LINK_PAYLOAD_OFFSET], trailer[LINK_CRC_LEN];
    header[LINK_KIND_OFFSET] = kind;
    link_put32(header + LINK_ID_OFFSET, id);
    link_put16(header + LINK_LENGTH_OFFSET, (uint16_t)len);

    /* CRC over header then payload, without assembling them in one buffer. */
    uint32_t crc = UINT32_MAX;
    const uint8_t *parts[2] = {header, data};
    const size_t lengths[2] = {sizeof(header), len};
    for (int part = 0; part < 2; part++)
        for (size_t i = 0; i < lengths[part]; i++) {
            crc ^= parts[part][i];
            for (int bit = 0; bit < 8; bit++)
                crc = (crc >> 1) ^ (CRC32_POLYNOMIAL & (0u - (crc & 1)));
        }
    link_put32(trailer, ~crc);

    size_t n = 0;
    out[n++] = LINK_DELIMITER;
    for (size_t i = 0; i < sizeof(header); i++)
        if (!put(header[i], out, capacity, &n)) return 0;
    for (size_t i = 0; i < len; i++)
        if (!put(data[i], out, capacity, &n)) return 0;
    for (size_t i = 0; i < sizeof(trailer); i++)
        if (!put(trailer[i], out, capacity, &n)) return 0;
    if (n >= capacity) return 0;
    out[n++] = LINK_DELIMITER;
    return n;
}

static bool finish(const link_decoder_t *d, link_packet_t *out) {
    if (d->len < LINK_OVERHEAD) return false;
    size_t size = link_get16(d->raw + LINK_LENGTH_OFFSET);
    size_t payload_end = LINK_PAYLOAD_OFFSET + size;
    if (d->len != payload_end + LINK_CRC_LEN) return false;
    if (link_crc32(d->raw, payload_end) != link_get32(d->raw + payload_end)) return false;
    out->kind = d->raw[LINK_KIND_OFFSET];
    out->id = link_get32(d->raw + LINK_ID_OFFSET);
    out->len = size;
    out->payload = d->raw + LINK_PAYLOAD_OFFSET;
    return true;
}

bool link_decode(link_decoder_t *d, uint8_t byte, link_packet_t *out) {
    if (byte == LINK_DELIMITER) {
        bool complete = d->active && !d->escape && finish(d, out);
        d->len = 0;
        d->escape = false;
        d->active = true;
        return complete;
    }
    if (!d->active) return false;
    if (d->escape) {
        d->escape = false;
        byte ^= LINK_ESCAPE_XOR;
    } else if (byte == LINK_ESCAPE) {
        d->escape = true;
        return false;
    }
    if (d->len == sizeof(d->raw)) {
        d->active = false; /* Oversized: wait for the next delimiter. */
        return false;
    }
    d->raw[d->len++] = byte;
    return false;
}
