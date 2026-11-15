#include "client.h"
#include <string.h>

/* Nintendo's fixed group addresses: host polls, host acknowledgements, client replies. */
static const uint8_t COMMAND[6] = {3, 9, 0xbf, 0, 0, 0};
static const uint8_t ACK[6] = {3, 9, 0xbf, 0, 0, 3};
static const uint8_t REPLY[6] = {3, 9, 0xbf, 0, 0, 0x10};
static const uint8_t BROADCAST[6] = {255, 255, 255, 255, 255, 255};

enum {
    FC_ASSOCIATION_REQUEST = 0x1000,
    FC_AUTHENTICATION = 0x10b0,
    FC_DEAUTHENTICATION = 0x10c0,
    FC_REPLY = 0x1118,
    FC_RETRY = 0x0800,
};

#define SECOND 1000000
#define SESSION_TIMEOUT (5 * SECOND) /* Without any frame from the selected host. */
#define MANAGEMENT_RETRY (SECOND / 2)
#define COMPLETION_TIMEOUT (SECOND / 10)
#define ACK_WINDOW 5000

/* ---- 802.11 helpers ---------------------------------------------------- */

PICTO_INLINE uint16_t le16(const uint8_t *p) { return (uint16_t)(p[0] | p[1] << 8); }

PICTO_INLINE void put16(uint8_t *p, uint16_t v) {
    p[0] = (uint8_t)v;
    p[1] = (uint8_t)(v >> 8);
}

PICTO_INLINE bool same_mac(const uint8_t *a, const uint8_t *b) { return memcmp(a, b, 6) == 0; }

static PICTO_HOT void header(uint8_t *out, uint16_t fc, const uint8_t host[6],
                             const uint8_t station[6], uint16_t sequence) {
    memset(out, 0, 24);
    put16(out, fc);
    put16(out + 2, 0xa2);
    memcpy(out + 4, host, 6);
    memcpy(out + 10, station, 6);
    memcpy(out + 16, host, 6);
    put16(out + 22, (uint16_t)(sequence << 4));
}

/* Finds a beacon information element; returns its value and sets *value_len. */
static const uint8_t *beacon_element(const uint8_t *p, size_t len, uint8_t id,
                                     bool (*match)(const uint8_t *, size_t, uint8_t), uint8_t arg) {
    if (len < 36 || (p[0] & 0xfc) != 0x80) return NULL;
    size_t i = 36;
    while (i + 2 <= len) {
        size_t n = p[i + 1];
        if (i + 2 + n > len) return NULL;
        if (p[i] == id && match(p + i + 2, n, arg)) return p + i + 2;
        i += 2 + n;
    }
    return NULL;
}

static bool is_room(const uint8_t *v, size_t n, uint8_t room) {
    static const uint8_t oui[4] = {0, 9, 0xbf, 0};
    return n == 32 && !memcmp(v, oui, 4) && v[18] == 8 && v[19] == 1 && v[28] == room;
}

static bool is_channel(const uint8_t *v, size_t n, uint8_t unused) {
    (void)unused;
    return n == 1 && v[0] >= 1 && v[0] <= 14;
}

/* Nintendo's 32-byte vendor element for a PictoChat room, or NULL.
 * Byte 29 is the member count; bytes 12..17 identify the session. */
static const uint8_t *room_element(const uint8_t *p, size_t len, uint8_t room) {
    return beacon_element(p, len, 221, is_room, room);
}

static uint8_t beacon_channel(const uint8_t *p, size_t len) {
    const uint8_t *v = beacon_element(p, len, 3, is_channel, 0);
    return v ? v[0] : 0;
}

/* ---- Discovery --------------------------------------------------------- */

void discovery_clear(discovery_t *d) {
    for (int i = 0; i < 4; i++) atomic_store_explicit(&d->rooms[i], 0, memory_order_relaxed);
}

void discovery_observe(discovery_t *d, const uint8_t *frame, size_t len, uint32_t seconds,
                       uint8_t channel) {
    if (channel == 0 || beacon_channel(frame, len) != channel) return;
    for (uint8_t room = 0; room < 4; room++) {
        const uint8_t *v = room_element(frame, len, room);
        if (v && v[29] >= 1 && v[29] <= 16)
            atomic_store_explicit(&d->rooms[room], (seconds & 0xffffff) << 8 | v[29],
                                  memory_order_relaxed);
    }
}

