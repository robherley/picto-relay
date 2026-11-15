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

/* Application packet IDs, with the flag bit of unknown meaning masked off. */
enum {
    PACKET_ID_MASK = 0x7fff,
    PACKET_ROSTER = 0x1c34,   /* Participant roster. */
    PACKET_CONTROL = 0x1d0a,  /* Transfer announcement or completion. */
    PACKET_IDENTITY = 0x1e30, /* A profile record in a fragment. */
    PACKET_FAMILY_MASK = 0x7f00,
    PACKET_FRAGMENT = 0x1e00, /* Any transfer fragment. */
};

#define SECOND 1000000
#define SESSION_TIMEOUT (5 * SECOND) /* Without any frame from the selected host. */
#define MANAGEMENT_RETRY (SECOND / 2)
#define MANAGEMENT_ATTEMPTS 3
#define COMPLETION_TIMEOUT (SECOND / 10)
/* Slack allowed for the host's acknowledgement after the last reply window. */
#define ACK_WINDOW 5000
/* Gap the host leaves between consecutive clients' reply windows. */
#define REPLY_WINDOW_GAP 10
/* Longest per-client reply window we will wait through for an earlier slot. */
#define MAX_CLIENT_TIME 4000
/* Time from the end of a poll to the start of our reply on the air. A DS
 * manages about 16 us; this radio's driver needs about ten times that. */
#define REPLY_TURNAROUND 180
/* Airtime of a reply with `n` body bytes at 2 Mbps: short preamble, 24-byte
 * header, body and FCS, in microseconds. */
#define REPLY_AIRTIME(n) (96 + 4 * (24 + (n) + 4))
/* What a DS adds to the remaining windows in a reply's duration field: 22 us
 * in ordinary rounds and 16 us in short-window ones, as captured. */
#define DURATION_MARGIN 22
#define DURATION_MARGIN_SHORT 16
/* Frame number of a session's first drawing fragment; the profile exchange used the earlier ones. */
#define FIRST_DRAWING_FRAME 3
/* IEEE reason code we give when leaving. */
#define REASON_LEAVING 3

/* ---- 802.11 helpers ---------------------------------------------------- */

PICTO_INLINE uint16_t le16(const uint8_t *p) { return (uint16_t)(p[0] | p[1] << 8); }

PICTO_INLINE void put16(uint8_t *p, uint16_t v) {
    p[0] = (uint8_t)v;
    p[1] = (uint8_t)(v >> 8);
}

