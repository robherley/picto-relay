#include "bridge.h"
#include <stdatomic.h>
#include <string.h>
#include "client.h"
#include "diag.h"
#include "link.h"

#define MILLISECOND 1000
/* Time spent listening on each channel while searching for rooms. */
#define CHANNEL_DWELL (250 * MILLISECOND)
/* Channels searched: 1 to 13. */
#define CHANNEL_COUNT 13
/* Largest forwarded DS application body: a 256-byte frame less its headers. */
#define RX_BODY_MAX 228
/* 802.11 header plus the host's 4-byte poll prefix precede the application body. */
#define RX_BODY_OFFSET 28

#ifndef PICTO_BUILD
#define PICTO_BUILD "unknown"
#endif

#if PICTO_DIAGNOSTICS
atomic_uint picto_diag[DIAG_COUNT];
#endif

/* The client is shared by the Wi-Fi task (RX, TX completion) and the control
 * task (USB commands, ticks). Access is a try-lock: nobody waits. The Wi-Fi
 * task drops one observation on contention rather than block. */
static client_t client;
static atomic_flag busy = ATOMIC_FLAG_INIT;
/* A TX completion that arrived while the client was locked: bit 16 set,
 * bit 17 success, low 12 bits sequence. One frame is in flight at a time,
 * so one slot is enough. */
static atomic_uint completion;
static atomic_bool leave_requested, tracing;
static atomic_uint usb_drops;

/* Beacons heard on the tuned channel, written by the Wi-Fi task. */
static discovery_t discovery;
static atomic_uchar tuned_channel;

/* Kinds of DS packet forwarded to the host. The DS host repeats each payload
 * many times; only a change within a kind is forwarded. */
enum { RX_ROSTER, RX_CONTROL, RX_FRAGMENT, RX_KINDS };

/* Control-task state; never touched from the Wi-Fi task. */
static struct {
    link_decoder_t decoder;
    bool attached;
    int64_t last_seen, last_status;

    /* Channel search, used by a join (until the room is found) and by SCAN. */
    uint8_t plan[CHANNEL_COUNT], hop;
    int64_t next_hop;
    bool scanning;
    uint8_t scan_counts[LINK_ROOM_COUNT];
    /* Channel each room was last heard on, tried first by the next join. */
    uint8_t room_channel[LINK_ROOM_COUNT];

    struct {
        uint8_t bytes[RX_BODY_MAX];
        size_t len;
    } last_rx[RX_KINDS];

    /* Latest successful command, replayed if the host retries it. */
    uint32_t last_id, last_hash;
    uint8_t response[1 + LINK_HELLO_BUILD_OFFSET + LINK_MAX_BUILD_LEN];
    size_t response_len;
} usb;

static PICTO_HOT bool transmit(void *context, const uint8_t *frame, size_t len,
                               uint32_t delay_us) {
    (void)context;
    return picto_platform_tx(frame, len, delay_us);
}

static PICTO_HOT bool take_completion(void) {
    unsigned event = atomic_exchange_explicit(&completion, 0, memory_order_acq_rel);
    if (!(event & 0x10000)) return false;
    client_tx_complete(&client, event & 0xfff, event & 0x20000);
    return true;
}

static PICTO_HOT bool lock_client(void) {
    if (atomic_flag_test_and_set_explicit(&busy, memory_order_acquire)) return false;
    take_completion();
    return true;
}

static PICTO_HOT void unlock_client(void) {
    /* A completion can arrive while we held the lock (even synchronously
     * from our own TX). Consume it before anyone else has to wait a tick. */
    if (take_completion()) client_tick(&client, picto_platform_now(), transmit, NULL);
    atomic_flag_clear_explicit(&busy, memory_order_release);
}

static void emit(uint8_t kind, uint32_t id, const uint8_t *data, size_t len) {
    uint8_t bytes[LINK_MAX_EVENT_ENCODED];
    size_t n = len <= LINK_MAX_EVENT_PAYLOAD ? link_encode(kind, id, data, len, bytes, sizeof(bytes))
                                             : 0;
    if (n == 0 || !picto_platform_usb(bytes, n))
        atomic_fetch_add_explicit(&usb_drops, 1, memory_order_relaxed);
}

/* ---- Wi-Fi task -------------------------------------------------------- */