void discovery_snapshot(const discovery_t *d, uint32_t seconds, uint8_t out[4]) {
    for (int i = 0; i < 4; i++) {
        uint32_t entry = atomic_load_explicit(&d->rooms[i], memory_order_relaxed);
        bool stale = ((seconds - (entry >> 8)) & 0xffffff) > 3;
        out[i] = (entry & 255) == 0 || stale ? 255 : (uint8_t)entry;
    }
}

/* ---- Profile exchange -------------------------------------------------- */

/* The two identity passes observed during a real DS client initialization.
 * `phase` counts acknowledged steps; CLIENT_PHASE_READY means complete. */
PICTO_HOT size_t client_profile_reply(const client_t *c, uint8_t phase, const uint8_t *p,
                                      size_t len, uint8_t out[LINK_MAX_BODY], uint8_t *next) {
    memset(out, 0, LINK_MAX_BODY);
    out[1] = 0x80;
    *next = phase;
    if (len < 34) return 2;
    uint16_t id = le16(p + 28) & 0x7fff;
    bool roster = id == 0x1c34, control = id == 0x1d0a;
    int subtype = id == 0x1e30 && len >= 128 && p[42] == 3 ? p[43] : -1;

    if ((phase == 0 || (phase == 1 && p[30] == 5 && p[31] == 0)) && roster) {
        static const uint8_t hello[10] = {0x34, 0x8c, 6, 0, 0x68, 0, 0x11, 0x19, 0x4e, 0xbb};
        memset(out, 0, LINK_MAX_BODY);
        memcpy(out, hello, sizeof(hello));
        out[106] = 1;
        *next = 1;
        return 108;
    }
    if ((phase == 1 && subtype == 0) || (phase == 7 && control)) {
        static const uint8_t initial[24] = {0x0a, 0x8d, 0, 0,    0x14, 0,    0,    0xd0,
                                            0xff, 0xff, 0x54, 0, 0,    0,    0,    0,
                                            0,    0,    0xd8, 0x26, 0x61, 0x45, 1, 0};
        static const uint8_t repeat[24] = {0x0a, 0x8d, 0, 0,    0x14, 0,    0,    0,
                                           0xff, 0xff, 0x54, 0, 0,    0,    0x0c, 0,
                                           0,    0,    0xbb, 0x93, 0x33, 0xd1, 3, 0};
        memcpy(out, phase == 1 ? initial : repeat, 24);
        out[6] = c->slot;
        *next = phase + 1;
        return 24;
    }
    if ((phase == 2 && control) || (phase == 5 && roster) || (phase == 8 && subtype == 1)) {
        *next = phase + 1;
        return 0;
    }
    if ((phase == 3 && control) || (phase == 9 && roster)) {
        /* Identity record: MAC, nickname, personal message and color. */
        static const uint8_t identity[14] = {0x30, 0x8e, 2, 0, 0x60, 0, 0, 0x69, 0x54, 1, 0, 0, 0, 0};
        bool final_pass = phase == 9;
        memset(out, 0, LINK_MAX_BODY);
        memcpy(out, identity, sizeof(identity));
        out[1] = final_pass ? 0x0e : 0x8e;
        out[6] = c->slot;
        out[14] = 3;
        out[15] = final_pass;
        for (int i = 0; i < 6; i += 2) {
            out[16 + i] = c->station[i + 1];
            out[17 + i] = c->station[i];
        }
        memcpy(out + 22, c->nickname, 20);
        memcpy(out + 42, c->profile_message, 52);
        put16(out + 94, c->profile_color);
        out[96] = 3;
        out[97] = 2;
        out[98] = final_pass ? 2 : 1;
        *next = phase + 1;
        return 100;
    }
    if ((phase == 4 && subtype == 0) || (phase == 10 && subtype == 1)) {
        *next = phase + 1;
        return 2;
    }
    if ((phase == 6 || phase == 11) && roster) {
        static const uint8_t done[24] = {0x0a, 0x8d, 3,    0,    0x14, 0,    0,    4,
                                         0xff, 0xff, 0xdd, 0x93, 0x32, 2,    1,    0xff,
                                         0xff, 0xff, 0xbb, 0xc8, 0x36, 2,    2,    0};
        memcpy(out, done, 24);
        out[6] = c->slot;
        if (phase == 11) {
            out[1] = 0x0d;
            out[22] = 4;
        }
        *next = phase + 1;
        return 24;
    }
    return 2;
}

/* ---- Session control --------------------------------------------------- */