PICTO_INLINE void put32(uint8_t *p, uint32_t v) {
    put16(p, (uint16_t)v);
    put16(p + 2, (uint16_t)(v >> 16));
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

/* Finds a beacon information element accepted by `match`; returns its value. */
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

/* The channel a beacon says it was sent on, or 0. Promiscuous RX can decode
 * neighboring channels, so the tuned channel alone is not proof. */
static uint8_t beacon_channel(const uint8_t *p, size_t len) {
    const uint8_t *v = beacon_element(p, len, 3, is_channel, 0);
    return v ? v[0] : 0;
}

/* ---- Discovery --------------------------------------------------------- */

void discovery_clear(discovery_t *d) {
    for (int i = 0; i < LINK_ROOM_COUNT; i++)
        atomic_store_explicit(&d->counts[i], 0, memory_order_relaxed);
}

void discovery_observe(discovery_t *d, const uint8_t *frame, size_t len, uint8_t channel) {
    if (channel == 0 || beacon_channel(frame, len) != channel) return;
    for (uint8_t room = 0; room < LINK_ROOM_COUNT; room++) {
        const uint8_t *v = room_element(frame, len, room);
        if (v && v[29] >= 1 && v[29] <= 16)
            atomic_store_explicit(&d->counts[room], v[29], memory_order_relaxed);
    }
}

void discovery_snapshot(const discovery_t *d, uint8_t out[LINK_ROOM_COUNT]) {
    for (int i = 0; i < LINK_ROOM_COUNT; i++) {
        uint8_t count = atomic_load_explicit(&d->counts[i], memory_order_relaxed);
        out[i] = count ? count : LINK_NO_BEACON;
    }
}

/* ---- Joining ----------------------------------------------------------- */

/* The roster's member table: sixteen 6-byte entries, the host first, each MAC
 * stored as three byte-swapped words. Offsets are into the application body. */
#define ROSTER_TABLE_OFFSET 10
#define ROSTER_SLOTS 16
#define ROSTER_TAIL_OFFSET (ROSTER_TABLE_OFFSET + ROSTER_SLOTS * 6)

static void swapped_mac(const uint8_t mac[6], uint8_t out[6]) {
    for (int i = 0; i < 6; i += 2) {
        out[i] = mac[i + 1];
        out[i + 1] = mac[i];
    }
}

/* Number of clients (members other than the host) in a roster, or 0 if the
 * roster does not list `station`. */
static uint8_t roster_clients(const uint8_t *body, size_t len, const uint8_t station[6]) {
    uint8_t ours[6], members = 0;
    bool listed = false;
    if (len < ROSTER_TAIL_OFFSET) return 0;
    swapped_mac(station, ours);
    for (int slot = 0; slot < ROSTER_SLOTS; slot++) {
        const uint8_t *entry = body + ROSTER_TABLE_OFFSET + slot * 6;
        static const uint8_t empty[6] = {0};
        if (!memcmp(entry, empty, 6)) continue;
        members++;
        listed |= !memcmp(entry, ours, 6);
    }
    return listed && members > 1 ? members - 1 : 0;
}

/* A joining client answers the roster with a packet of the same shape whose
 * table is empty and whose tail is the number of clients in the room. (A DS
 * joining a room that already held one client was captured sending 2; one
 * joining an empty room sends 1.) The four bytes after the header vary
 * between real clients and are replayed from a capture. */
size_t client_hello_body(uint8_t clients, uint8_t out[CLIENT_MAX_BODY]) {
    static const uint8_t hello[10] = {0x34, 0x8c, 6, 0, 0x68, 0, 0x11, 0x19, 0x4e, 0xbb};
    memset(out, 0, CLIENT_MAX_BODY);
    memcpy(out, hello, sizeof(hello));
    out[ROSTER_TAIL_OFFSET] = clients;
    return ROSTER_TAIL_OFFSET + 2;
}

/* ---- Transfers --------------------------------------------------------- */

/* Our profile record, as carried in a profile transfer. */
#define PROFILE_BYTES 84
#define PROFILE_SUBTYPE_OFFSET 1 /* 0: introducing ourselves; 1: answering a newcomer. */
#define PROFILE_MAC_OFFSET 2

static void profile_record(const client_t *c, uint8_t subtype, uint8_t out[PROFILE_BYTES]) {
    memset(out, 0, PROFILE_BYTES);
    out[0] = 3; /* Record marker. */
    out[PROFILE_SUBTYPE_OFFSET] = subtype;
    swapped_mac(c->station, out + PROFILE_MAC_OFFSET);
    memcpy(out + 8, c->nickname, LINK_NICKNAME_LEN);
    memcpy(out + 28, c->profile_message, LINK_PROFILE_MESSAGE_LEN);
    put16(out + 80, c->profile_color);
    out[82] = 3; /* Two bytes of unknown meaning, as a DS sent them. */
    out[83] = 2;
}

PICTO_INLINE const client_drawing_t *sending_drawing(const client_t *c) {
    return &c->drawings[c->sending_slot];
}

/* Stages of a transfer: announcement, fragments, completion. */
PICTO_INLINE uint8_t stage_count(const client_t *c) { return c->transfer.fragment_count + 2; }

/* Announcement and completion (`d0a` controls): a 6-byte envelope (packet ID
 * 0x8d0a, sequence, declared length 20) and an 18-byte payload of slot,
 * marker, 0xffff, three u32 fields and a transfer counter. The host ignores a
 * transfer whose counters repeat an earlier one, so they advance through the
 * session the way a real DS client's do. */
#define CONTROL_BODY_BYTES 24
#define CONTROL_SLOT_OFFSET 6
#define CONTROL_MARKER_OFFSET 7
#define CONTROL_FIELDS_OFFSET 10
#define CONTROL_COUNTER_OFFSET 22
/* Values replayed from captures. The command field of a DS's first transfer is
 * 0 and of its second 12; drawings then advance it by 3. How a real DS picks
 * the nonce is unknown; any value that changes works. */
#define INTRODUCTION_MARKER 0xd0
#define INTRODUCTION_NONCE 0x456126d8u
#define ANSWER_COMMAND 12
#define ANSWER_NONCE 0xd13393bbu
#define DRAWING_COMMAND 12
#define DRAWING_COMMAND_STEP 3
#define DRAWING_NONCE 0x75f03ccau
#define NONCE_MULTIPLIER 0x9e3779b9u
/* An announcement the host has not echoed after this long is sent again. */
#define OPEN_TIMEOUT 500000

static PICTO_HOT size_t announcement_body(const client_t *c, uint8_t *out) {
    static const uint8_t template[CONTROL_FIELDS_OFFSET] = {0x0a, 0x8d, 0, 0,    0x14,
                                                            0,    0,    0, 0xff, 0xff};
    const client_transfer_t *t = &c->transfer;
    memcpy(out, template, sizeof(template));
    out[CONTROL_SLOT_OFFSET] = c->slot;
    out[CONTROL_MARKER_OFFSET] = t->marker;
    put32(out + CONTROL_FIELDS_OFFSET, t->length);
    put32(out + CONTROL_FIELDS_OFFSET + 4, t->command);
    put32(out + CONTROL_FIELDS_OFFSET + 8, t->nonce);
    put16(out + CONTROL_COUNTER_OFFSET, t->counter);
    return CONTROL_BODY_BYTES;
}

static PICTO_HOT size_t completion_body(const client_t *c, uint8_t *out) {
    /* Envelope sequence 3, marker 4, then three fields replayed from a
     * capture whose meaning is unknown. */
    static const uint8_t template[CONTROL_COUNTER_OFFSET] = {
        0x0a, 0x8d, 3, 0,    0x14, 0,    0,    4,    0xff, 0xff, 0xdd,
        0x93, 0x32, 2, 1,    0xff, 0xff, 0xff, 0xbb, 0xc8, 0x36, 2};
    memcpy(out, template, sizeof(template));
    out[CONTROL_SLOT_OFFSET] = c->slot;
    put16(out + CONTROL_COUNTER_OFFSET, (uint16_t)(c->transfer.counter + 1));
    return CONTROL_BODY_BYTES;
}

/* Fragment body: 6-byte envelope, 8-byte fragment header, data, frame number. */
#define FRAGMENT_SLOT_OFFSET 6      /* Who the data is from. */
#define FRAGMENT_MARKER_OFFSET 7    /* Varies; replayed from captures. */
#define FRAGMENT_LENGTH_OFFSET 8    /* Data bytes in this fragment. */
#define FRAGMENT_FINAL_OFFSET 9     /* 1 on the last fragment. */
#define FRAGMENT_POSITION_OFFSET 10 /* Byte offset of the data in the transfer. */
#define FRAGMENT_TAG_OFFSET 12      /* Opaque transfer tag; a DS client sends zeros. */
#define FRAGMENT_DATA_OFFSET 14

PICTO_INLINE size_t fragment_length(const client_t *c, size_t index) {
    size_t remaining = c->transfer.length - index * CLIENT_FRAGMENT_BYTES;
    return remaining < CLIENT_FRAGMENT_BYTES ? remaining : CLIENT_FRAGMENT_BYTES;
}

static PICTO_HOT size_t fragment_body(const client_t *c, size_t index, uint8_t *out) {
    const client_transfer_t *t = &c->transfer;
    bool profile = t->kind == CLIENT_TRANSFER_PROFILE;
    size_t n = fragment_length(c, index), position = index * CLIENT_FRAGMENT_BYTES;
    /* The envelope declares the payload plus 2: header, data and frame
     * number, minus the envelope itself. The ID's low byte is that in halfwords. */
    size_t declared = n + CLIENT_FRAGMENT_OVERHEAD - 4;
    out[0] = (uint8_t)(declared / 2);
    out[1] = 0x8e;
    put16(out + 2, 2); /* Envelope sequence of every fragment. */
    put16(out + 4, (uint16_t)declared);
    out[FRAGMENT_SLOT_OFFSET] = c->slot;
    out[FRAGMENT_MARKER_OFFSET] = profile ? 0x69 : index == 0 ? 0x6c : 4;
    out[FRAGMENT_LENGTH_OFFSET] = (uint8_t)n;
    out[FRAGMENT_FINAL_OFFSET] = index + 1 == t->fragment_count;
    put16(out + FRAGMENT_POSITION_OFFSET, (uint16_t)position);
    put16(out + FRAGMENT_TAG_OFFSET, 0);
    if (profile)
        profile_record(c, t->profile_subtype, out + FRAGMENT_DATA_OFFSET);
    else
        memcpy(out + FRAGMENT_DATA_OFFSET, sending_drawing(c)->bytes + position, n);
    put16(out + FRAGMENT_DATA_OFFSET + n, (uint16_t)(t->first_frame + index));
    return n + CLIENT_FRAGMENT_OVERHEAD;
}

PICTO_HOT size_t client_transfer_body(const client_t *c, uint8_t stage,
                                      uint8_t out[CLIENT_MAX_BODY]) {
    if (c->transfer.kind == CLIENT_TRANSFER_NONE || stage >= stage_count(c)) return 0;
    if (stage == 0) return announcement_body(c, out);
    if (stage == stage_count(c) - 1) return completion_body(c, out);
    return fragment_body(c, stage - 1u, out);
}

static uint32_t echo_count(const client_t *c) {
    uint32_t count = 0;
    for (size_t i = 0; i < sizeof(c->transfer.echoed) / sizeof(c->transfer.echoed[0]); i++)
        count += (uint32_t)__builtin_popcount(c->transfer.echoed[i]);
    return count;
}

static void begin_transfer(client_t *c, client_transfer_kind_t kind, uint16_t length) {
    client_transfer_t *t = &c->transfer;
    uint32_t generation = t->generation + 1;
    memset(t, 0, sizeof(*t));
    t->kind = kind;
    t->generation = generation;
    t->length = length;
    t->fragment_count = (uint8_t)((length + CLIENT_FRAGMENT_BYTES - 1) / CLIENT_FRAGMENT_BYTES);
    /* The host remembers counters and frame numbers across a session. */
    t->counter = c->next_counter;
    c->next_counter += 2;
    t->first_frame = c->next_frame;
    c->next_frame += t->fragment_count;
}

/* Starts whatever is waiting, if nothing is being sent: our introduction,
 * then answers owed to newcomers, then the staged drawing. */
static void start_next_transfer(client_t *c, int64_t now) {
    client_transfer_t *t = &c->transfer;
    if (t->kind != CLIENT_TRANSFER_NONE || !c->hello_done || now < c->settled_at) return;
    if (c->introduce_pending) {
        c->introduce_pending = false;
        begin_transfer(c, CLIENT_TRANSFER_PROFILE, PROFILE_BYTES);
        t->profile_subtype = 0;
        t->marker = INTRODUCTION_MARKER;
        t->nonce = INTRODUCTION_NONCE;
    } else if (c->answers_pending > 0) {
        c->answers_pending--;
        begin_transfer(c, CLIENT_TRANSFER_PROFILE, PROFILE_BYTES);
        t->profile_subtype = 1;
        t->command = ANSWER_COMMAND;
        t->nonce = ANSWER_NONCE ^ (c->answers_sent++ * NONCE_MULTIPLIER);
    } else if (c->staged && c->phase == LINK_PHASE_READY) {
        uint32_t n = c->drawings_started++;
        c->staged = false;
        c->sending_slot ^= 1;
        c->drawings[c->sending_slot ^ 1].received = 0;
        begin_transfer(c, CLIENT_TRANSFER_DRAWING, sending_drawing(c)->length);
        t->command = DRAWING_COMMAND + n * DRAWING_COMMAND_STEP;
        t->nonce = DRAWING_NONCE ^ (n * NONCE_MULTIPLIER);
    }
}

/* The transfer has a stage to send in answer to a poll right now. */
static PICTO_HOT bool transfer_due(const client_t *c) {
    const client_transfer_t *t = &c->transfer;
    if (t->kind == CLIENT_TRANSFER_NONE || t->stage >= stage_count(c)) return false;
    /* Data follows only once the host has echoed the announcement: a busy
     * host drops fragments of a transfer it has not opened yet. */
    if (t->stage > 0 && !t->opened) return false;
    /* A drawing needs somebody to draw for. */
    return t->kind != CLIENT_TRANSFER_DRAWING || c->member_seen;
}

static void finish_transfer(client_t *c, int64_t now) {
    client_transfer_t *t = &c->transfer;
    if (t->kind == CLIENT_TRANSFER_DRAWING) {
        c->delivered_id = sending_drawing(c)->id;
        c->events |= CLIENT_EVENT_DELIVERED;
    } else if (c->phase != LINK_PHASE_READY) {
        /* Our introduction is out: we are a member like any other. */
        c->phase = LINK_PHASE_READY;
        c->events |= CLIENT_EVENT_READY;
    }
    t->kind = CLIENT_TRANSFER_NONE;
    start_next_transfer(c, now);
}

/* Every stage sent and acknowledged. A profile is then done. A drawing must
 * also have been echoed in full: MAC acknowledgements alone are not delivery.
 * The host never echoes the completion. */
static PICTO_HOT void check_finished(client_t *c, int64_t now) {
    const client_transfer_t *t = &c->transfer;
    if (t->kind == CLIENT_TRANSFER_NONE || t->stage != stage_count(c)) return;
    if (t->kind == CLIENT_TRANSFER_DRAWING && echo_count(c) != t->fragment_count) return;
    finish_transfer(c, now);
}

uint8_t client_upload(client_t *c, uint32_t id, uint16_t position, uint16_t total,
                      const uint8_t *data, size_t len) {
    if (c->state != CLIENT_CONNECTED || c->phase != LINK_PHASE_READY)
        return LINK_STATUS_WRONG_STATE;
    if (total < LINK_MIN_DRAWING_BYTES || total > LINK_MAX_DRAWING_BYTES || len == 0 ||
        position + len > total)
        return LINK_STATUS_INVALID;
    if (c->staged) return LINK_STATUS_BUSY; /* The waiting drawing must start first. */
    client_drawing_t *d = &c->drawings[c->sending_slot ^ 1];
    if (position == 0) {
        d->id = id;
        d->length = total;
        d->received = 0;
    } else if (id != d->id || total != d->length || position != d->received) {
        return LINK_STATUS_INVALID;
    }
    memcpy(d->bytes + position, data, len);
    d->received = (uint16_t)(position + len);
    if (d->received == d->length) {
        c->staged = true;
        start_next_transfer(c, c->settled_at);
    }
    return LINK_STATUS_OK;
}

client_progress_t client_progress(const client_t *c) {
    client_progress_t progress = {0};
    const client_transfer_t *t = &c->transfer;
    if (t->kind != CLIENT_TRANSFER_DRAWING) return progress;
    uint8_t fragments_sent = t->stage > 0 ? t->stage - 1 : 0;
    progress.id = sending_drawing(c)->id;
    progress.total = t->fragment_count;
    progress.sent = fragments_sent < t->fragment_count ? fragments_sent : t->fragment_count;
    progress.echoed = (uint8_t)echo_count(c);
    return progress;
}

/* ---- Session control --------------------------------------------------- */

/* How long a real DS waits between its roster answer and its introduction. */
#define HELLO_SETTLE_TIME 500000
/* The host repeats a newcomer's introduction; answer it once per this long. */
#define ANSWER_HOLDOFF (3 * SECOND)
#define MAX_ANSWERS_PENDING 4

void client_init(client_t *c, const uint8_t station[6]) {
    memset(c, 0, sizeof(*c));
    memcpy(c->station, station, 6);
    c->sequence = 2;
    c->in_flight.early_ack = -1;
}

PICTO_HOT bool client_active(const client_t *c) { return c->state != CLIENT_OFF; }

static PICTO_HOT void stop(client_t *c, uint8_t reason, uint16_t code) {
    if (c->state != CLIENT_OFF) {
        c->events |= CLIENT_EVENT_LEFT;
        c->left_reason = reason;
        c->left_code = code;
    }
    c->state = CLIENT_OFF;
    c->pending.present = false;
    c->in_flight.reply.present = false;
    c->in_flight.early_ack = -1;
    c->transfer.kind = CLIENT_TRANSFER_NONE;
    c->staged = false;
}

uint8_t client_join(client_t *c, uint8_t room, const uint8_t nickname[LINK_NICKNAME_LEN],
                    const uint8_t message[LINK_PROFILE_MESSAGE_LEN], uint8_t color, int64_t now) {
    if (room >= LINK_ROOM_COUNT || color > LINK_MAX_PROFILE_COLOR) return LINK_STATUS_INVALID;
    if (client_active(c)) return LINK_STATUS_WRONG_STATE;
    /* A frame from the previous session (often its leave) is still with the driver. */
    if (c->in_flight.present || c->leave_pending) return LINK_STATUS_BUSY;
    c->room = room;
    memcpy(c->nickname, nickname, LINK_NICKNAME_LEN);
    memcpy(c->profile_message, message, LINK_PROFILE_MESSAGE_LEN);
    c->profile_color = color;

    memset(c->host, 0, 6);
    memset(c->session, 0, 6);
    c->attempts = 0;
    c->slot = 0;
    c->phase = CLIENT_PHASE_HELLO;
    c->channel = 0;
    c->member_seen = false;
    c->pending.present = false;
    c->poll_window = 998 + REPLY_WINDOW_GAP; /* Until a poll says otherwise. */
    c->poll_later = 0;
    c->hello_done = false;
    c->introduce_pending = true;
    c->answers_pending = 0;
    c->answers_sent = 0;
    memset(c->answered, 0, sizeof(c->answered));
    c->transfer.kind = CLIENT_TRANSFER_NONE;
    c->next_counter = 1;
    c->next_frame = 1;
    c->drawings_started = 0;
    c->staged = false;
    c->drawings[0].received = 0;
    c->drawings[1].received = 0;
    c->events = 0;
    memset(&c->counters, 0, sizeof(c->counters));
    c->state = CLIENT_DISCOVER;
    c->deadline = now + CLIENT_DISCOVER_TIMEOUT;
    return LINK_STATUS_OK;
}

void client_set_channel(client_t *c, uint8_t channel) { c->channel = channel; }

void client_leave(client_t *c, int64_t now) {
    /* Tell the host we are going, so it frees the slot immediately. */
    c->leave_pending |= c->state == CLIENT_CONNECTED;
    c->deadline = now + MANAGEMENT_RETRY;
    stop(c, LINK_LEFT_REQUESTED, 0);
}

/* ---- Radio ------------------------------------------------------------- */

/* A window too short for even the idle reply: the host wants bare frames. */
PICTO_INLINE bool short_window(uint32_t client_time) { return REPLY_AIRTIME(2) > client_time; }

static PICTO_HOT size_t reply_frame(const client_t *c, const uint8_t *body, size_t n,
                                    uint8_t out[CLIENT_MAX_FRAME]) {
    /* The duration field covers the rest of this round: what is left of our
     * window and the windows of the clients polled after us. */
    uint32_t client_time = c->poll_window - REPLY_WINDOW_GAP;
    uint32_t duration = (c->poll_later + 1u) * c->poll_window +
                        (short_window(client_time) ? DURATION_MARGIN_SHORT : DURATION_MARGIN) - 4 * n;
    header(out, FC_REPLY, c->host, c->station, c->sequence);
    put16(out + 2, (uint16_t)duration);
    memcpy(out + 16, REPLY, 6);
    memcpy(out + 24, body, n);
    return 24 + n;
}

size_t client_prepare_reply(const client_t *c, uint8_t out[CLIENT_MAX_FRAME]) {
    /* Only drawing stages are worth staging; everything else is occasional. */
    if (c->state != CLIENT_CONNECTED || c->in_flight.present ||
        c->transfer.kind != CLIENT_TRANSFER_DRAWING || !transfer_due(c))
        return 0;
    /* After a completed transmission that still awaits its host ACK this is
     * the same body again, which makes a missed round cheap to retry. */
    uint8_t body[CLIENT_MAX_BODY];
    size_t n = client_transfer_body(c, c->transfer.stage, body);
    return reply_frame(c, body, n, out);
}

/* The host acknowledged (or missed) the reply described by `pending`. */
static PICTO_HOT void commit(client_t *c, client_pending_t pending, bool accepted) {
    client_transfer_t *t = &c->transfer;
    if (!accepted) {
        c->counters.ack_missed++;
        return;
    }
    c->counters.ack_ok++;
    switch (pending.carried) {
    case CLIENT_REPLY_IDLE:
        break;
    case CLIENT_REPLY_HELLO:
        if (!c->hello_done) {
            c->hello_done = true;
            c->phase = CLIENT_PHASE_INTRODUCING;
            c->settled_at = pending.sent_at + HELLO_SETTLE_TIME;
        }
        break;
    case CLIENT_REPLY_TRANSFER:
        /* Advance only if the acknowledged stage is the one we are still on. */
        if (t->kind == CLIENT_TRANSFER_NONE || pending.generation != t->generation ||
            pending.stage != t->stage)
            break;
        t->stage++;
        if (t->stage == 1) t->open_by = pending.sent_at + OPEN_TIMEOUT;
        check_finished(c, pending.sent_at);
        break;
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
                             const client_pending_t *reply, uint32_t delay_us, client_tx_fn tx,
                             void *context) {
    if (!tx(context, frame, len, delay_us)) return;
    c->in_flight = (client_flight_t){
        .present = true,
        .sequence = le16(frame + 22) >> 4,
        .submitted_at = now,
        .early_ack = -1,
    };
    if (reply) {
        c->in_flight.reply = *reply;
        c->counters.replies++;
    }
}

PICTO_INLINE uint16_t next_sequence(uint16_t sequence) { return (sequence + 1) & 0xfff; }

/* Timed work: completion timeout, the leave frame, and join retries. */
PICTO_HOT void client_tick(client_t *c, int64_t now, client_tx_fn tx, void *context) {
    uint8_t out[CLIENT_MAX_FRAME] = {0};
    if (c->in_flight.present) {
        /* Fail closed: a lost completion must never permit a second queued
         * frame. A late completion releases the guard but cannot commit. */
        if (now - c->in_flight.submitted_at >= COMPLETION_TIMEOUT)
            stop(c, LINK_LEFT_RADIO_STALLED, 0);
        return;
    }
    if (c->leave_pending) {
        if (now >= c->deadline) {
            c->leave_pending = false;
            return;
        }
        header(out, FC_DEAUTHENTICATION, c->host, c->station, c->sequence);
        put16(out + 24, REASON_LEAVING);
        submit(c, out, 26, now, NULL, 0, tx, context);
        if (c->in_flight.present) {
            c->sequence = next_sequence(c->sequence);
            c->leave_pending = false;
        }
        return;
    }
    if (now < c->deadline) return;
    switch (c->state) {
    case CLIENT_OFF:
        return;
    case CLIENT_DISCOVER:
        stop(c, LINK_LEFT_NO_HOST, 0);
        return;
    case CLIENT_CONNECTED:
        stop(c, LINK_LEFT_HOST_LOST, 0);
        return;
    case CLIENT_AUTHENTICATING:
    case CLIENT_ASSOCIATING:
        break;
    }
    if (c->attempts == MANAGEMENT_ATTEMPTS) {
        stop(c, LINK_LEFT_JOIN_TIMEOUT, 0);
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
    submit(c, out, n, now, NULL, 0, tx, context);
}

/* A newcomer introduced itself (profile subtype 0). Members answer with their
 * own profile so the newcomer learns who is in the room. The host repeats the
 * introduction, so answer each member once per holdoff. */
static void answer_introduction(client_t *c, const uint8_t mac[6], int64_t now) {
    client_introduction_t *slot = &c->answered[0];
    for (size_t i = 0; i < sizeof(c->answered) / sizeof(c->answered[0]); i++) {
        client_introduction_t *seen = &c->answered[i];
        if (!memcmp(seen->mac, mac, 6) && seen->at != 0) {
            if (now - seen->at < ANSWER_HOLDOFF) return;
            slot = seen;
            break;
        }
        if (seen->at < slot->at) slot = seen; /* Otherwise reuse the oldest. */
    }
    memcpy(slot->mac, mac, 6);
    slot->at = now;
    if (c->answers_pending < MAX_ANSWERS_PENDING) c->answers_pending++;
}

/* What the host broadcasts: its own packets and what it relays from members.
 * Look for the echo of our announcement (which opens our transfer), echoes of
 * our drawing fragments (which prove delivery), and newcomers' introductions. */
static PICTO_HOT void observe_host(client_t *c, const uint8_t *body, size_t len, int64_t now) {
    client_transfer_t *t = &c->transfer;
    if (len < FRAGMENT_DATA_OFFSET) return;
    uint16_t id = le16(body) & PACKET_ID_MASK;
    bool ours = body[FRAGMENT_SLOT_OFFSET] == c->slot;
    if (id == PACKET_CONTROL) {
        /* The host echoes the 16 payload bytes from the slot onwards. */
        uint8_t expected[CONTROL_BODY_BYTES];
        if (t->kind == CLIENT_TRANSFER_NONE || len < CONTROL_COUNTER_OFFSET) return;
        announcement_body(c, expected);
        if (!memcmp(body + CONTROL_SLOT_OFFSET, expected + CONTROL_SLOT_OFFSET, 16))
            t->opened = true;
    } else if (id == PACKET_IDENTITY && !ours && len >= FRAGMENT_DATA_OFFSET + PROFILE_MAC_OFFSET + 6) {
        const uint8_t *record = body + FRAGMENT_DATA_OFFSET;
        uint8_t own[6];
        swapped_mac(c->station, own);
        if (record[0] == 3 && record[PROFILE_SUBTYPE_OFFSET] == 0 &&
            memcmp(record + PROFILE_MAC_OFFSET, own, 6))
            answer_introduction(c, record + PROFILE_MAC_OFFSET, now);
    } else if ((id & PACKET_FAMILY_MASK) == PACKET_FRAGMENT && ours &&
               t->kind == CLIENT_TRANSFER_DRAWING) {
        size_t position = le16(body + FRAGMENT_POSITION_OFFSET);
        size_t index = position / CLIENT_FRAGMENT_BYTES;
        if (index >= t->fragment_count || position % CLIENT_FRAGMENT_BYTES) return;
        size_t n = fragment_length(c, index);
        if (len >= FRAGMENT_DATA_OFFSET + n && body[FRAGMENT_LENGTH_OFFSET] == n &&
            body[FRAGMENT_FINAL_OFFSET] == (index + 1 == t->fragment_count) &&
            le16(body + FRAGMENT_TAG_OFFSET) == 0 &&
            !memcmp(body + FRAGMENT_DATA_OFFSET, sending_drawing(c)->bytes + position, n))
            t->echoed[index / 32] |= 1u << (index % 32);
        check_finished(c, now);
    }
}

static PICTO_HOT void connected_data(client_t *c, const uint8_t *p, size_t len, int64_t now,
                                     client_tx_fn tx, void *context) {
    /* Every host frame starts with the per-client reply window in
     * microseconds and a mask with one bit per slot. */
    uint16_t client_time = le16(p + 24), mask = le16(p + 26), sequence = le16(p + 22) >> 4;
    uint16_t slot_bit = (uint16_t)(1u << c->slot);

    if (same_mac(p + 4, ACK)) {
        /* A set bit means the host missed that slot's reply. */
        bool accepted = !(mask & slot_bit);
        client_pending_t *pending = &c->pending, *flying = &c->in_flight.reply;
        if (pending->present && sequence == pending->sequence && now >= pending->sent_at &&
            now < pending->ack_by) {
            pending->present = false;
            commit(c, *pending, accepted);
        } else if (c->in_flight.present && flying->present && sequence == flying->sequence &&
                   now >= flying->sent_at && now < flying->ack_by && c->in_flight.early_ack < 0) {
            /* RX may precede the completion callback. Do not commit yet. */
            c->in_flight.early_ack = accepted;
        }
        return;
    }
    if (p[0] != 0x28 || !same_mac(p + 4, COMMAND) || !(mask & slot_bit)) return;

    /* A poll that includes our slot. Polled clients answer one after another
     * in slot order, each in its own window of `client_time` plus a small gap.
     * The host sends its acknowledgement after the last window. */
    uint32_t window = (uint32_t)client_time + REPLY_WINDOW_GAP;
    uint16_t clients = mask & (uint16_t)~1u; /* Slot 0 is the host itself. */
    unsigned earlier = (unsigned)__builtin_popcount(clients & (slot_bit - 1));
    unsigned polled = (unsigned)__builtin_popcount(clients);
    int64_t ack_by = now + (int64_t)polled * window + ACK_WINDOW;
    c->counters.polls++;
    c->pending.present = false;
    if (c->in_flight.present) {
        /* The driver can defer a queued reply into a later poll's response
         * window, so a missed earlier round does not mean it was sent. Keep
         * the same payload and correlate it with this latest round. A
         * positive result already received stands until completion. */
        if (c->in_flight.early_ack != 1) {
            c->in_flight.early_ack = -1;
            if (c->in_flight.reply.present) {
                c->in_flight.reply.sequence = next_sequence(sequence);
                c->in_flight.reply.sent_at = now;
                c->in_flight.reply.ack_by = ack_by;
            }
        }
        return;
    }
    /* A window too long to sit out in the receive path is not a real poll. */
    if (earlier > 0 && client_time > MAX_CLIENT_TIME) return;
    unsigned later = polled - earlier - 1;
    c->poll_window = (uint16_t)window;
    c->poll_later = (uint8_t)later;
    /* About once a second the host polls with a window only a bare frame
     * fits, and a DS answers within microseconds. We cannot: our reply would
     * still be on the air in the next client's window and destroy its reply.
     * When somebody follows us, stay out of it. The host counts that round
     * against us but the other DS is unharmed. */
    if (later > 0 && REPLY_TURNAROUND + REPLY_AIRTIME(0) > window) return;

    /* Choose what to say. With nothing to send a client answers `00 80`. */
    uint8_t body[CLIENT_MAX_BODY] = {0, 0x80};
    size_t n = 2;
    client_pending_t reply = {
        .present = true,
        .sequence = next_sequence(sequence),
        .sent_at = now,
        .ack_by = ack_by,
        .carried = CLIENT_REPLY_IDLE,
    };
    client_transfer_t *t = &c->transfer;
    if (!c->hello_done) {
        /* Joining: answer the roster once it lists us. */
        uint8_t clients = 0;
        if (len >= 28 + 2 && (le16(p + 28) & PACKET_ID_MASK) == PACKET_ROSTER)
            clients = roster_clients(p + 28, len - 28, c->station);
        if (clients > 0) {
            n = client_hello_body(clients, body);
            reply.carried = CLIENT_REPLY_HELLO;
        }
    } else {
        start_next_transfer(c, now);
        /* An announcement the host never echoed is sent again. */
        if (t->kind != CLIENT_TRANSFER_NONE && t->stage > 0 && !t->opened && now >= t->open_by)
            t->stage = 0;
        if (transfer_due(c)) {
            n = client_transfer_body(c, t->stage, body);
            reply.carried = CLIENT_REPLY_TRANSFER;
            reply.stage = t->stage;
            reply.generation = t->generation;
        }
    }
    if (REPLY_AIRTIME(n) > client_time) {
        /* A short window: answer with a bare frame, as a DS does. Whatever
         * we had to say waits for an ordinary poll. */
        n = 0;
        reply.carried = CLIENT_REPLY_IDLE;
    }
    uint8_t out[CLIENT_MAX_FRAME];
    size_t frame_len = reply_frame(c, body, n, out);
    c->sequence = next_sequence(c->sequence);
    submit(c, out, frame_len, now, &reply, earlier * window, tx, context);
}

static PICTO_HOT void receive(client_t *c, const uint8_t *p, size_t len, int64_t now,
                              client_tx_fn tx, void *context) {
    if (!client_active(c) || len < 24) return;
    if (c->state == CLIENT_DISCOVER) {
        /* Only lock on to a host that is really on the tuned channel. */
        const uint8_t *room = room_element(p, len, c->room);
        if (room && c->channel != 0 && beacon_channel(p, len) == c->channel) {
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
        stop(c, LINK_LEFT_KICKED, len >= 26 ? le16(p + 24) : 0);
        return;
    }
    if (c->state == CLIENT_AUTHENTICATING) {
        /* Open-system authentication response, transaction 2. */
        if (!addressed || subtype != 0xb0 || len < 30 || le16(p + 24) != 0 || le16(p + 26) != 2)
            return;
        uint16_t status = le16(p + 28);
        if (status != 0) {
            stop(c, LINK_LEFT_REJECTED, status);
        } else {
            c->state = CLIENT_ASSOCIATING;
            c->attempts = 0;
            c->deadline = 0;
        }
    } else if (c->state == CLIENT_ASSOCIATING) {
        if (!addressed || subtype != 0x10 || len < 30) return;
        uint16_t status = le16(p + 26), aid = le16(p + 28) & 0x3fff;
        if (status != 0 || aid < 1 || aid > 15) {
            stop(c, LINK_LEFT_REJECTED, status);
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
        if (same_mac(p + 4, COMMAND)) observe_host(c, p + 28, len - 28, now);
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