void bridge_init(void) {
    uint8_t mac[6];
    picto_platform_mac(mac);
    client_init(&client, mac);
}

PICTO_HOT void bridge_receive(const uint8_t *frame, size_t len) {
    if (!frame || len < 24 || len > 2346) return;
    if ((frame[0] & 0xfc) == 0x80)
        discovery_observe(&discovery, frame, len,
                          atomic_load_explicit(&tuned_channel, memory_order_relaxed));
    if (!lock_client()) return; /* The control task has it; skip this frame. */
    client_receive(&client, frame, len, picto_platform_now(), transmit, NULL);
    unlock_client();
}

PICTO_HOT void bridge_tx_complete(uint16_t sequence, bool success) {
    atomic_store_explicit(&completion, 0x10000u | (sequence & 0xfff) | (success ? 0x20000u : 0),
                          memory_order_release);
    /* If a submission still holds the lock, its unlock drains the mailbox. */
    if (!lock_client()) return;
    client_tick(&client, picto_platform_now(), transmit, NULL);
    unlock_client();
}

void bridge_prepare_reply(void) {
    if (!lock_client()) return;
    uint8_t frame[CLIENT_MAX_FRAME];
    size_t n = client_prepare_reply(&client, frame);
    picto_radio_prepare_reply(frame, n);
    unlock_client();
}

void bridge_usb_drop(void) { atomic_fetch_add_explicit(&usb_drops, 1, memory_order_relaxed); }

PICTO_HOT bool bridge_tracing(void) {
    return atomic_load_explicit(&tracing, memory_order_relaxed);
}

void bridge_trace(const uint8_t *frame, size_t len, size_t air_len, uint32_t timestamp, bool sent) {
    if (!bridge_tracing() || !usb.attached) return;
    uint8_t event[LINK_MAX_EVENT_PAYLOAD];
    if (len > LINK_FRAME_MAX_BYTES) len = LINK_FRAME_MAX_BYTES;
    link_put32(event + LINK_FRAME_TIME_OFFSET, timestamp);
    event[LINK_FRAME_FLAGS_OFFSET] = sent ? LINK_FRAME_FLAG_SENT : 0;
    link_put16(event + LINK_FRAME_LENGTH_OFFSET, (uint16_t)air_len);
    memcpy(event + LINK_FRAME_DATA_OFFSET, frame, len);
    emit(LINK_FRAME, 0, event, LINK_FRAME_DATA_OFFSET + len);
}

/* ---- Channel search (control task, client locked) ---------------------- */

static bool tune(uint8_t channel) {
    if (!picto_platform_channel(channel)) return false;
    discovery_clear(&discovery);
    atomic_store_explicit(&tuned_channel, channel, memory_order_relaxed);
    client_set_channel(&client, channel);
    return true;
}

/* Search order: `first` if known, then the channels a DS host usually picks,
 * then the rest. Every channel appears exactly once. */
static void plan_search(uint8_t first) {
    static const uint8_t usual[] = {1, 7, 13};
    bool planned[CHANNEL_COUNT + 1] = {false};
    size_t n = 0;
    if (first >= 1 && first <= CHANNEL_COUNT) {
        usb.plan[n++] = first;
        planned[first] = true;
    }
    for (size_t i = 0; i < sizeof(usual); i++)
        if (!planned[usual[i]]) {
            usb.plan[n++] = usual[i];
            planned[usual[i]] = true;
        }
    for (uint8_t channel = 1; channel <= CHANNEL_COUNT; channel++)
        if (!planned[channel]) usb.plan[n++] = channel;
}

static bool start_search(uint8_t first, int64_t now) {
    plan_search(first);
    usb.hop = 0;
    usb.next_hop = now + CHANNEL_DWELL;
    return tune(usb.plan[0]);
}

/* Moves to the next channel when the dwell is over. Returns true when a SCAN
 * has just covered every channel. */