void client_init(client_t *c, const uint8_t station[6]) {
    memset(c, 0, sizeof(*c));
    memcpy(c->station, station, 6);
    c->sequence = 2;
}

PICTO_HOT bool client_active(const client_t *c) { return c->state != CLIENT_OFF; }

static uint32_t echo_count(const client_t *c) {
    uint32_t count = 0;
    for (int i = 0; i < 5; i++) count += (uint32_t)__builtin_popcount(c->echoed[i]);
    return count;
}

/* Bodies are: announcement, fragments, completion control. */
PICTO_INLINE uint32_t fragment_count(const client_t *c) {
    return c->upload.count >= 2 ? c->upload.count - 2u : 0;
}

/* A drawing reply is due: joined, another member present, and bodies remain. */
PICTO_INLINE bool drawing_due(const client_t *c) {
    return c->phase == CLIENT_PHASE_READY && c->member_seen && c->upload.ready &&
           c->message < c->upload.count;
}

client_status_t client_status(const client_t *c) {
    client_status_t s = c->stats;
    s.state = c->state;
    s.phase = c->phase;
    s.message = c->message;
    s.slot = c->slot;
    s.echoed_fragments = echo_count(c);
    /* MAC acknowledgements alone are not delivery: the host must also echo
     * the announcement and every fragment. It never echoes the completion. */
    s.delivered = c->announcement_seen && s.echoed_fragments == fragment_count(c) &&
                  c->message == c->upload.count && c->upload.ready;
    return s;
}

bool client_configure(client_t *c, uint8_t room, const uint8_t nickname[20],
                      const uint8_t message[52], uint8_t color) {
    if (client_active(c) || c->in_flight.present || c->leave_pending || room > 3 || color > 15)
        return false;
    c->room = room;
    memcpy(c->nickname, nickname, 20);
    memcpy(c->profile_message, message, 52);
    c->profile_color = color;
    return true;
}

static PICTO_HOT void stop(client_t *c, uint32_t reason) {
    c->state = CLIENT_OFF;
    c->pending.present = false;
    c->in_flight.reply.present = false;
    c->in_flight.early_ack = -1;
    c->retained_profile.present = false;
    c->stats.disconnect_reason = reason;
}

void client_start(client_t *c, int64_t now) {
    if (client_active(c) || c->in_flight.present || c->leave_pending) return;
    memset(c->host, 0, 6);
    memset(c->session, 0, 6);
    c->attempts = 0;
    c->slot = 0;
    c->phase = 0;
    c->message = 0;
    c->member_seen = false;
    c->announcement_seen = false;
    memset(c->echoed, 0, sizeof(c->echoed));
    c->pending.present = false;
    c->retained_profile.present = false;
    c->upload.ready = false;
    c->upload.count = 0;
    c->upload.received = 0;
    memset(&c->stats, 0, sizeof(c->stats));
    c->state = CLIENT_DISCOVER;
    c->deadline = now + SESSION_TIMEOUT;
}

void client_leave(client_t *c, int64_t now) {
    /* Tell the host we are going, so it frees the slot immediately. */
    c->leave_pending |= c->state == CLIENT_CONNECTED;
    c->deadline = now + MANAGEMENT_RETRY;
    stop(c, 0);
}

/* ---- Drawing upload ---------------------------------------------------- */

bool client_begin_upload(client_t *c, uint32_t id, uint8_t count) {
    if (c->state != CLIENT_CONNECTED || c->phase != CLIENT_PHASE_READY || count < 3 ||
        count > CLIENT_MAX_BODIES || c->in_flight.present ||
        (c->upload.ready && !client_status(c).delivered))
        return false;
    c->pending.present = false;
    c->upload.ready = false;
    c->upload.count = count;
    c->upload.received = 0;
    c->upload.id = id;
    c->message = 0;
    memset(c->echoed, 0, sizeof(c->echoed));
    c->announcement_seen = false;
    return true;
}

bool client_upload_bodies(client_t *c, uint32_t id, uint8_t first, const uint8_t *records,
                          size_t len) {
    if (c->upload.ready || id != c->upload.id || first != c->upload.received || len == 0)
        return false;
    /* Validate the whole batch first so a retry never sees a partial upload. */
    uint8_t body[LINK_MAX_BODY];
    size_t count = 0;
    for (size_t i = 0; i < len; count++) {
        size_t n = link_record_len(records + i, len - i);
        if (n == 0 || link_unpack_body(records + i, n, body) == 0) return false;
        i += n;
    }
    if (first + count > c->upload.count) return false;
    for (size_t i = 0; i < len;) {
        size_t n = link_record_len(records + i, len - i);
        uint8_t slot = c->upload.received++;
        c->upload.lengths[slot] = (uint8_t)link_unpack_body(records + i, n, c->upload.bodies[slot]);
        i += n;
    }
    return true;
}

