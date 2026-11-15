#include "link.h"
#include <string.h>

uint32_t link_crc32(const uint8_t *bytes, size_t len) {
    uint32_t crc = ~0u;
    for (size_t i = 0; i < len; i++) {
        crc ^= bytes[i];
        for (int bit = 0; bit < 8; bit++) crc = (crc >> 1) ^ (0xedb88320u & (0u - (crc & 1)));
    }
    return ~crc;
}

static void put32(uint8_t *p, uint32_t v) {
    p[0] = (uint8_t)v;
    p[1] = (uint8_t)(v >> 8);
    p[2] = (uint8_t)(v >> 16);
    p[3] = (uint8_t)(v >> 24);
}

static uint32_t get32(const uint8_t *p) {
    return p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}

size_t link_encode(uint8_t kind, uint32_t id, const uint8_t *data, size_t len,
                   uint8_t out[LINK_MAX_ENCODED]) {
    if (len > LINK_MAX_PAYLOAD) return 0;
    uint8_t raw[LINK_MAX_RAW];
    raw[0] = LINK_VERSION;
    raw[1] = kind;
    put32(raw + 2, id);
    raw[6] = (uint8_t)len;
    raw[7] = (uint8_t)(len >> 8);
    if (len) memcpy(raw + 8, data, len);
    put32(raw + 8 + len, link_crc32(raw, 8 + len));
    size_t n = 0;
    out[n++] = 0x7e;
    for (size_t i = 0; i < len + 12; i++) {
        if (raw[i] == 0x7e || raw[i] == 0x7d) {
            out[n++] = 0x7d;
            out[n++] = raw[i] ^ 0x20;
        } else {
            out[n++] = raw[i];
        }
    }
    out[n++] = 0x7e;
    return n;
}

static bool finish(const link_decoder_t *d, link_packet_t *out) {
    if (d->len < 12 || d->raw[0] != LINK_VERSION) return false;
    size_t size = d->raw[6] | (size_t)d->raw[7] << 8;
    if (size > LINK_MAX_PAYLOAD || d->len != size + 12) return false;
    if (link_crc32(d->raw, d->len - 4) != get32(d->raw + d->len - 4)) return false;
    out->kind = d->raw[1];
    out->id = get32(d->raw + 2);
    out->len = size;
    memset(out->payload, 0, sizeof(out->payload));
    memcpy(out->payload, d->raw + 8, size);
    return true;
}

bool link_decode(link_decoder_t *d, uint8_t byte, link_packet_t *out) {
    if (byte == 0x7e) {
        bool complete = d->active && !d->escape && finish(d, out);
        d->len = 0;
        d->escape = false;
        d->active = true;
        return complete;
    }
    if (!d->active) return false;
    if (d->escape) {
        d->escape = false;
        byte ^= 0x20;
    } else if (byte == 0x7d) {
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

/* A record is a tag byte followed by data. Tag 1..127: that many raw bytes.
 * Tag with the high bit set: that many zero-run-encoded bytes, where a nonzero
 * byte is literal and `0, N` expands to N zeros. Tag 0: extended raw record
 * `0, length, body` for bodies of 128 bytes or more. */
size_t link_record_len(const uint8_t *records, size_t len) {
    if (len < 1) return 0;
    size_t n;
    if (records[0] == 0) {
        if (len < 2 || records[1] < 128 || records[1] > LINK_MAX_BODY) return 0;
        n = (size_t)records[1] + 2;
    } else {
        n = (size_t)(records[0] & 0x7f) + 1;
    }
    return len >= n ? n : 0;
}

size_t link_unpack_body(const uint8_t *record, size_t len, uint8_t out[LINK_MAX_BODY]) {
    if (len == 0 || link_record_len(record, len) != len) return 0;
    uint8_t tag = record[0];
    const uint8_t *bytes = record + (tag == 0 ? 2 : 1);
    size_t count = len - (tag == 0 ? 2 : 1);
    if (!(tag & 0x80)) {
        if (count < 16 || count > LINK_MAX_BODY) return 0;
        memcpy(out, bytes, count);
        return count;
    }
    size_t n = 0;
    for (size_t i = 0; i < count;) {
        uint8_t value = bytes[i++];
        size_t run = 1;
        if (value == 0) {
            if (i >= count) return 0;
            run = bytes[i++];
        }
        if (run == 0 || n + run > LINK_MAX_BODY) return 0;
        memset(out + n, value, run);
        n += run;
    }
    return n >= 16 ? n : 0;
}