static bool search_step(int64_t now) {
    bool joining = client.state == CLIENT_DISCOVER;
    if (!joining && !usb.scanning) return false;
    if (now < usb.next_hop) return false;
    if (usb.scanning) {
        uint8_t counts[LINK_ROOM_COUNT];
        discovery_snapshot(&discovery, counts);
        for (int room = 0; room < LINK_ROOM_COUNT; room++)
            if (counts[room] != LINK_NO_BEACON) {
                usb.scan_counts[room] = counts[room];
                usb.room_channel[room] = usb.plan[usb.hop];
            }
        if (usb.hop + 1 == CHANNEL_COUNT) {
            usb.scanning = false;
            return true;
        }
    }
    /* A join keeps cycling until the client finds the room or gives up. */
    usb.hop = (usb.hop + 1) % CHANNEL_COUNT;
    usb.next_hop = now + CHANNEL_DWELL;
    tune(usb.plan[usb.hop]);
    return false;
}

/* ---- Reports to the host (control task) -------------------------------- */

/* Everything the host should be told, copied out while the client is locked
 * and written to USB after it is released. */
typedef struct {
    uint8_t events;
    uint32_t delivered_id;
    uint8_t ready[LINK_READY_LEN], left[LINK_LEFT_LEN];
    bool status_due;
    uint32_t status_id;
    uint8_t status[LINK_STATUS_LEN];
    bool rooms_due;
    uint8_t rooms[LINK_ROOM_COUNT];
} report_t;

static void collect(report_t *report, int64_t now) {
    report->events = client.events;
    client.events = 0;
    if (report->events & CLIENT_EVENT_READY) {
        memcpy(report->ready + LINK_READY_HOST_OFFSET, client.host, LINK_MAC_LEN);
        report->ready[LINK_READY_SLOT_OFFSET] = client.slot;
        report->ready[LINK_READY_ROOM_OFFSET] = client.room;
        report->ready[LINK_READY_CHANNEL_OFFSET] = client.channel;
        usb.room_channel[client.room] = client.channel;
    }
    if (report->events & CLIENT_EVENT_DELIVERED) report->delivered_id = client.delivered_id;
    if (report->events & CLIENT_EVENT_LEFT) {
        report->left[LINK_LEFT_REASON_OFFSET] = client.left_reason;
        link_put16(report->left + LINK_LEFT_CODE_OFFSET, client.left_code);
    }
    client_progress_t progress = client_progress(&client);
    /* Report faster while there is drawing progress to show. */
    int64_t interval = MILLISECOND * (progress.id ? LINK_STATUS_SENDING_INTERVAL_MS
                                                  : LINK_STATUS_INTERVAL_MS);
    report->status_due = now - usb.last_status >= interval;
    if (!report->status_due) return;
    usb.last_status = now;
    report->status_id = progress.id;
    uint8_t *status = report->status;
    status[LINK_STATUS_STATE_OFFSET] = (uint8_t)client.state;
    status[LINK_STATUS_PHASE_OFFSET] = client.phase;
    status[LINK_STATUS_SLOT_OFFSET] = client.slot;
    status[LINK_STATUS_ROOM_OFFSET] = client.room;
    status[LINK_STATUS_CHANNEL_OFFSET] = client.channel;
    status[LINK_STATUS_SENT_OFFSET] = progress.sent;
    status[LINK_STATUS_TOTAL_OFFSET] = progress.total;
    status[LINK_STATUS_ECHOED_OFFSET] = progress.echoed;
    const uint32_t counters[LINK_STATUS_COUNTERS] = {client.counters.polls, client.counters.replies,
                                                     client.counters.ack_ok,
                                                     client.counters.ack_missed};
    for (int i = 0; i < LINK_STATUS_COUNTERS; i++)
        link_put32(status + LINK_STATUS_COUNTERS_OFFSET + 4 * i, counters[i]);
}

static void send(const report_t *report) {
    if (!usb.attached) return; /* Nobody to tell. */
    if (report->events & CLIENT_EVENT_READY) emit(LINK_READY, 0, report->ready, LINK_READY_LEN);
    if (report->events & CLIENT_EVENT_DELIVERED) emit(LINK_DELIVERED, report->delivered_id, NULL, 0);
    if (report->events & CLIENT_EVENT_LEFT) emit(LINK_LEFT, 0, report->left, LINK_LEFT_LEN);
    if (report->rooms_due) emit(LINK_ROOMS, 0, report->rooms, LINK_ROOM_COUNT);
    if (!report->status_due) return;
    emit(LINK_STATUS, report->status_id, report->status, LINK_STATUS_LEN);
#if PICTO_DIAGNOSTICS
    uint8_t diag[DIAG_COUNT * 4];
    for (int i = 0; i < DIAG_COUNT; i++)
        link_put32(diag + 4 * i, atomic_load_explicit(&picto_diag[i], memory_order_relaxed));
    emit(LINK_DIAG, report->status_id, diag, sizeof(diag));
#endif
}