bool client_commit_upload(client_t *c, uint32_t id) {
    if (id != c->upload.id || c->upload.received != c->upload.count || c->upload.count < 3)
        return false;
    c->upload.ready = true;
    return true;
}

static PICTO_HOT size_t drawing_body(const client_t *c, uint8_t stage, uint8_t *out) {
    if (stage >= c->upload.count) return 0;
    size_t n = c->upload.lengths[stage];
    memcpy(out, c->upload.bodies[stage], n);
    return n;
}

/* ---- Radio ------------------------------------------------------------- */

static PICTO_HOT size_t reply_frame(const client_t *c, const uint8_t *body, size_t n,
                                    uint8_t out[CLIENT_MAX_FRAME]) {
    header(out, FC_REPLY, c->host, c->station, c->sequence);
    put16(out + 2, (uint16_t)(1030 - 4 * n)); /* Remaining reply window. */
    memcpy(out + 16, REPLY, 6);
    memcpy(out + 24, body, n);
    return 24 + n;
}

size_t client_prepare_reply(const client_t *c, uint8_t out[CLIENT_MAX_FRAME]) {
    if (c->state != CLIENT_CONNECTED || c->in_flight.present || !drawing_due(c)) return 0;
    /* After a completed transmission that still awaits its host ACK this is
     * the same body again, which makes a missed round cheap to retry. */
    uint8_t body[LINK_MAX_BODY];
    size_t n = drawing_body(c, c->message, body);
    return reply_frame(c, body, n, out);
}

static PICTO_HOT void commit(client_t *c, client_pending_t pending, bool accepted) {
    if (accepted) {
        c->stats.ack_ok++;
        c->phase = pending.phase;
        c->message = pending.message;
        c->retained_profile.present = false;
    } else {
        c->stats.ack_missed++;
    }
}

/* Driver completion, not API acceptance. A reply commits only when both the
 * driver reports success and the host acknowledges that round, in either order. */
PICTO_HOT void client_tx_complete(client_t *c, uint16_t sequence, bool success) {
    if (!c->in_flight.present || c->in_flight.sequence != sequence) return;
    client_flight_t flight = c->in_flight;
    c->in_flight.present = false;
    if (!success || !flight.reply.present) return;
    if (flight.early_ack >= 0)
        commit(c, flight.reply, flight.early_ack);
    else
        c->pending = flight.reply;
}

static PICTO_HOT void submit(client_t *c, const uint8_t *frame, size_t len, int64_t now,
                             const client_pending_t *reply, client_tx_fn tx, void *context) {
    if (!tx(context, frame, len)) return;
    c->in_flight = (client_flight_t){
        .present = true,
        .sequence = le16(frame + 22) >> 4,
        .submitted_at = now,
        .early_ack = -1,
    };
    if (reply) {
        c->in_flight.reply = *reply;
        c->stats.replies++;
    }
}

PICTO_INLINE uint16_t next_sequence(uint16_t sequence) { return (sequence + 1) & 0xfff; }

