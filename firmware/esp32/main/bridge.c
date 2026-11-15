#include "bridge.h"
#include <stdatomic.h>
#include <string.h>
#include "client.h"
#include "diag.h"
#include "link.h"

#define HOST_TIMEOUT 5000000   /* Leave the room when the host stops sending commands. */
#define STATUS_INTERVAL 250000
#define RX_HISTORY 64

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
static atomic_bool leave_requested;
static atomic_uint rx_drops, usb_drops;

static discovery_t discovery;
static atomic_uchar discovery_channel;

/* Control-task state; never touched from the Wi-Fi task. */
static struct {
    link_decoder_t decoder;
    bool attached;
    int64_t last_seen, next_status;
    /* Host poll bodies already forwarded, to suppress the host's repeats. */
    uint32_t rx_hashes[RX_HISTORY];
    size_t rx_index;
    /* Latest successful command, replayed if the host retries it. */
    uint32_t last_id, last_hash;
    uint8_t response[8];
    size_t response_len;
} usb;

static PICTO_HOT bool transmit(void *context, const uint8_t *frame, size_t len) {
    (void)context;
    return picto_platform_tx(frame, len);
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
    uint8_t bytes[LINK_MAX_ENCODED];
    size_t n = link_encode(kind, id, data, len, bytes);
    if (n == 0 || !picto_platform_usb(bytes, n))
        atomic_fetch_add_explicit(&usb_drops, 1, memory_order_relaxed);
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

/* ---- Wi-Fi task -------------------------------------------------------- */

void bridge_init(void) {
    uint8_t mac[6];
    picto_platform_mac(mac);
    client_init(&client, mac);
}

PICTO_HOT void bridge_receive(const uint8_t *frame, size_t len) {
    if (!frame || len < 24 || len > 2346) return;
    int64_t now = picto_platform_now();
    if ((frame[0] & 0xfc) == 0x80)
        discovery_observe(&discovery, frame, len, (uint32_t)(now / 1000000),
                          atomic_load_explicit(&discovery_channel, memory_order_relaxed));
    if (!lock_client()) {
        atomic_fetch_add_explicit(&rx_drops, 1, memory_order_relaxed);
        return;
    }
    client_receive(&client, frame, len, now, transmit, NULL);
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

/* ---- Control task ------------------------------------------------------ */

static void emit_status(void) {
    client_status_t s = client_status(&client);
    uint8_t data[LINK_STATUS_LEN] = {
        (uint8_t)s.state, (uint8_t)s.phase, (uint8_t)s.message, client.upload.count,
        (uint8_t)s.slot,  client.room,      (uint8_t)s.delivered, 0,
    };
    memcpy(data + 8, client.station, 6);
    memcpy(data + 14, client.host, 6);
    const uint32_t counters[6] = {s.polls,      s.replies,           s.ack_ok,
                                  s.ack_missed, s.disconnect_reason, s.echoed_fragments};
    for (int i = 0; i < 6; i++) put32(data + 20 + 4 * i, counters[i]);
    uint32_t transfer = client.upload.id;
    unlock_client();
    emit(LINK_STATUS, transfer, data, sizeof(data));
#if PICTO_DIAGNOSTICS
    uint8_t diag[DIAG_COUNT * 4];
    for (int i = 0; i < DIAG_COUNT; i++)
        put32(diag + 4 * i, atomic_load_explicit(&picto_diag[i], memory_order_relaxed));
    emit(LINK_DIAG, transfer, diag, sizeof(diag));
#endif
}

void bridge_tick(void) {
    int64_t now = picto_platform_now();
    if (usb.attached && now - usb.last_seen > HOST_TIMEOUT) {
        usb.attached = false;
        atomic_store_explicit(&leave_requested, true, memory_order_release);
    }
    if (!lock_client()) return;
    if (atomic_exchange_explicit(&leave_requested, false, memory_order_acq_rel))
        client_leave(&client, now);
    client_tick(&client, now, transmit, NULL);
    if (now < usb.next_status) {
        unlock_client();
        return;
    }
    usb.next_status = now + STATUS_INTERVAL;
    if (usb.attached)
        emit_status(); /* Unlocks before writing to the host. */
    else
        unlock_client();
}

void bridge_forward(const uint8_t *frame, size_t len) {
    static const uint8_t command[6] = {3, 9, 0xbf, 0, 0, 0};
    if (!frame || len < 34 || len > 256) return;
    if (!lock_client()) {
        bridge_usb_drop();
        return;
    }
    bool wanted = usb.attached && client_active(&client) && !memcmp(frame + 4, command, 6) &&
                  !memcmp(frame + 10, client.host, 6);
    unlock_client();
    if (!wanted) return;

    /* Forward rosters, controls and fragments; drop idle polls. */
    const uint8_t *body = frame + 28;
    size_t body_len = len - 28;
    uint16_t id = (uint16_t)(body[0] | body[1] << 8) & 0x7fff;
    if (id != 0x1c34 && id != 0x1d0a && (id & 0x7f00) != 0x1e00) return;

    /* The host repeats each payload many times with a changing flag bit (and,
     * for rosters, changing counters around the member list). Hash the stable
     * part and forward each distinct payload once. */
    uint8_t stable[228];
    memcpy(stable, body, body_len);
    stable[1] &= 0x7f;
    uint32_t hash = id == 0x1c34 && body_len >= 106 ? link_crc32(stable + 10, 96)
                                                    : link_crc32(stable, body_len);
    for (size_t i = 0; i < RX_HISTORY; i++)
        if (usb.rx_hashes[i] == hash) return;
    usb.rx_hashes[usb.rx_index] = hash;
    usb.rx_index = (usb.rx_index + 1) % RX_HISTORY;
    emit(LINK_RX, 0, body, body_len);
}

static bool set_channel(uint8_t channel) {
    if (!picto_platform_channel(channel)) return false;
    discovery_clear(&discovery);
    atomic_store_explicit(&discovery_channel, channel, memory_order_relaxed);
    return true;
}

/* Runs one host command with the client locked. Returns the response length
 * (status byte included) on success, or 0 for invalid/busy. */
static size_t command(const link_packet_t *p, uint8_t response[8]) {
    const uint8_t *data = p->payload;
    int64_t now = picto_platform_now();
    switch (p->kind) {
    case LINK_HELLO:
        usb.attached = true;
        memcpy(response + 1, client.station, 6);
        return 7;
    case LINK_PING:
        return usb.attached;
    case LINK_LEAVE:
        client_leave(&client, now);
        return 1;
    case LINK_DISCOVER: {
        /* Payload: channel. Response: member counts for rooms A-D. */
        uint8_t channel = data[0];
        if (p->len != 1 || channel < 1 || channel > 14 || client_active(&client)) return 0;
        if (atomic_load_explicit(&discovery_channel, memory_order_relaxed) != channel &&
            !set_channel(channel))
            return 0;
        discovery_snapshot(&discovery, (uint32_t)(now / 1000000), response + 1);
        return 5;
    }
    case LINK_JOIN: {
        /* Payload: room, channel, nickname[20], message[52], color. */
        uint8_t room = data[0], channel = data[1];
        if (p->len != LINK_JOIN_LEN || channel < 1 || channel > 14) return 0;
        if (!client_configure(&client, room, data + 2, data + 22, data[74])) return 0;
        if (!set_channel(channel)) return 0;
        memset(usb.rx_hashes, 0, sizeof(usb.rx_hashes));
        usb.rx_index = 0;
        client_start(&client, now);
        return 1;
    }
    case LINK_BEGIN: /* Payload: transfer ID, body count. */
        return p->len == 5 && client_begin_upload(&client, get32(data), data[4]);
    case LINK_BODIES: /* Payload: transfer ID, first index, packed records. */
        return p->len > 5 &&
               client_upload_bodies(&client, get32(data), data[4], data + 5, p->len - 5);
    case LINK_COMMIT: /* Payload: transfer ID. */
        return p->len == 4 && client_commit_upload(&client, get32(data));
    default:
        return 0;
    }
}

void bridge_usb_byte(uint8_t byte) {
    static link_packet_t packet; /* Too large for the control task's stack habits. */
    if (!link_decode(&usb.decoder, byte, &packet)) return;
    usb.last_seen = picto_platform_now();

    /* The host retries until it sees a response. Replay the previous success
     * instead of running the command twice. */
    uint32_t hash = link_crc32(packet.payload, packet.len) ^ packet.kind;
    if (usb.response_len > 0 && packet.id == usb.last_id && hash == usb.last_hash) {
        emit(LINK_RESPONSE, packet.id, usb.response, usb.response_len);
        return;
    }
    uint8_t response[8] = {1}; /* Status byte: 0 success, 1 invalid/busy. */
    size_t n = 0;
    if (lock_client()) {
        n = command(&packet, response);
        unlock_client();
    }
    if (n > 0) {
        response[0] = 0;
        usb.last_id = packet.id;
        usb.last_hash = hash;
        memcpy(usb.response, response, sizeof(response));
        usb.response_len = n;
    }
    emit(LINK_RESPONSE, packet.id, response, n > 0 ? n : 1);

    unsigned drops = atomic_exchange_explicit(&usb_drops, 0, memory_order_relaxed);
    if (drops > 0) {
        uint8_t count[4];
        put32(count, drops);
        emit(LINK_GAP, 0, count, sizeof(count));
    }
}