void bridge_tick(void) {
    int64_t now = picto_platform_now();
    if (usb.attached && now - usb.last_seen > (int64_t)LINK_HOST_TIMEOUT_MS * MILLISECOND) {
        usb.attached = false;
        atomic_store_explicit(&tracing, false, memory_order_relaxed);
        atomic_store_explicit(&leave_requested, true, memory_order_release);
    }
    if (!lock_client()) return;
    if (atomic_exchange_explicit(&leave_requested, false, memory_order_acq_rel)) {
        usb.scanning = false;
        client_leave(&client, now);
    }
    client_tick(&client, now, transmit, NULL);
    report_t report = {0};
    if (search_step(now)) {
        report.rooms_due = true;
        memcpy(report.rooms, usb.scan_counts, sizeof(report.rooms));
    }
    collect(&report, now);
    unlock_client();
    send(&report);
}

/* ---- Forwarding DS packets to the host (control task) ------------------ */

void bridge_forward(const uint8_t *frame, size_t len) {
    static const uint8_t command[6] = {3, 9, 0xbf, 0, 0, 0};
    if (!frame || len < RX_BODY_OFFSET + 6 || len > RX_BODY_OFFSET + RX_BODY_MAX) return;
    if (!lock_client()) {
        bridge_usb_drop();
        return;
    }
    bool wanted = usb.attached && client_active(&client) && !memcmp(frame + 4, command, 6) &&
                  !memcmp(frame + 10, client.host, 6);
    unlock_client();
    if (!wanted) return;

    /* Forward rosters, controls and fragments; drop idle polls. */
    const uint8_t *body = frame + RX_BODY_OFFSET;
    size_t body_len = len - RX_BODY_OFFSET;
    uint16_t id = link_get16(body) & 0x7fff;
    int kind = id == 0x1c34 ? RX_ROSTER : id == 0x1d0a ? RX_CONTROL
                                      : (id & 0x7f00) == 0x1e00 ? RX_FRAGMENT : -1;
    if (kind < 0) return;

    /* The host repeats each payload with a changing flag bit in the packet ID,
     * and rosters also carry counters around the 96-byte member list. Compare
     * only the part that carries information. */
    uint8_t stable[RX_BODY_MAX];
    size_t stable_len = body_len;
    memcpy(stable, body, body_len);
    stable[1] &= 0x7f;
    if (kind == RX_ROSTER && body_len >= 106) {
        memmove(stable, stable + 10, 96);
        stable_len = 96;
    }
    if (usb.last_rx[kind].len == stable_len && !memcmp(usb.last_rx[kind].bytes, stable, stable_len))
        return;
    memcpy(usb.last_rx[kind].bytes, stable, stable_len);
    usb.last_rx[kind].len = stable_len;
    emit(LINK_RX, 0, body, body_len);
}

/* ---- Commands from the host (control task) ----------------------------- */

static size_t hello_data(uint8_t *data) {
    static const char build[] = PICTO_BUILD;
    size_t build_len = sizeof(build) - 1;
    if (build_len > LINK_MAX_BUILD_LEN) build_len = LINK_MAX_BUILD_LEN;
    data[LINK_HELLO_VERSION_OFFSET] = LINK_PROTOCOL_VERSION;
    memcpy(data + LINK_HELLO_MAC_OFFSET, client.station, LINK_MAC_LEN);
    memcpy(data + LINK_HELLO_BUILD_OFFSET, build, build_len);
    return LINK_HELLO_BUILD_OFFSET + build_len;
}

/* Runs one command with the client locked. Returns a LINK_STATUS_* code and
 * sets *data_len to the response data written to `data`. */