/* Timed work: completion timeout, the leave frame, and join retries. */
PICTO_HOT void client_tick(client_t *c, int64_t now, client_tx_fn tx, void *context) {
    uint8_t out[CLIENT_MAX_FRAME] = {0};
    if (c->in_flight.present) {
        /* Fail closed: a lost completion must never permit a second queued
         * frame. A late completion releases the guard but cannot commit. */
        if (now - c->in_flight.submitted_at >= COMPLETION_TIMEOUT) stop(c, 0x10000);
        return;
    }
    if (c->leave_pending) {
        if (now >= c->deadline) {
            c->leave_pending = false;
            return;
        }
        header(out, FC_DEAUTHENTICATION, c->host, c->station, c->sequence);
        put16(out + 24, 3); /* Reason: station is leaving. */
        submit(c, out, 26, now, NULL, tx, context);
        if (c->in_flight.present) {
            c->sequence = next_sequence(c->sequence);
            c->leave_pending = false;
        }
        return;
    }
    if (now < c->deadline) return;
    if (c->state == CLIENT_DISCOVER || c->state == CLIENT_CONNECTED) {
        stop(c, 0);
        return;
    }
    if (c->state != CLIENT_AUTHENTICATING && c->state != CLIENT_ASSOCIATING) return;
    if (c->attempts == 3) {
        stop(c, 0);
        return;
    }
    uint16_t retry = c->attempts > 0 ? FC_RETRY : 0;
    size_t n;
    if (c->state == CLIENT_AUTHENTICATING) {
        static const uint8_t open_system[6] = {0, 0, 1, 0, 0, 0};
        header(out, FC_AUTHENTICATION | retry, c->host, c->station, c->sequence);
        memcpy(out + 24, open_system, 6);
        n = 30;
    } else {
        static const uint8_t capability[4] = {0x21, 0, 1, 0};
        static const uint8_t rates[4] = {1, 2, 0x82, 0x84};
        header(out, FC_ASSOCIATION_REQUEST | retry, c->host, c->station, c->sequence);
        memcpy(out + 24, capability, 4);
        out[29] = 32; /* SSID element length; the SSID carries the session ID. */
        memcpy(out + 30, c->session, 6);
        memcpy(out + 62, rates, 4);
        n = 66;
    }
    c->attempts++;
    c->deadline = now + MANAGEMENT_RETRY;
    c->sequence = next_sequence(c->sequence);
    submit(c, out, n, now, NULL, tx, context);
}

/* The host rebroadcasts what it accepted. Record exact matches of our bodies. */
static PICTO_HOT void observe_echo(client_t *c, const uint8_t *body, size_t len) {
    if (len < 2) return;
    uint16_t id = le16(body);
    uint8_t expected[LINK_MAX_BODY] = {0};
    if ((id & 0x7fff) == 0x1d0a) {
        drawing_body(c, 0, expected);
        if (len >= 22 && !memcmp(body + 6, expected + 6, 16)) c->announcement_seen = true;
    } else if ((id & 0x7f00) == 0x1e00 && len >= 14) {
        /* The first fragment defines the stride; the last may be shorter. */
        size_t offset = le16(body + 10);
        size_t stride = c->upload.lengths[1] > 16 ? c->upload.lengths[1] - 16u : 1;
        size_t index = offset / stride;
        if (index >= fragment_count(c) || offset % stride) return;
        size_t n = drawing_body(c, (uint8_t)(index + 1), expected);
        if (n < 16) return;
        n -= 16;
        if (len >= 14 + n && !memcmp(body + 8, expected + 8, 6) &&
            !memcmp(body + 14, expected + 14, n))
            c->echoed[index / 32] |= 1u << (index % 32);
    }
}

static PICTO_HOT void connected_data(client_t *c, const uint8_t *p, size_t len, int64_t now,
                                     client_tx_fn tx, void *context) {
    uint16_t mask = le16(p + 26), sequence = le16(p + 22) >> 4;
    uint16_t slot_bit = (uint16_t)(1u << c->slot);

    if (same_mac(p + 4, ACK)) {
        /* A set bit means the host missed that slot's reply. */
        bool accepted = !(mask & slot_bit);
        client_pending_t *pending = &c->pending, *flying = &c->in_flight.reply;
        if (pending->present && sequence == pending->sequence && now >= pending->sent_at &&
            now - pending->sent_at < ACK_WINDOW) {
            pending->present = false;
            commit(c, *pending, accepted);
        } else if (c->in_flight.present && flying->present && sequence == flying->sequence &&
                   now >= flying->sent_at && now - flying->sent_at < ACK_WINDOW &&
                   c->in_flight.early_ack < 0) {
            /* RX may precede the completion callback. Do not commit yet. */
            c->in_flight.early_ack = accepted;
        }
        return;
    }
    if (p[0] != 0x28 || !same_mac(p + 4, COMMAND) || !(mask & slot_bit)) return;

    /* A poll addressed to our slot. */
    c->stats.polls++;
    c->pending.present = false;
    if (c->in_flight.present) {
        /* The driver can defer a queued reply into a later poll's response
         * window, so a missed earlier round does not mean it was sent. Keep
         * the same payload and correlate it with this latest single-slot
         * round. A positive result already received stands until completion. */
        if (c->in_flight.early_ack != 1) {
            c->in_flight.early_ack = -1;
            if (mask != slot_bit) {
                c->in_flight.reply.present = false;
            } else if (c->in_flight.reply.present) {
                c->in_flight.reply.sequence = next_sequence(sequence);
                c->in_flight.reply.sent_at = now;
            }
        }
        return;
    }
    /* Multi-slot ordering is deliberately unsupported until measured. */
    if (mask != slot_bit) return;

    uint8_t body[LINK_MAX_BODY] = {0};
    size_t n;
    client_pending_t reply = {
        .present = true,
        .sequence = next_sequence(sequence),
        .sent_at = now,
        .message = c->message,
    };
    if (c->retained_profile.present) {
        n = c->retained_profile.length;
        memcpy(body, c->retained_profile.bytes, n);
        reply.phase = c->retained_profile.next_phase;
    } else {
        n = client_profile_reply(c, c->phase, p, len, body, &reply.phase);
        /* A later poll may carry a different payload before this reply is
         * acknowledged. Keep retrying the same logical reply; an idle round
         * must not replace an unresolved profile step. */
        if (reply.phase != c->phase) {
            c->retained_profile.present = true;
            memcpy(c->retained_profile.bytes, body, sizeof(body));
            c->retained_profile.length = n;
            c->retained_profile.next_phase = reply.phase;
        }
    }
    if (drawing_due(c)) {
        n = drawing_body(c, c->message, body);
        reply.message = c->message + 1;
    }
    uint8_t out[CLIENT_MAX_FRAME];
    size_t frame_len = reply_frame(c, body, n, out);
    c->sequence = next_sequence(c->sequence);
    submit(c, out, frame_len, now, &reply, tx, context);
}

static PICTO_HOT void receive(client_t *c, const uint8_t *p, size_t len, int64_t now,
                              client_tx_fn tx, void *context) {
    if (!client_active(c) || len < 24) return;
    if (c->state == CLIENT_DISCOVER) {
        const uint8_t *room = room_element(p, len, c->room);
        if (room) {
            memcpy(c->host, p + 10, 6);
            memcpy(c->session, room + 12, 6);
            c->state = CLIENT_AUTHENTICATING;
            c->attempts = 0;
            c->deadline = 0;
        }
        return;
    }
    if (!same_mac(p + 10, c->host)) return;
    bool from_host = same_mac(p + 16, c->host);
    bool addressed = same_mac(p + 4, c->station) && from_host;
    uint8_t subtype = p[0] & 0xfc;

    /* Deauthentication or disassociation. A broadcast one only counts once
     * connected: hosts send cleanup broadcasts while we are still joining. */
    if ((subtype == 0xc0 || subtype == 0xa0) &&
        (addressed || (c->state == CLIENT_CONNECTED && same_mac(p + 4, BROADCAST) && from_host))) {
        stop(c, len >= 26 ? le16(p + 24) : 0);
        return;
    }
    if (c->state == CLIENT_AUTHENTICATING) {
        /* Open-system authentication response, transaction 2. */
        if (!addressed || subtype != 0xb0 || len < 30 || le16(p + 24) != 0 || le16(p + 26) != 2)
            return;
        uint16_t status = le16(p + 28);
        if (status != 0) {
            stop(c, status);
        } else {
            c->state = CLIENT_ASSOCIATING;
            c->attempts = 0;
            c->deadline = 0;
        }
    } else if (c->state == CLIENT_ASSOCIATING) {
        if (!addressed || subtype != 0x10 || len < 30) return;
        uint16_t status = le16(p + 26), aid = le16(p + 28) & 0x3fff;
        if (status != 0 || aid < 1 || aid > 15) {
            stop(c, status);
        } else {
            c->slot = (uint8_t)aid;
            c->state = CLIENT_CONNECTED;
            c->deadline = now + SESSION_TIMEOUT;
        }
    } else if (c->state == CLIENT_CONNECTED) {
        c->deadline = now + SESSION_TIMEOUT;
        const uint8_t *room = room_element(p, len, c->room);
        if (room) {
            c->member_seen |= room[29] >= 2;
            return;
        }
        /* Only data frames from the host's distribution system remain. */
        if (len < 28 || (p[0] & 0x0c) != 8 || (p[1] & 3) != 2 || !from_host) return;
        if (c->phase == CLIENT_PHASE_READY && c->upload.ready && same_mac(p + 4, COMMAND))
            observe_echo(c, p + 28, len - 28);
        connected_data(c, p, len, now, tx, context);
    }
}

/* RX entry point. Ticks afterwards so join steps complete without waiting for
 * the control task. */
PICTO_HOT void client_receive(client_t *c, const uint8_t *frame, size_t len, int64_t now,
                              client_tx_fn tx, void *context) {
    receive(c, frame, len, now, tx, context);
    client_tick(c, now, tx, context);
}