static uint8_t command(const link_packet_t *p, uint8_t *data, size_t *data_len) {
    const uint8_t *payload = p->payload;
    int64_t now = picto_platform_now();
    if (p->kind == LINK_HELLO) {
        /* Always answered with our version, so a mismatch is reported, not timed out. */
        *data_len = hello_data(data);
        atomic_store_explicit(&tracing, false, memory_order_relaxed); /* A new host starts clean. */
        usb.attached = p->len == 1 && payload[0] == LINK_PROTOCOL_VERSION;
        return usb.attached ? LINK_STATUS_OK : LINK_STATUS_VERSION_MISMATCH;
    }
    if (!usb.attached) return LINK_STATUS_NOT_ATTACHED;
    switch (p->kind) {
    case LINK_PING:
        return LINK_STATUS_OK;
    case LINK_TRACE:
        if (p->len != 1 || payload[0] > 1) return LINK_STATUS_INVALID;
        atomic_store_explicit(&tracing, payload[0], memory_order_relaxed);
        return LINK_STATUS_OK;
    case LINK_LEAVE:
        usb.scanning = false;
        client_leave(&client, now);
        return LINK_STATUS_OK;
    case LINK_JOIN: {
        if (p->len != LINK_JOIN_LEN) return LINK_STATUS_INVALID;
        uint8_t room = payload[LINK_JOIN_ROOM_OFFSET];
        uint8_t status = client_join(&client, room, payload + LINK_JOIN_NICKNAME_OFFSET,
                                     payload + LINK_JOIN_MESSAGE_OFFSET,
                                     payload[LINK_JOIN_COLOR_OFFSET], now);
        if (status != LINK_STATUS_OK) return status;
        usb.scanning = false;
        memset(usb.last_rx, 0, sizeof(usb.last_rx));
        if (!start_search(usb.room_channel[room], now)) {
            client_leave(&client, now);
            return LINK_STATUS_BUSY;
        }
        return LINK_STATUS_OK;
    }
    case LINK_SCAN:
        if (p->len != 0) return LINK_STATUS_INVALID;
        if (client_active(&client)) return LINK_STATUS_WRONG_STATE;
        /* Do not tune away while the previous session's leave is still going out. */
        if (client.leave_pending || client.in_flight.present) return LINK_STATUS_BUSY;
        memset(usb.scan_counts, LINK_NO_BEACON, sizeof(usb.scan_counts));
        usb.scanning = start_search(0, now);
        return usb.scanning ? LINK_STATUS_OK : LINK_STATUS_BUSY;
    case LINK_DRAW:
        if (p->len <= LINK_DRAW_DATA_OFFSET) return LINK_STATUS_INVALID;
        return client_upload(&client, link_get32(payload + LINK_DRAW_ID_OFFSET),
                             link_get16(payload + LINK_DRAW_POSITION_OFFSET),
                             link_get16(payload + LINK_DRAW_TOTAL_OFFSET),
                             payload + LINK_DRAW_DATA_OFFSET, p->len - LINK_DRAW_DATA_OFFSET);
    default:
        return LINK_STATUS_INVALID;
    }
}

void bridge_usb_byte(uint8_t byte) {
    link_packet_t packet;
    if (!link_decode(&usb.decoder, byte, &packet)) return;
    usb.last_seen = picto_platform_now();

    /* The host retries until it sees a response. Replay the previous success
     * instead of running the command twice. */
    uint32_t hash = link_crc32(packet.payload, packet.len) ^ packet.kind;
    if (usb.response_len > 0 && packet.id == usb.last_id && hash == usb.last_hash) {
        emit(LINK_RESPONSE, packet.id, usb.response, usb.response_len);
        return;
    }
    uint8_t response[sizeof(usb.response)] = {LINK_STATUS_BUSY};
    size_t data_len = 0;
    if (lock_client()) {
        response[0] = command(&packet, response + 1, &data_len);
        unlock_client();
    }
    if (response[0] == LINK_STATUS_OK) {
        usb.last_id = packet.id;
        usb.last_hash = hash;
        memcpy(usb.response, response, sizeof(response));
        usb.response_len = 1 + data_len;
    }
    emit(LINK_RESPONSE, packet.id, response, 1 + data_len);

    unsigned drops = atomic_exchange_explicit(&usb_drops, 0, memory_order_relaxed);
    if (drops > 0) {
        uint8_t count[4];
        link_put32(count, drops);
        emit(LINK_GAP, 0, count, sizeof(count));
    }
}
