/* Host tests for the portable firmware code: link.c, client.c and bridge.c.
 * No ESP-IDF required. Run with `just firmware-test` from the repository root. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "../main/bridge.h"
#include "../main/client.h"
#include "../main/link.h"

/* ---- Harness ----------------------------------------------------------- */

static int failures;
static const char *current;

#define CHECK(condition)                                                         \
    do {                                                                         \
        if (!(condition)) {                                                      \
            printf("  FAIL %s:%d: %s\n", current, __LINE__, #condition);         \
            failures++;                                                          \
        }                                                                        \
    } while (0)
#define CHECK_EQ(actual, expected)                                               \
    do {                                                                         \
        long long a_ = (long long)(actual), e_ = (long long)(expected);          \
        if (a_ != e_) {                                                          \
            printf("  FAIL %s:%d: %s = %lld, expected %lld\n", current, __LINE__, \
                   #actual, a_, e_);                                             \
            failures++;                                                          \
        }                                                                        \
    } while (0)
#define CHECK_BYTES(actual, expected, n) CHECK(memcmp(actual, expected, n) == 0)
#define RUN(test)                                                                \
    do {                                                                         \
        current = #test;                                                         \
        test();                                                                  \
    } while (0)

static size_t load(const char *name, uint8_t *out, size_t capacity) {
    char path[256];
    snprintf(path, sizeof(path), "test/fixtures/%s", name);
    FILE *file = fopen(path, "rb");
    if (!file) {
        printf("cannot open %s; run from firmware/esp32\n", path);
        exit(2);
    }
    size_t n = fread(out, 1, capacity, file);
    fclose(file);
    return n;
}

/* ---- Frames ------------------------------------------------------------ */

static const uint8_t STATION[6] = {0xac, 0x15, 0x18, 0xd6, 0x94, 0x34};
static const uint8_t HOST[6] = {0, 0x23, 0x31, 0xcc, 0x71, 0xb1};
static const uint8_t COMMAND[6] = {3, 9, 0xbf, 0, 0, 0};
static const uint8_t ACK[6] = {3, 9, 0xbf, 0, 0, 3};
static const uint8_t EVERYONE[6] = {255, 255, 255, 255, 255, 255};

static uint8_t beacon[128];
static size_t beacon_len;

typedef struct {
    uint8_t bytes[256];
} frame_t;

static uint16_t le16(const uint8_t *p) { return (uint16_t)(p[0] | p[1] << 8); }

/* MAC addresses inside PictoChat packets are stored as byte-swapped words. */
static void swapped(const uint8_t mac[6], uint8_t out[6]) {
    for (int i = 0; i < 6; i += 2) {
        out[i] = mac[i + 1];
        out[i + 1] = mac[i];
    }
}
static uint16_t sequence_of(const uint8_t *frame) { return le16(frame + 22) >> 4; }

static frame_t host_frame(uint16_t fc, const uint8_t dest[6], uint16_t seq) {
    frame_t f = {{0}};
    f.bytes[0] = (uint8_t)fc;
    f.bytes[1] = (uint8_t)(fc >> 8);
    f.bytes[2] = 0xa2;
    memcpy(f.bytes + 4, dest, 6);
    memcpy(f.bytes + 10, HOST, 6);
    memcpy(f.bytes + 16, HOST, 6);
    f.bytes[22] = (uint8_t)(seq << 4);
    f.bytes[23] = (uint8_t)(seq >> 4);
    return f;
}

/* A host poll of slot 1 carrying the given application packet ID. Use 140 bytes. */
static frame_t poll(uint16_t seq, uint16_t id, uint8_t subtype) {
    frame_t f = host_frame(0x0228, COMMAND, seq);
    f.bytes[24] = (uint8_t)998; /* The usual per-client reply window, in microseconds. */
    f.bytes[25] = 998 >> 8;
    f.bytes[26] = 2;
    f.bytes[28] = (uint8_t)id;
    f.bytes[29] = (uint8_t)(id >> 8);
    f.bytes[42] = 3;
    f.bytes[43] = subtype;
    if (id == 0x1c34) {
        /* A roster: the member table lists the host and the relay. */
        swapped(HOST, f.bytes + 28 + 10);
        swapped(STATION, f.bytes + 28 + 16);
    }
    return f;
}

/* The host relaying `body` to the room, as it does with everything it accepts
 * from a member. It is addressed to nobody, so nobody replies. */
static frame_t relayed(const uint8_t *body, size_t n) {
    frame_t f = host_frame(0x0228, COMMAND, 900);
    memcpy(f.bytes + 28, body, n);
    f.bytes[29] = (f.bytes[29] & 0x7f) | 0x10; /* The host's packet IDs differ in these bits. */
    return f;
}

/* The host relaying a member's profile: `subtype` 0 when the member is
 * introducing itself, 1 when it is answering a newcomer. */
static frame_t relayed_profile(const uint8_t mac[6], uint8_t slot, uint8_t subtype) {
    uint8_t body[102] = {0x30, 0x1e, 2, 0, 0x60, 0, slot, 0, 0x54, 1};
    body[14] = 3;
    body[15] = subtype;
    swapped(mac, body + 16);
    frame_t f = host_frame(0x0228, COMMAND, 901);
    memcpy(f.bytes + 28, body, sizeof(body));
    return f;
}

/* The host's verdict on a poll round; `missed` sets slot 1's bit. Use 28 bytes. */
static frame_t ack(uint16_t seq, bool missed) {
    frame_t f = host_frame(0x0218, ACK, seq);
    f.bytes[26] = missed ? 2 : 0;
    return f;
}

/* ---- Simulated radio driver -------------------------------------------- */

typedef struct {
    bool accept;      /* What the driver returns for a submission. */
    int count;        /* Frames submitted during the call. */
    uint8_t frame[256];
    size_t len;
    uint32_t delay_us; /* Requested wait before transmitting. */
} radio_t;

static bool radio_tx(void *context, const uint8_t *frame, size_t len, uint32_t delay_us) {
    radio_t *radio = context;
    radio->count++;
    radio->delay_us = delay_us;
    radio->len = len;
    memcpy(radio->frame, frame, len);
    return radio->accept;
}

static client_t client;

/* Deliver a frame. With `complete`, the driver reports success for any
 * accepted frame right after the call returns, as the common case does. */
static radio_t rx_raw(const uint8_t *frame, size_t len, int64_t now, bool accept) {
    radio_t radio = {.accept = accept};
    client_receive(&client, frame, len, now, radio_tx, &radio);
    return radio;
}

static radio_t rx(const uint8_t *frame, size_t len, int64_t now, bool accept) {
    radio_t radio = rx_raw(frame, len, now, accept);
    if (radio.count && accept) client_tx_complete(&client, sequence_of(radio.frame), true);
    return radio;
}

static radio_t tick(int64_t now, bool accept) {
    radio_t radio = {.accept = accept};
    client_tick(&client, now, radio_tx, &radio);
    if (radio.count && accept) client_tx_complete(&client, sequence_of(radio.frame), true);
    return radio;
}

static void utf16(const char *text, uint8_t *out) {
    for (size_t i = 0; text[i]; i++) out[2 * i] = (uint8_t)text[i];
}

/* Pops the events the client raised, as bridge.c does. */
static uint8_t take_events(void) {
    uint8_t events = client.events;
    client.events = 0;
    return events;
}

/* Asks the current client to join Room A, tuned to the beacon's channel. */
static uint8_t begin_join(int64_t now) {
    static const uint8_t blank[LINK_PROFILE_MESSAGE_LEN] = {0};
    uint8_t status = client_join(&client, 0, blank, blank, 0, now);
    if (status == LINK_STATUS_OK) client_set_channel(&client, 1);
    return status;
}

/* A fresh client searching for Room A. */
static void search(void) {
    client_init(&client, STATION);
    CHECK_EQ(begin_join(0), LINK_STATUS_OK);
}

/* A client that has authenticated and associated in Room A as `slot`. */
static void join_as(uint8_t slot, const char *nickname, const char *message, uint8_t color) {
    uint8_t name[LINK_NICKNAME_LEN] = {0}, text[LINK_PROFILE_MESSAGE_LEN] = {0};
    utf16(nickname, name);
    utf16(message, text);
    client_init(&client, STATION);
    CHECK_EQ(client_join(&client, 0, name, text, color, 0), LINK_STATUS_OK);
    client_set_channel(&client, 1);

    radio_t radio = rx(beacon, beacon_len, 1, true);
    static const uint8_t open_system[6] = {0, 0, 1, 0, 0, 0};
    CHECK_EQ(radio.len, 30);
    CHECK_BYTES(radio.frame + 24, open_system, 6);

    frame_t auth = host_frame(0xb0, STATION, 1);
    auth.bytes[26] = 2;
    radio = rx(auth.bytes, 30, 3, true);
    static const uint8_t rates[4] = {1, 2, 0x82, 0x84};
    CHECK_EQ(client.state, CLIENT_ASSOCIATING);
    CHECK_EQ(radio.len, 66);
    CHECK_BYTES(radio.frame + 62, rates, 4);

    frame_t assoc = host_frame(0x10, STATION, 2);
    assoc.bytes[28] = slot;
    assoc.bytes[29] = 0xc0;
    rx(assoc.bytes, 30, 5, false);
    CHECK_EQ(client.state, CLIENT_CONNECTED);
    CHECK_EQ(client.slot, slot);
}

static void join_with(const char *nickname, const char *message, uint8_t color) {
    join_as(1, nickname, message, color);
}

static void join(void) { join_with("Relay", "", 0); }

/* A poll like poll(), but for the given slots and per-client reply window. */
static frame_t shared_poll(uint16_t seq, uint16_t mask, uint16_t client_time) {
    frame_t f = poll(seq, 0x1c34, 0);
    f.bytes[24] = (uint8_t)client_time;
    f.bytes[25] = (uint8_t)(client_time >> 8);
    f.bytes[26] = (uint8_t)mask;
    f.bytes[27] = (uint8_t)(mask >> 8);
    return f;
}

/* The host's verdict for a shared round: one bit per slot whose reply it missed. */
static frame_t shared_ack(uint16_t seq, uint16_t missed_mask) {
    frame_t f = ack(seq, false);
    f.bytes[26] = (uint8_t)missed_mask;
    f.bytes[27] = (uint8_t)(missed_mask >> 8);
    return f;
}

/* A running clock and poll sequence for tests that walk through a session. */
static int64_t clock_us;
static uint16_t poll_sequence;

/* One poll of slot 1, and the host's acknowledgement of whatever was replied. */
static radio_t round_trip(void) {
    clock_us += 10;
    poll_sequence += 2;
    radio_t radio = rx(poll(poll_sequence, 0x1c34, 0).bytes, 140, clock_us, true);
    rx(ack(poll_sequence + 1, false).bytes, 28, clock_us + 1, false);
    return radio;
}

/* Something the host broadcasts to the room without polling anyone. */
static void broadcast(frame_t frame, size_t len) {
    clock_us += 10;
    CHECK_EQ(rx(frame.bytes, len, clock_us, true).count, 0);
}

typedef struct {
    uint8_t body[CLIENT_MAX_BODY];
    size_t len;
} sent_t;

/* Lets the client run one whole transfer: its announcement, the host opening
 * the transfer by echoing it, then the data and the completion. Returns the
 * bodies the client sent. */
static int run_transfer(sent_t sent[], int capacity) {
    int count = 0;
    radio_t radio = round_trip();
    sent[count].len = radio.len - 24;
    memcpy(sent[count++].body, radio.frame + 24, radio.len - 24);
    CHECK_EQ(round_trip().len, 26); /* Nothing more until the host opens the transfer. */
    broadcast(relayed(sent[0].body, sent[0].len), 28 + sent[0].len);
    while (count < capacity) {
        radio = round_trip();
        if (radio.len == 26) break; /* Back to idle: the transfer is over. */
        sent[count].len = radio.len - 24;
        memcpy(sent[count++].body, radio.frame + 24, radio.len - 24);
    }
    return count;
}

/* Joins the room properly: answer the roster, introduce ourselves, and answer
 * the host's own introduction. Afterwards two transfers have been made. */
static void exchange_profile(void) {
    sent_t sent[4];
    clock_us = 100;
    poll_sequence = 0;
    CHECK_EQ(round_trip().len, 24 + 108); /* Roster answer. */
    CHECK_EQ(client.phase, CLIENT_PHASE_INTRODUCING);
    CHECK_EQ(round_trip().len, 26); /* A DS pauses before introducing itself. */
    clock_us += 500000;
    CHECK_EQ(run_transfer(sent, 4), 3);
    CHECK_EQ(client.phase, LINK_PHASE_READY);
    CHECK_EQ(take_events(), CLIENT_EVENT_READY);
    broadcast(relayed_profile(HOST, 0, 0), 28 + 102);
    CHECK_EQ(run_transfer(sent, 4), 3);
    CHECK_EQ(take_events(), 0);
}

/* ---- Link -------------------------------------------------------------- */

static void link_crc_matches_standard_vector(void) {
    CHECK_EQ(link_crc32((const uint8_t *)"123456789", 9), 0xcbf43926u);
}

static void link_framing_recovers_after_noise_corruption_and_truncation(void) {
    static uint8_t data[LINK_MAX_COMMAND_PAYLOAD + 1];
    static uint8_t wire[LINK_ENCODED_CAPACITY(LINK_MAX_COMMAND_PAYLOAD)];
    for (size_t i = 0; i < sizeof(data); i++) data[i] = (uint8_t)i;
    size_t n = link_encode(LINK_DRAW, 0x7e7d1234, data, LINK_MAX_COMMAND_PAYLOAD, wire, sizeof(wire));
    CHECK(n > 0 && n <= sizeof(wire));
    CHECK_EQ(link_encode(LINK_DRAW, 1, data, sizeof(data), wire, sizeof(wire)), 0); /* Too long. */
    CHECK_EQ(link_encode(LINK_DRAW, 1, data, 64, wire, 64), 0);                    /* No room. */

    static link_decoder_t decoder;
    memset(&decoder, 0, sizeof(decoder));
    link_packet_t packet;
    for (const char *noise = "boot log\r\n"; *noise; noise++)
        CHECK(!link_decode(&decoder, (uint8_t)*noise, &packet));
    n = link_encode(LINK_DRAW, 0x7e7d1234, data, LINK_MAX_COMMAND_PAYLOAD, wire, sizeof(wire));
    for (size_t i = 0; i < n / 2; i++) CHECK(!link_decode(&decoder, wire[i], &packet));
    int complete = 0;
    for (size_t i = 0; i < n; i++) complete += link_decode(&decoder, wire[i], &packet);
    CHECK_EQ(complete, 1);
    CHECK_EQ(packet.kind, LINK_DRAW);
    CHECK_EQ(packet.id, 0x7e7d1234);
    CHECK_EQ(packet.len, LINK_MAX_COMMAND_PAYLOAD);
    CHECK_BYTES(packet.payload, data, LINK_MAX_COMMAND_PAYLOAD);

    wire[10] ^= 1;
    for (size_t i = 0; i < n; i++) CHECK(!link_decode(&decoder, wire[i], &packet));
    const uint8_t runt[] = {LINK_DELIMITER, 1, 2, 3, LINK_DELIMITER, LINK_DELIMITER};
    for (size_t i = 0; i < sizeof(runt); i++) CHECK(!link_decode(&decoder, runt[i], &packet));
}

/* The fixture is produced by the host's Rust encoder
 * (crates/picto-relay-link/tests/firmware_fixture.rs). */
static void link_matches_the_fixture_shared_with_the_rust_host(void) {
    static uint8_t fixture[1024], wire[1024], every_byte[256];
    size_t n = load("link.bin", fixture, sizeof(fixture)), total = 0;
    for (int i = 0; i < 256; i++) every_byte[i] = (uint8_t)i;
    const uint8_t version = LINK_PROTOCOL_VERSION;
    const struct {
        uint8_t kind;
        uint32_t id;
        const uint8_t *payload;
        size_t len;
    } expected[3] = {{LINK_HELLO, 1, &version, 1},
                     {LINK_DRAW, 0x7e7d1234, every_byte, sizeof(every_byte)},
                     /* An empty payload still needs somewhere to point: memcmp may not be given NULL. */
                     {LINK_RESPONSE, 2, &version, 0}};

    static link_decoder_t decoder;
    memset(&decoder, 0, sizeof(decoder));
    link_packet_t packet;
    int count = 0;
    for (size_t i = 0; i < n; i++) {
        if (!link_decode(&decoder, fixture[i], &packet)) continue;
        CHECK(count < 3);
        if (count >= 3) break;
        CHECK_EQ(packet.kind, expected[count].kind);
        CHECK_EQ(packet.id, expected[count].id);
        CHECK_EQ(packet.len, expected[count].len);
        CHECK_BYTES(packet.payload, expected[count].payload, expected[count].len);
        count++;
    }
    CHECK_EQ(count, 3);

    /* The C encoder reproduces the Rust encoder's bytes exactly. */
    for (int i = 0; i < 3; i++)
        total += link_encode(expected[i].kind, expected[i].id, expected[i].payload, expected[i].len,
                             wire + total, sizeof(wire) - total);
    CHECK_EQ(total, n);
    CHECK_BYTES(wire, fixture, n);
}

/* ---- Discovery --------------------------------------------------------- */

static void discovery_reads_the_captured_beacon_and_rejects_truncation(void) {
    discovery_t discovery = {0};
    uint8_t counts[LINK_ROOM_COUNT];
    const uint8_t none[4] = {255, 255, 255, 255}, room_a[4] = {1, 255, 255, 255};
    for (size_t n = 0; n < beacon_len; n++) discovery_observe(&discovery, beacon, n, 1);
    discovery_snapshot(&discovery, counts);
    CHECK_BYTES(counts, none, 4);
    discovery_observe(&discovery, beacon, beacon_len, 1);
    discovery_snapshot(&discovery, counts);
    CHECK_BYTES(counts, room_a, 4);
    discovery_clear(&discovery);
    discovery_snapshot(&discovery, counts);
    CHECK_BYTES(counts, none, 4);
}

static void discovery_ignores_beacons_heard_on_a_neighboring_channel(void) {
    /* Promiscuous RX can decode adjacent channels; joining there would fail. */
    discovery_t discovery = {0};
    uint8_t counts[LINK_ROOM_COUNT], frame[73] = {0};
    const uint8_t none[4] = {255, 255, 255, 255}, room_c[4] = {255, 255, 1, 255};
    const uint8_t vendor[6] = {221, 32, 0, 9, 0xbf, 0}, channel[3] = {3, 1, 13};
    frame[0] = 0x80;
    memcpy(frame + 36, vendor, 6);
    frame[56] = 8;
    frame[57] = 1;
    frame[66] = 2; /* Room C. */
    frame[67] = 1; /* One member. */
    memcpy(frame + 70, channel, 3);
    discovery_observe(&discovery, frame, sizeof(frame), 12);
    discovery_snapshot(&discovery, counts);
    CHECK_BYTES(counts, none, 4);
    discovery_observe(&discovery, frame, sizeof(frame), 13);
    discovery_snapshot(&discovery, counts);
    CHECK_BYTES(counts, room_c, 4);
}

static void discovery_keeps_the_last_plausible_count(void) {
    discovery_t discovery = {0};
    uint8_t counts[LINK_ROOM_COUNT], frame[84];
    memcpy(frame, beacon, beacon_len);
    frame[beacon_len - 3] = 3;
    discovery_observe(&discovery, frame, beacon_len, 1);
    frame[beacon_len - 3] = 17; /* More members than a room holds. */
    discovery_observe(&discovery, frame, beacon_len, 1);
    discovery_snapshot(&discovery, counts);
    CHECK_EQ(counts[0], 3);
}

/* ---- Joining ----------------------------------------------------------- */

static void join_ignores_other_rooms(void) {
    uint8_t other[84];
    memcpy(other, beacon, beacon_len);
    other[beacon_len - 4] = 1; /* Room B. */
    search();
    CHECK_EQ(rx(other, beacon_len, 1, true).count, 0);
    client_set_channel(&client, 6); /* Heard through from a neighboring channel. */
    CHECK_EQ(rx(beacon, beacon_len, 1, true).count, 0);
    client_set_channel(&client, 1);
    CHECK_EQ(client.state, CLIENT_DISCOVER);
    for (size_t n = 0; n < beacon_len; n++) CHECK_EQ(rx(beacon, n, 1, true).count, 0);
    CHECK_EQ(rx(beacon, beacon_len, 1, true).count, 1);
}

static void authentication_retries_then_times_out_and_allows_restart(void) {
    search();
    radio_t radio = rx(beacon, beacon_len, 1, true);
    CHECK_EQ(radio.frame[1] & 8, 0);
    for (int attempt = 1; attempt < 3; attempt++) {
        radio = tick(attempt * 500000 + 2, true);
        CHECK_EQ(radio.count, 1);
        CHECK_EQ(radio.frame[1] & 8, 8); /* Retry flag. */
    }
    CHECK_EQ(tick(1500003, true).count, 0);
    CHECK(!client_active(&client));
    CHECK_EQ(take_events(), CLIENT_EVENT_LEFT);
    CHECK_EQ(client.left_reason, LINK_LEFT_JOIN_TIMEOUT);
    CHECK_EQ(begin_join(2000000), LINK_STATUS_OK);
    CHECK(client_active(&client));
    client_leave(&client, 2000001);
    CHECK(!client_active(&client));
}

static void management_response_before_completion_defers_association(void) {
    search();
    radio_t radio = rx_raw(beacon, beacon_len, 1, true);
    uint16_t sequence = sequence_of(radio.frame);
    frame_t auth = host_frame(0xb0, STATION, 1);
    auth.bytes[26] = 2;
    CHECK_EQ(rx_raw(auth.bytes, 30, 2, true).count, 0); /* Auth still in flight. */
    client_tx_complete(&client, sequence, true);
    radio = tick(3, true);
    CHECK_EQ(radio.len, 66);
}

static void broadcast_cleanup_before_membership_does_not_abort_join(void) {
    search();
    rx(beacon, beacon_len, 1, true);
    frame_t cleanup = host_frame(0xc0, EVERYONE, 1);
    cleanup.bytes[24] = 3;
    rx(cleanup.bytes, 26, 2, false);
    CHECK_EQ(client.state, CLIENT_AUTHENTICATING);
    frame_t auth = host_frame(0xb0, STATION, 1);
    auth.bytes[26] = 2;
    rx(auth.bytes, 30, 3, true);
    rx(cleanup.bytes, 26, 4, false);
    CHECK_EQ(client.state, CLIENT_ASSOCIATING);
    memcpy(cleanup.bytes + 4, STATION, 6); /* A directed rejection does end it. */
    rx(cleanup.bytes, 26, 5, false);
    CHECK(!client_active(&client));
}

static void host_broadcast_leave_ends_session(void) {
    join();
    frame_t leave = host_frame(0xc0, EVERYONE, 1);
    leave.bytes[24] = 3;
    rx(leave.bytes, 26, 10, false);
    CHECK(!client_active(&client));
    CHECK_EQ(take_events(), CLIENT_EVENT_LEFT);
    CHECK_EQ(client.left_reason, LINK_LEFT_KICKED);
    CHECK_EQ(client.left_code, 3);
}

static void polls_from_another_host_or_for_other_slots_do_not_transmit(void) {
    join();
    frame_t p = poll(0, 0x1c34, 0);
    p.bytes[10] ^= 1;
    CHECK_EQ(rx(p.bytes, 140, 10, true).count, 0);
    CHECK_EQ(rx(shared_poll(1, 0x000c, 998).bytes, 140, 11, true).count, 0); /* Slots 2 and 3. */
    CHECK_EQ(client.counters.replies, 0);
}

/* With several clients in the room the host polls them together. Each answers
 * in its own window, in slot order; the windows are `client_time` + 10 us. */
static void shared_polls_are_answered_in_the_relays_own_window(void) {
    /* First of the polled slots: answer immediately. */
    join();
    radio_t radio = rx(shared_poll(10, 0x0006, 998).bytes, 140, 1000, true);
    CHECK_EQ(radio.len, 24 + 108);
    CHECK_EQ(radio.delay_us, 0);
    rx(shared_ack(11, 0x0004).bytes, 28, 3000, false); /* The other client was missed, not us. */
    CHECK_EQ(client.phase, 1);
    CHECK_EQ(client.counters.ack_ok, 1);

    /* Third slot: wait out the windows of the polled slots before ours. */
    join_as(3, "Relay", "", 0);
    radio = rx(shared_poll(20, 0x000e, 998).bytes, 140, 1000, true);
    CHECK_EQ(radio.count, 1);
    CHECK_EQ(radio.delay_us, 2 * 1008);
    rx(shared_ack(21, 0x0008).bytes, 28, 4100, false); /* Our reply was missed. */
    CHECK_EQ(client.phase, 0);
    CHECK_EQ(client.counters.ack_missed, 1);
    radio = rx(shared_poll(22, 0x000a, 998).bytes, 140, 20000, true); /* Slot 2 not polled. */
    CHECK_EQ(radio.delay_us, 1008);
    rx(shared_ack(23, 0x0002).bytes, 28, 22100, false);
    CHECK_EQ(client.phase, 1);
    radio = rx(shared_poll(24, 0x0008, 998).bytes, 140, 40000, true); /* Polled alone. */
    CHECK_EQ(radio.count, 1);
    CHECK_EQ(radio.delay_us, 0);
    CHECK_EQ(rx(shared_poll(26, 0x0006, 998).bytes, 140, 60000, true).count, 0); /* Not us. */
    /* An absurd window cannot be waited out in the receive path. */
    CHECK_EQ(rx(shared_poll(28, 0x000e, 60000).bytes, 140, 80000, true).count, 0);
}

/* About once a second a DS host polls with a 214 us window, which only a bare
 * frame fits. Captured replies from a real DS are empty, with these durations. */
static void short_window_polls_get_a_bare_reply_or_none(void) {
    /* Alone in the room: a bare frame. It is late by a DS's standards, but
     * there is nobody for it to collide with. */
    join();
    radio_t radio = rx(shared_poll(10, 0x0002, 214).bytes, 30, 1000, true);
    CHECK_EQ(radio.len, 24);
    CHECK_EQ(le16(radio.frame + 2), 240);
    rx(shared_ack(11, 0).bytes, 28, 1500, false);
    CHECK_EQ(client.phase, CLIENT_PHASE_HELLO); /* Nothing was said, so nothing advances. */
    CHECK_EQ(rx(poll(12, 0x1c34, 0).bytes, 140, 2000, true).len, 24 + 108);

    /* Another client's window follows ours. We cannot finish in time, and a
     * late reply would destroy theirs, so say nothing. */
    join();
    CHECK_EQ(rx(shared_poll(10, 0x0006, 214).bytes, 30, 1000, true).count, 0);
    CHECK_EQ(rx(shared_poll(12, 0x0006, 998).bytes, 140, 2000, true).len, 24 + 108);

    /* Last of those polled: wait our turn, then the bare frame. */
    join_as(2, "Relay", "", 0);
    radio = rx(shared_poll(10, 0x0006, 214).bytes, 30, 1000, true);
    CHECK_EQ(radio.len, 24);
    CHECK_EQ(radio.delay_us, 224);
    CHECK_EQ(le16(radio.frame + 2), 240);
}

static void reply_duration_covers_the_rest_of_the_round(void) {
    /* Values captured from a real DS: alone, first of two, and second of two. */
    join();
    CHECK_EQ(le16(rx(poll(10, 0x1c34, 0).bytes, 140, 1000, true).frame + 2), 1030 - 4 * 108);
    join();
    CHECK_EQ(le16(rx(shared_poll(10, 0x0006, 998).bytes, 140, 1000, true).frame + 2), 1606);
    join_as(2, "Relay", "", 0);
    CHECK_EQ(le16(rx(shared_poll(10, 0x0006, 998).bytes, 140, 1000, true).frame + 2), 1030 - 4 * 108);
}

static void acknowledgement_is_awaited_until_every_polled_client_has_replied(void) {
    /* Fifteen clients: the verdict arrives some 15 ms after the poll. */
    join_as(15, "Relay", "", 0);
    radio_t radio = rx(shared_poll(30, 0xfffe, 998).bytes, 140, 1000, true);
    CHECK_EQ(radio.delay_us, 14 * 1008);
    rx(shared_ack(31, 0).bytes, 28, 1000 + 15 * 1008 + 200, false);
    CHECK_EQ(client.phase, 1);

    /* But not indefinitely: a verdict long after the round is someone else's. */
    radio = rx(shared_poll(32, 0xfffe, 998).bytes, 140, 100000, true);
    CHECK_EQ(radio.count, 1);
    rx(shared_ack(33, 0).bytes, 28, 100000 + 15 * 1008 + 6000, false);
    CHECK_EQ(client.phase, 1);
    CHECK_EQ(client.counters.ack_ok, 1);
}

static void malformed_frames_do_not_transmit(void) {
    join();
    uint8_t garbage[256];
    memset(garbage, 255, sizeof(garbage));
    for (size_t n = 0; n <= sizeof(garbage); n++) CHECK_EQ(rx(garbage, n, 10, true).count, 0);
    /* Addressed correctly but truncated at every length: too short to be a
     * poll, then a poll whose payload is cut off anywhere. Never read past it. */
    frame_t p = poll(0, 0x1c34, 0);
    for (size_t n = 0; n < 28; n++) CHECK_EQ(rx(p.bytes, n, 10, true).count, 0);
    for (size_t n = 28; n <= 140; n++) rx(p.bytes, n, 10 + (int64_t)n, true);
    frame_t profile = relayed_profile(HOST, 0, 0);
    frame_t control = relayed(p.bytes + 28, 24);
    control.bytes[28] = 0x0a;
    control.bytes[29] = 0x1d;
    for (size_t n = 28; n <= 28 + 102; n++) {
        CHECK_EQ(rx(profile.bytes, n, 200, true).count, 0);
        CHECK_EQ(rx(control.bytes, n, 200, true).count, 0);
    }
}

/* ---- Profile exchange -------------------------------------------------- */

static void joining_sends_what_a_real_ds_sends(void) {
    /* Byte for byte what was verified against a real DS as slot 1: the roster
     * answer, then our profile as a transfer of one fragment. */
    static const uint8_t hello[10] = {0x34, 0x8c, 6, 0, 0x68, 0, 0x11, 0x19, 0x4e, 0xbb};
    static const uint8_t introduction[24] = {0x0a, 0x8d, 0, 0,    0x14, 0,    1,    0xd0,
                                             0xff, 0xff, 0x54, 0, 0,    0,    0,    0,
                                             0,    0,    0xd8, 0x26, 0x61, 0x45, 1, 0};
    static const uint8_t answer[24] = {0x0a, 0x8d, 0, 0,    0x14, 0,    1,    0,
                                       0xff, 0xff, 0x54, 0, 0,    0,    0x0c, 0,
                                       0,    0,    0xbb, 0x93, 0x33, 0xd1, 3, 0};
    static const uint8_t completion[22] = {0x0a, 0x8d, 3, 0,    0x14, 0,    1,    4,    0xff, 0xff, 0xdd,
                                           0x93, 0x32, 2, 1,    0xff, 0xff, 0xff, 0xbb, 0xc8, 0x36, 2};
    static const uint8_t fragment_header[14] = {0x30, 0x8e, 2, 0, 0x60, 0, 1, 0x69, 0x54, 1, 0, 0, 0, 0};
    uint8_t name[LINK_NICKNAME_LEN] = {0}, message[LINK_PROFILE_MESSAGE_LEN] = {0}, mac[6];
    sent_t sent[4];
    utf16("Relay", name);
    utf16("abcdefghijklmnopqrstuvwxyz", message);
    swapped(STATION, mac);
    join_with("Relay", "abcdefghijklmnopqrstuvwxyz", 2);
    clock_us = 100;
    poll_sequence = 0;

    radio_t radio = round_trip();
    CHECK_EQ(radio.len, 24 + 108);
    CHECK_BYTES(radio.frame + 24, hello, sizeof(hello));
    CHECK_EQ(le16(radio.frame + 24 + 106), 1); /* One client in the room: us. */
    for (int i = 10; i < 106; i++) CHECK_EQ(radio.frame[24 + i], 0);
    CHECK_EQ(round_trip().len, 26); /* Answered once; then idle while settling. */
    clock_us += 499000;
    CHECK_EQ(round_trip().len, 26);
    clock_us += 1000;

    for (int pass = 0; pass < 2; pass++) {
        if (pass == 1) broadcast(relayed_profile(HOST, 0, 0), 28 + 102); /* The host's introduction. */
        CHECK_EQ(run_transfer(sent, 4), 3);
        CHECK_EQ(sent[0].len, 24);
        CHECK_BYTES(sent[0].body, pass == 0 ? introduction : answer, 24);
        CHECK_EQ(sent[1].len, 100);
        CHECK_BYTES(sent[1].body, fragment_header, sizeof(fragment_header));
        const uint8_t *record = sent[1].body + 14;
        CHECK_EQ(record[0], 3);
        CHECK_EQ(record[1], pass); /* Subtype: introducing, then answering. */
        CHECK_BYTES(record + 2, mac, 6);
        CHECK_BYTES(record + 8, name, sizeof(name));
        CHECK_BYTES(record + 28, message, sizeof(message));
        CHECK_EQ(le16(record + 80), 2);
        CHECK_EQ(le16(record + 82), 0x0203);
        CHECK_EQ(le16(sent[1].body + 98), 1 + pass); /* Fragment frame numbers run on. */
        CHECK_EQ(sent[2].len, 24);
        CHECK_BYTES(sent[2].body, completion, sizeof(completion));
        CHECK_EQ(le16(sent[2].body + 22), 2 + 2 * pass);
        CHECK_EQ(take_events(), pass == 0 ? CLIENT_EVENT_READY : 0);
    }
    CHECK_EQ(round_trip().len, 26);
}

static void roster_answer_counts_the_clients_and_waits_to_be_listed(void) {
    join();
    /* A roster that does not list us yet: we are not a member, so stay idle. */
    frame_t roster = poll(2, 0x1c34, 0);
    memset(roster.bytes + 28 + 16, 0, 6);
    CHECK_EQ(rx(roster.bytes, 140, 10, true).len, 26);
    CHECK_EQ(client.phase, CLIENT_PHASE_HELLO);
    /* Listed, alongside another DS: two clients. A real DS joining such a
     * room was captured answering 2. */
    const uint8_t other[6] = {0x00, 0x24, 0xf3, 0x67, 0x41, 0x87};
    roster = poll(4, 0x1c34, 0);
    swapped(other, roster.bytes + 28 + 22);
    radio_t radio = rx(roster.bytes, 140, 20, true);
    CHECK_EQ(radio.len, 24 + 108);
    CHECK_EQ(le16(radio.frame + 24 + 106), 2);
}

static void announcement_is_repeated_until_the_host_opens_the_transfer(void) {
    join();
    clock_us = 100;
    poll_sequence = 0;
    round_trip();
    clock_us += 500000;
    radio_t announcement = round_trip();
    CHECK_EQ(announcement.len, 48);
    /* A busy host has not echoed it. Data sent now would be dropped. */
    for (int i = 0; i < 5; i++) CHECK_EQ(round_trip().len, 26);
    clock_us += 500000;
    radio_t again = round_trip();
    CHECK_EQ(again.len, 48);
    CHECK_BYTES(again.frame + 24, announcement.frame + 24, 24);
    /* Somebody else's announcement does not open ours. */
    uint8_t other[24];
    memcpy(other, announcement.frame + 24, 24);
    other[6] = 2;
    broadcast(relayed(other, 24), 28 + 24);
    CHECK_EQ(round_trip().len, 26);
    broadcast(relayed(announcement.frame + 24, 24), 28 + 24);
    CHECK_EQ(round_trip().len, 124); /* Opened: the profile follows. */
    CHECK_EQ(round_trip().len, 48);
    CHECK_EQ(take_events(), CLIENT_EVENT_READY);
}

static void newcomers_are_answered_with_our_profile_once(void) {
    const uint8_t newcomer[6] = {0x00, 0x24, 0xf3, 0x67, 0x41, 0x87};
    sent_t sent[4];
    join();
    exchange_profile();
    /* Another DS joins and introduces itself. The host repeats the packet. */
    for (int repeat = 0; repeat < 3; repeat++) broadcast(relayed_profile(newcomer, 2, 0), 28 + 102);
    CHECK_EQ(run_transfer(sent, 4), 3);
    CHECK_EQ(sent[1].body[14 + 1], 1); /* Our profile, as an answer. */
    CHECK_EQ(round_trip().len, 26);    /* Answered once, not three times. */
    /* Every transfer in a session has fresh counters. */
    CHECK_EQ(le16(sent[0].body + 22), 5);
    CHECK_EQ(le16(sent[1].body + 98), 3);

    /* Answers from others, and the echo of our own introduction, need no answer. */
    const uint8_t third[6] = {0x00, 0x1b, 0x7a, 0x01, 0x02, 0x03};
    broadcast(relayed_profile(third, 3, 1), 28 + 102);
    broadcast(relayed_profile(STATION, 1, 0), 28 + 102);
    broadcast(relayed_profile(STATION, 3, 0), 28 + 102); /* Ours, whatever slot it claims. */
    CHECK_EQ(round_trip().len, 26);

    /* The same DS leaving and joining again later is answered again. */
    for (int second = 0; second < 4; second++) {
        clock_us += 1000000;
        rx(beacon, beacon_len, clock_us, false);
    }
    broadcast(relayed_profile(newcomer, 2, 0), 28 + 102);
    CHECK_EQ(run_transfer(sent, 4), 3);
    CHECK_EQ(take_events(), 0);
}

static void failed_stale_or_unrelated_ack_does_not_advance(void) {
    join();
    rx(poll(10, 0x1c34, 0).bytes, 140, 10, true);
    rx(ack(12, false).bytes, 28, 11, false); /* Wrong round. */
    CHECK_EQ(client.phase, 0);
    rx(ack(11, true).bytes, 28, 12, false); /* Right round, but missed. */
    CHECK_EQ(client.phase, 0);
    rx(poll(14, 0x1c34, 0).bytes, 140, 20, true);
    rx(ack(15, false).bytes, 28, 6100, false); /* Too late. */
    CHECK_EQ(client.phase, 0);
    rx(poll(16, 0x1c34, 0).bytes, 140, 7000, false); /* Driver rejected the reply. */
    rx(ack(17, false).bytes, 28, 7001, false);
    CHECK_EQ(client.phase, 0);
    rx(poll(4095, 0x1c34, 0).bytes, 140, 8000, true); /* Sequence wraps to 0. */
    rx(ack(0, false).bytes, 28, 8001, false);
    CHECK_EQ(client.phase, 1);
}

/* ---- Driver completion ordering ---------------------------------------- */

static void completion_and_host_ack_are_both_required_in_either_order(void) {
    for (int ack_first = 0; ack_first < 2; ack_first++) {
        join();
        radio_t radio = rx_raw(poll(10, 0x1c34, 0).bytes, 140, 10, true);
        uint16_t sequence = sequence_of(radio.frame);
        client_tx_complete(&client, sequence + 1, true); /* Not ours. */
        if (ack_first) rx_raw(ack(11, false).bytes, 28, 11, false);
        CHECK_EQ(client.phase, 0);
        client_tx_complete(&client, sequence, true);
        if (!ack_first) {
            CHECK_EQ(client.phase, 0);
            rx_raw(ack(11, false).bytes, 28, 12, false);
        }
        CHECK_EQ(client.phase, 1);
        client_tx_complete(&client, sequence, true); /* A duplicate cannot commit twice. */
        CHECK_EQ(client.counters.ack_ok, 1);
    }
}

static void in_flight_blocks_new_round_and_failure_retains_reply(void) {
    join();
    radio_t first = rx_raw(poll(10, 0x1c34, 0).bytes, 140, 10, true);
    CHECK_EQ(rx_raw(poll(12, 0x1c34, 0).bytes, 140, 20, true).count, 0);
    client_tx_complete(&client, sequence_of(first.frame), true);
    rx_raw(ack(11, false).bytes, 28, 21, false);
    CHECK_EQ(client.phase, 0); /* The old round was invalidated. */
    radio_t second = rx_raw(poll(14, 0x1c34, 0).bytes, 140, 30, true);
    CHECK_EQ(second.len, first.len);
    CHECK_BYTES(second.frame + 24, first.frame + 24, first.len - 24);
    rx_raw(ack(15, false).bytes, 28, 31, false);
    client_tx_complete(&client, sequence_of(second.frame), false);
    CHECK_EQ(client.phase, 0); /* Failed TX cannot use an early ACK. */
}

static void missing_completion_fails_closed_and_late_completion_cannot_commit(void) {
    join();
    radio_t radio = rx_raw(poll(10, 0x1c34, 0).bytes, 140, 10, true);
    uint16_t sequence = sequence_of(radio.frame);
    rx_raw(ack(11, false).bytes, 28, 11, false);
    CHECK_EQ(tick(100010, true).count, 0);
    CHECK(!client_active(&client));
    CHECK_EQ(client.left_reason, LINK_LEFT_RADIO_STALLED);
    CHECK_EQ(begin_join(100011), LINK_STATUS_BUSY); /* Not over an outstanding driver buffer. */
    CHECK(!client_active(&client));
    client_tx_complete(&client, sequence, true);
    CHECK_EQ(client.phase, 0);
    CHECK_EQ(begin_join(100012), LINK_STATUS_OK);
    CHECK(client_active(&client));
    radio = rx_raw(beacon, beacon_len, 100013, true);
    CHECK(sequence_of(radio.frame) != sequence);
    client_tx_complete(&client, sequence, true); /* Stale: must not release the new frame. */
    CHECK_EQ(tick(100014, true).count, 0);
}

static void deferred_tx_accepts_the_later_round_result(void) {
    /* Captured on hardware: a reply queued on poll 4088 went out in poll
     * 4091's window. A negative ACK preceded completion; the later was positive. */
    join();
    radio_t radio = rx_raw(poll(4088, 0x1c34, 0).bytes, 140, 3610456, true);
    rx_raw(ack(4089, true).bytes, 28, 3611707, false);
    CHECK_EQ(rx_raw(poll(4091, 0x1c34, 0).bytes, 140, 3618364, true).count, 0);
    client_tx_complete(&client, sequence_of(radio.frame), true);
    CHECK_EQ(client.phase, 0);
    rx_raw(ack(4092, false).bytes, 28, 3619626, false);
    CHECK_EQ(client.phase, 1);
}

static void early_positive_ack_survives_completion_delay(void) {
    for (int before_next_poll = 0; before_next_poll < 2; before_next_poll++) {
        join();
        radio_t radio = rx_raw(poll(4093, 0x1c34, 0).bytes, 140, 10, true);
        if (before_next_poll) rx_raw(ack(4094, false).bytes, 28, 11, false);
        CHECK_EQ(rx_raw(poll(4095, 0x1c34, 0).bytes, 140, 20, true).count, 0);
        if (!before_next_poll) rx_raw(ack(0, false).bytes, 28, 21, false);
        client_tx_complete(&client, sequence_of(radio.frame), true);
        CHECK_EQ(client.phase, 1);
        CHECK_EQ(client.counters.ack_ok, 1);
    }
}

/* ---- Drawings ---------------------------------------------------------- */

/* Fixtures written by the Rust server tests from the codec: a native drawing
 * and the bodies a client must transmit for it, twice in one session. */
#define STAGES (CLIENT_MAX_FRAGMENTS + 2)
static uint8_t drawing[LINK_MAX_DRAWING_BYTES];
static uint8_t reference[2 * STAGES * (CLIENT_MAX_BODY + 1)];
static const uint8_t *bodies[2][STAGES];
static size_t body_lengths[2][STAGES];

static void load_drawing_fixtures(void) {
    size_t n = load("drawing.bin", drawing, sizeof(drawing));
    size_t total = load("drawing-bodies.bin", reference, sizeof(reference)), at = 0;
    if (n != sizeof(drawing)) {
        printf("drawing.bin has the wrong size\n");
        exit(2);
    }
    for (int set = 0; set < 2; set++)
        for (int stage = 0; stage < STAGES; stage++) {
            if (at >= total) {
                printf("drawing-bodies.bin is too short\n");
                exit(2);
            }
            body_lengths[set][stage] = reference[at];
            bodies[set][stage] = reference + at + 1;
            at += 1 + reference[at];
        }
}

/* A second member must be in the room before drawings are sent. */
static void member_joins(void) {
    uint8_t two[84];
    memcpy(two, beacon, beacon_len);
    two[beacon_len - 3] = 2;
    clock_us += 10;
    rx(two, beacon_len, clock_us, false);
}

static void ready_session(void) {
    join();
    exchange_profile();
    member_joins();
}

/* Uploads the fixture drawing in chunks; returns the first non-OK status. */
static uint8_t upload(uint32_t id) {
    const size_t chunk = 1000;
    for (size_t position = 0; position < sizeof(drawing); position += chunk) {
        size_t n = sizeof(drawing) - position < chunk ? sizeof(drawing) - position : chunk;
        uint8_t status = client_upload(&client, id, (uint16_t)position, sizeof(drawing),
                                       drawing + position, n);
        if (status != LINK_STATUS_OK) return status;
    }
    return LINK_STATUS_OK;
}

/* Polls and acknowledges `count` stages starting at `first`, checking each
 * reply against reference set `set`. */
static void send_stages(int set, int first, int count, int64_t *now, uint16_t *seq) {
    for (int stage = first; stage < first + count; stage++, *now += 10, *seq += 2) {
        uint8_t preview[CLIENT_MAX_FRAME];
        size_t preview_len = client_prepare_reply(&client, preview);
        CHECK_EQ(preview_len, 24 + body_lengths[set][stage]);

        radio_t radio = rx(poll(*seq, 0x1c34, 0).bytes, 140, *now, true);
        CHECK_EQ(radio.len, 24 + body_lengths[set][stage]);
        CHECK_BYTES(radio.frame + 24, bodies[set][stage], body_lengths[set][stage]);
        CHECK_BYTES(radio.frame, preview, preview_len); /* Preparing predicted it exactly. */

        /* Until the host acknowledges, the same stage is staged for a retry. */
        CHECK_EQ(client_prepare_reply(&client, preview), preview_len);
        CHECK_BYTES(preview + 24, bodies[set][stage], body_lengths[set][stage]);
        rx(ack(*seq + 1, false).bytes, 28, *now + 1, false);
        if (stage == 0) {
            /* The host opens the transfer by echoing the announcement. */
            CHECK_EQ(client_prepare_reply(&client, preview), 0);
            frame_t echo = relayed(bodies[set][0], body_lengths[set][0]);
            CHECK_EQ(rx(echo.bytes, 28 + body_lengths[set][0], *now + 2, true).count, 0);
        }
    }
}

/* The host rebroadcasts the announcement and fragments, never the completion. */
static void host_echoes(int set, int64_t now) {
    for (int stage = 0; stage + 1 < STAGES; stage++) {
        frame_t echo = host_frame(0x0228, COMMAND, (uint16_t)(500 + stage));
        memcpy(echo.bytes + 28, bodies[set][stage], body_lengths[set][stage]);
        echo.bytes[29] = (echo.bytes[29] & 0x7f) | 0x10;
        if (stage == 0) {
            echo.bytes[30] = 1;
            echo.bytes[50] = 9;
        }
        CHECK_EQ(rx(echo.bytes, 28 + body_lengths[set][stage], now + stage, true).count, 0);
    }
}

static void drawings_are_sent_exactly_as_the_codec_reference(void) {
    uint16_t seq = 100;
    ready_session();
    int64_t now = clock_us + 1000;
    for (int set = 0; set < 2; set++) {
        uint32_t id = 40 + (uint32_t)set;
        CHECK_EQ(upload(id), LINK_STATUS_OK);
        CHECK_EQ(client_progress(&client).id, id);
        CHECK_EQ(client_progress(&client).total, CLIENT_MAX_FRAGMENTS);
        send_stages(set, 0, STAGES, &now, &seq);
        CHECK_EQ(client_prepare_reply(&client, (uint8_t[CLIENT_MAX_FRAME]){0}), 0);
        CHECK_EQ(client_progress(&client).sent, CLIENT_MAX_FRAGMENTS);
        CHECK_EQ(take_events(), 0); /* MAC acknowledgements are not delivery. */

        host_echoes(set, now);
        now += 1000;
        CHECK_EQ(take_events(), CLIENT_EVENT_DELIVERED);
        CHECK_EQ(client.delivered_id, id);
        CHECK_EQ(client_progress(&client).id, 0);
    }
    /* Nothing left to send: idle replies again. */
    CHECK_EQ(rx(poll(seq, 0x1c34, 0).bytes, 140, now, true).len, 26);
}

static void everything_we_send_names_our_slot(void) {
    /* The host relays our packets to the other members, who use this byte to
     * tell whose they are. A drawing labelled with another member's slot is
     * shown by the host but not by anybody else. */
    uint8_t body[CLIENT_MAX_BODY];
    ready_session();
    CHECK_EQ(upload(1), LINK_STATUS_OK);
    client.slot = 5;
    for (uint8_t stage = 0; stage < STAGES; stage++) {
        CHECK(client_transfer_body(&client, stage, body) > 0);
        CHECK_EQ(body[6], 5);
    }
}

static void next_drawing_uploads_while_one_is_being_sent(void) {
    uint16_t seq = 100;
    ready_session();
    int64_t now = clock_us + 1000;
    CHECK_EQ(upload(1), LINK_STATUS_OK);
    send_stages(0, 0, 10, &now, &seq);
    CHECK_EQ(upload(2), LINK_STATUS_OK);   /* Waits in the second buffer. */
    CHECK_EQ(upload(3), LINK_STATUS_BUSY); /* Both buffers hold a drawing. */
    CHECK_EQ(client_progress(&client).id, 1);
    send_stages(0, 10, STAGES - 10, &now, &seq);
    host_echoes(0, now);
    now += 1000;
    CHECK_EQ(take_events(), CLIENT_EVENT_DELIVERED);
    CHECK_EQ(client.delivered_id, 1);

    /* The waiting drawing starts by itself, as the session's second drawing. */
    CHECK_EQ(client_progress(&client).id, 2);
    CHECK_EQ(upload(3), LINK_STATUS_OK);
    send_stages(1, 0, STAGES, &now, &seq);
    host_echoes(1, now);
    CHECK_EQ(take_events(), CLIENT_EVENT_DELIVERED);
    CHECK_EQ(client.delivered_id, 2);
    CHECK_EQ(client_progress(&client).id, 3);
}

static void uploads_are_validated(void) {
    join();
    CHECK_EQ(upload(1), LINK_STATUS_WRONG_STATE); /* Profile exchange not finished. */
    exchange_profile();
    const uint16_t total = sizeof(drawing);
    CHECK_EQ(client_upload(&client, 1, 0, LINK_MIN_DRAWING_BYTES - 1, drawing, 100), LINK_STATUS_INVALID);
    CHECK_EQ(client_upload(&client, 1, 0, total, drawing, 0), LINK_STATUS_INVALID);
    CHECK_EQ(client_upload(&client, 1, total - 10, total, drawing, 11), LINK_STATUS_INVALID);
    CHECK_EQ(client_upload(&client, 1, 100, total, drawing, 100), LINK_STATUS_INVALID); /* No start. */
    CHECK_EQ(client_upload(&client, 1, 0, total, drawing, 100), LINK_STATUS_OK);
    CHECK_EQ(client_upload(&client, 1, 200, total, drawing, 100), LINK_STATUS_INVALID); /* Gap. */
    CHECK_EQ(client_upload(&client, 2, 100, total, drawing, 100), LINK_STATUS_INVALID); /* Other ID. */
    CHECK_EQ(client_upload(&client, 1, 100, total - 2, drawing, 100), LINK_STATUS_INVALID);
    CHECK_EQ(client_progress(&client).id, 0);
    /* A chunk at position 0 abandons the partial upload and starts over. */
    CHECK_EQ(upload(9), LINK_STATUS_OK);
    CHECK_EQ(client_progress(&client).id, 9);
}

static void missed_fragment_is_resent(void) {
    ready_session();
    CHECK_EQ(upload(1), LINK_STATUS_OK);
    uint32_t missed = client.counters.ack_missed;
    int64_t now = clock_us + 1000;
    radio_t first = rx(poll(100, 0x1c34, 0).bytes, 140, now, true);
    rx(ack(101, true).bytes, 28, now + 1, false); /* Host missed it. */
    CHECK_EQ(client.counters.ack_missed, missed + 1);
    radio_t again = rx(poll(102, 0x1c34, 0).bytes, 140, now + 10, true);
    CHECK_EQ(again.len, first.len);
    CHECK_BYTES(again.frame + 24, first.frame + 24, first.len - 24);
    CHECK(sequence_of(again.frame) != sequence_of(first.frame));
}

static void acknowledgement_of_an_idle_reply_does_not_advance_a_new_drawing(void) {
    ready_session();
    /* An idle reply is awaiting its acknowledgement when a drawing arrives. */
    int64_t now = clock_us + 1000;
    CHECK_EQ(rx(poll(100, 0x1c34, 0).bytes, 140, now, true).len, 26);
    CHECK_EQ(upload(1), LINK_STATUS_OK);
    rx(ack(101, false).bytes, 28, now + 1, false);
    radio_t radio = rx(poll(102, 0x1c34, 0).bytes, 140, now + 10, true);
    CHECK_BYTES(radio.frame + 24, bodies[0][0], body_lengths[0][0]); /* Still the announcement. */
}

static void altered_echoes_do_not_count_as_delivery(void) {
    uint16_t seq = 100;
    ready_session();
    int64_t now = clock_us + 1000;
    CHECK_EQ(upload(1), LINK_STATUS_OK);
    send_stages(0, 0, STAGES, &now, &seq);
    for (int stage = 0; stage + 1 < STAGES; stage++) {
        frame_t echo = host_frame(0x0228, COMMAND, (uint16_t)(500 + stage));
        memcpy(echo.bytes + 28, bodies[0][stage], body_lengths[0][stage]);
        echo.bytes[29] = (echo.bytes[29] & 0x7f) | 0x10;
        if (stage == 5) echo.bytes[28 + 20] ^= 1; /* One corrupted drawing byte. */
        rx(echo.bytes, 28 + body_lengths[0][stage], now + stage, true);
    }
    CHECK_EQ(take_events(), 0);
    CHECK_EQ(client_progress(&client).echoed, CLIENT_MAX_FRAGMENTS - 1);
}

static void session_survives_while_the_host_beacons_and_ends_when_it_stops(void) {
    join();
    for (int64_t second = 1; second <= 70; second++) {
        rx(beacon, beacon_len, second * 1000000, false);
        CHECK(client_active(&client));
    }
    tick(76000000, false);
    CHECK(!client_active(&client));
    CHECK_EQ(take_events(), CLIENT_EVENT_LEFT);
    CHECK_EQ(client.left_reason, LINK_LEFT_HOST_LOST);
}

static void search_gives_up_when_no_host_is_found(void) {
    search();
    tick(CLIENT_DISCOVER_TIMEOUT - 1, true);
    CHECK(client_active(&client));
    CHECK_EQ(tick(CLIENT_DISCOVER_TIMEOUT, true).count, 0);
    CHECK(!client_active(&client));
    CHECK_EQ(take_events(), CLIENT_EVENT_LEFT);
    CHECK_EQ(client.left_reason, LINK_LEFT_NO_HOST);
}

static void leaving_sends_one_deauthentication_before_reuse(void) {
    join();
    client_leave(&client, 1000);
    CHECK(!client_active(&client));
    CHECK_EQ(take_events(), CLIENT_EVENT_LEFT);
    CHECK_EQ(client.left_reason, LINK_LEFT_REQUESTED);
    CHECK_EQ(begin_join(1000), LINK_STATUS_BUSY); /* Leave frame still owed. */
    radio_t radio = tick(1001, true);
    CHECK_EQ(radio.len, 26);
    CHECK_EQ(radio.frame[0], 0xc0);
    CHECK_EQ(le16(radio.frame + 24), 3);
    CHECK_BYTES(radio.frame + 4, HOST, 6);
    CHECK_EQ(tick(1002, true).count, 0);
    CHECK_EQ(begin_join(1003), LINK_STATUS_OK);
    client_leave(&client, 1004); /* Never connected: nothing to tell the host. */
    CHECK_EQ(tick(1005, true).count, 0);
}

/* ---- Bridge (USB commands), with a simulated platform ------------------- */

static struct {
    int64_t now;
    uint8_t channel;
    uint32_t channels_visited; /* Bit per channel tuned. */
    radio_t radio;
    uint8_t usb[32768];
    size_t usb_len;
    size_t prepared_len;
} platform;

void picto_platform_mac(uint8_t mac[6]) { memcpy(mac, STATION, 6); }
int64_t picto_platform_now(void) { return platform.now; }
bool picto_platform_channel(uint8_t channel) {
    platform.channel = channel;
    platform.channels_visited |= 1u << channel;
    return true;
}
bool picto_platform_tx(const uint8_t *frame, size_t len, uint32_t delay_us) {
    radio_tx(&platform.radio, frame, len, delay_us);
    /* The driver completes synchronously, inside the TX call. */
    bridge_tx_complete(sequence_of(frame), true);
    return true;
}
bool picto_platform_usb(const uint8_t *bytes, size_t len) {
    if (platform.usb_len + len > sizeof(platform.usb)) return false;
    memcpy(platform.usb + platform.usb_len, bytes, len);
    platform.usb_len += len;
    return true;
}
void picto_radio_prepare_reply(const uint8_t *frame, size_t len) {
    (void)frame;
    platform.prepared_len = len;
}

typedef struct {
    uint8_t kind;
    uint32_t id;
    size_t len;
    uint8_t payload[LINK_MAX_EVENT_PAYLOAD];
} event_t;

/* Finds the last packet of `kind` the firmware wrote since the buffer was cleared. */
static bool host_event(uint8_t kind, event_t *out) {
    static link_decoder_t decoder;
    memset(&decoder, 0, sizeof(decoder));
    link_packet_t packet;
    bool found = false;
    for (size_t i = 0; i < platform.usb_len; i++)
        if (link_decode(&decoder, platform.usb[i], &packet) && packet.kind == kind) {
            out->kind = packet.kind;
            out->id = packet.id;
            out->len = packet.len;
            memcpy(out->payload, packet.payload, packet.len);
            found = true;
        }
    return found;
}

static uint32_t request_id;

/* Sends one command; returns its response (status byte first). */
static event_t host_send(uint8_t kind, uint32_t id, const uint8_t *data, size_t len) {
    static uint8_t wire[LINK_ENCODED_CAPACITY(LINK_MAX_COMMAND_PAYLOAD)];
    event_t response = {0};
    size_t n = link_encode(kind, id, data, len, wire, sizeof(wire));
    platform.usb_len = 0;
    for (size_t i = 0; i < n; i++) bridge_usb_byte(wire[i]);
    CHECK(host_event(LINK_RESPONSE, &response));
    CHECK_EQ(response.id, id);
    CHECK(response.len >= 1);
    return response;
}

static uint8_t host_request(uint8_t kind, const uint8_t *data, size_t len) {
    return host_send(kind, ++request_id, data, len).payload[0];
}

/* Advances time by one channel dwell and runs the control task. */
static void dwell(void) {
    platform.now += 250001;
    bridge_tick();
}

/* One poll of slot 1 through the bridge, acknowledged; returns what was sent. */
static radio_t bridge_round(void) {
    static uint16_t seq;
    seq += 2;
    platform.radio = (radio_t){0};
    platform.now += 10;
    bridge_receive(poll(seq, 0x1c34, 0).bytes, 140);
    platform.now += 1;
    bridge_receive(ack(seq + 1, false).bytes, 28);
    return platform.radio;
}

/* One whole transfer through the bridge: announcement, the host's echo that
 * opens it, the profile fragment, the completion. */
static void bridge_profile_transfer(void) {
    radio_t announcement = bridge_round();
    CHECK_EQ(announcement.len, 48);
    CHECK_EQ(bridge_round().len, 26);
    platform.now += 10;
    bridge_receive(relayed(announcement.frame + 24, 24).bytes, 28 + 24);
    CHECK_EQ(bridge_round().len, 124);
    CHECK_EQ(bridge_round().len, 48);
}

static void bridge_runs_a_session_from_usb_commands(void) {
    event_t event;
    uint8_t version = LINK_PROTOCOL_VERSION, join_payload[LINK_JOIN_LEN] = {0};
    memset(&platform, 0, sizeof(platform));
    bridge_init();

    /* Frame capture is off until asked for. */
    bridge_trace(beacon, beacon_len, beacon_len, 1234, false);
    CHECK_EQ(platform.usb_len, 0);
    CHECK(!bridge_tracing());

    /* Nothing is accepted before HELLO, and a mismatched host is told why. */
    CHECK_EQ(host_request(LINK_PING, NULL, 0), LINK_STATUS_NOT_ATTACHED);
    uint8_t other_version = LINK_PROTOCOL_VERSION + 1;
    event = host_send(LINK_HELLO, ++request_id, &other_version, 1);
    CHECK_EQ(event.payload[0], LINK_STATUS_VERSION_MISMATCH);
    CHECK_EQ(event.payload[1 + LINK_HELLO_VERSION_OFFSET], LINK_PROTOCOL_VERSION);
    CHECK_EQ(host_request(LINK_PING, NULL, 0), LINK_STATUS_NOT_ATTACHED);
    event = host_send(LINK_HELLO, ++request_id, &version, 1);
    CHECK_EQ(event.payload[0], LINK_STATUS_OK);
    CHECK_BYTES(event.payload + 1 + LINK_HELLO_MAC_OFFSET, STATION, 6);
    CHECK_EQ(event.len, 1 + LINK_HELLO_BUILD_OFFSET + strlen("unknown"));
    CHECK_BYTES(event.payload + 1 + LINK_HELLO_BUILD_OFFSET, "unknown", 7);
    CHECK_EQ(host_request(LINK_PING, NULL, 0), LINK_STATUS_OK);
    CHECK_EQ(host_request(99, NULL, 0), LINK_STATUS_INVALID);

    /* TRACE turns frame capture on; long frames are cut to fit one event. */
    const uint8_t on = 1, off = 0, bad = 2;
    static uint8_t long_frame[400];
    for (size_t i = 0; i < sizeof(long_frame); i++) long_frame[i] = (uint8_t)i;
    CHECK_EQ(host_request(LINK_TRACE, &bad, 1), LINK_STATUS_INVALID);
    CHECK_EQ(host_request(LINK_TRACE, &on, 1), LINK_STATUS_OK);
    CHECK(bridge_tracing());
    platform.usb_len = 0;
    bridge_trace(long_frame, 256, sizeof(long_frame), 0x01020304, true);
    CHECK(host_event(LINK_FRAME, &event));
    CHECK_EQ(event.len, LINK_FRAME_DATA_OFFSET + LINK_FRAME_MAX_BYTES);
    CHECK_EQ(link_get32(event.payload + LINK_FRAME_TIME_OFFSET), 0x01020304);
    CHECK_EQ(event.payload[LINK_FRAME_FLAGS_OFFSET], LINK_FRAME_FLAG_SENT);
    CHECK_EQ(link_get16(event.payload + LINK_FRAME_LENGTH_OFFSET), sizeof(long_frame));
    CHECK_BYTES(event.payload + LINK_FRAME_DATA_OFFSET, long_frame, LINK_FRAME_MAX_BYTES);
    CHECK_EQ(host_request(LINK_TRACE, &off, 1), LINK_STATUS_OK);
    platform.usb_len = 0;
    bridge_trace(beacon, beacon_len, beacon_len, 1, false);
    CHECK_EQ(platform.usb_len, 0);

    /* SCAN sweeps every channel once and reports the rooms it heard. */
    CHECK_EQ(host_request(LINK_SCAN, &version, 1), LINK_STATUS_INVALID);
    CHECK_EQ(host_request(LINK_SCAN, NULL, 0), LINK_STATUS_OK);
    platform.usb_len = 0;
    for (int hop = 0; hop < 13; hop++) {
        CHECK(!host_event(LINK_ROOMS, &event));
        if (platform.channel == 1) bridge_receive(beacon, beacon_len);
        dwell();
    }
    const uint8_t room_a[4] = {1, 255, 255, 255};
    CHECK_EQ(platform.channels_visited, 0x3ffe); /* Channels 1 to 13. */
    CHECK(host_event(LINK_ROOMS, &event));
    CHECK_BYTES(event.payload, room_a, 4);
    platform.usb_len = 0;
    dwell();
    CHECK(!host_event(LINK_ROOMS, &event)); /* One sweep per SCAN. */

    /* JOIN searches the channels itself, starting where the room was last heard. */
    utf16("Relay", join_payload + LINK_JOIN_NICKNAME_OFFSET);
    CHECK_EQ(host_request(LINK_JOIN, join_payload, LINK_JOIN_LEN - 1), LINK_STATUS_INVALID);
    join_payload[LINK_JOIN_ROOM_OFFSET] = 4;
    CHECK_EQ(host_request(LINK_JOIN, join_payload, sizeof(join_payload)), LINK_STATUS_INVALID);
    join_payload[LINK_JOIN_ROOM_OFFSET] = 0;
    platform.channel = 0;
    CHECK_EQ(host_request(LINK_JOIN, join_payload, sizeof(join_payload)), LINK_STATUS_OK);
    CHECK_EQ(platform.channel, 1);
    /* A retried request (same ID and payload) replays the response, not the command. */
    CHECK_EQ(host_send(LINK_JOIN, request_id, join_payload, sizeof(join_payload)).payload[0],
             LINK_STATUS_OK);
    CHECK_EQ(host_request(LINK_JOIN, join_payload, sizeof(join_payload)), LINK_STATUS_WRONG_STATE);
    CHECK_EQ(host_request(LINK_SCAN, NULL, 0), LINK_STATUS_WRONG_STATE);

    /* The room is not answering yet: the search moves on, and comes back round. */
    dwell();
    CHECK(platform.channel != 1);
    platform.radio = (radio_t){0};
    bridge_receive(beacon, beacon_len); /* Leaking in from channel 1: ignored. */
    CHECK_EQ(platform.radio.count, 0);
    for (int hop = 0; hop < 12 && platform.channel != 1; hop++) dwell();
    CHECK_EQ(platform.channel, 1);

    /* Found: authenticate and associate, and stop hopping. */
    bridge_receive(beacon, beacon_len);
    CHECK_EQ(platform.radio.len, 30);
    frame_t auth = host_frame(0xb0, STATION, 1);
    auth.bytes[26] = 2;
    bridge_receive(auth.bytes, 30);
    CHECK_EQ(platform.radio.len, 66);
    frame_t assoc = host_frame(0x10, STATION, 2);
    assoc.bytes[28] = 0x01;
    assoc.bytes[29] = 0xc0;
    bridge_receive(assoc.bytes, 30);
    dwell();
    CHECK_EQ(platform.channel, 1);

    /* Uploads are refused until the profile exchange has finished. */
    static uint8_t draw[LINK_MAX_COMMAND_PAYLOAD];
    link_put32(draw + LINK_DRAW_ID_OFFSET, 77);
    link_put16(draw + LINK_DRAW_POSITION_OFFSET, 0);
    link_put16(draw + LINK_DRAW_TOTAL_OFFSET, sizeof(drawing));
    memcpy(draw + LINK_DRAW_DATA_OFFSET, drawing, LINK_MAX_DRAW_CHUNK);
    CHECK_EQ(host_request(LINK_DRAW, draw, sizeof(draw)), LINK_STATUS_WRONG_STATE);
    CHECK_EQ(host_request(LINK_DRAW, draw, LINK_DRAW_DATA_OFFSET), LINK_STATUS_INVALID);

    /* Join: answer the roster, pause, introduce ourselves. READY is reported
     * as soon as that completes. */
    platform.usb_len = 0;
    CHECK_EQ(bridge_round().len, 24 + 108);
    platform.now += 500001;
    bridge_receive(beacon, beacon_len);
    bridge_profile_transfer();
    bridge_tick();
    CHECK(host_event(LINK_READY, &event)); /* Not held back until the next status. */
    platform.now += 1000001;
    bridge_tick();
    CHECK(host_event(LINK_READY, &event));
    CHECK_EQ(event.len, LINK_READY_LEN);
    CHECK_BYTES(event.payload + LINK_READY_HOST_OFFSET, HOST, 6);
    CHECK_EQ(event.payload[LINK_READY_SLOT_OFFSET], 1);
    CHECK_EQ(event.payload[LINK_READY_ROOM_OFFSET], 0);
    CHECK_EQ(event.payload[LINK_READY_CHANNEL_OFFSET], 1);
    CHECK(host_event(LINK_STATUS, &event));
    CHECK_EQ(event.len, LINK_STATUS_LEN);
    CHECK_EQ(event.payload[LINK_STATUS_STATE_OFFSET], LINK_STATE_CONNECTED);
    CHECK_EQ(event.payload[LINK_STATUS_PHASE_OFFSET], LINK_PHASE_READY);
    CHECK_EQ(event.id, 0);

    /* Host packets are forwarded when they change; idle polls and repeats are not. */
    frame_t roster = poll(0, 0x1c34, 0);
    roster.bytes[28 + 20] = 7;
    platform.usb_len = 0;
    bridge_forward(roster.bytes, 140);
    CHECK(host_event(LINK_RX, &event));
    CHECK_EQ(event.len, 112);
    platform.usb_len = 0;
    roster.bytes[29] |= 0x80; /* Only the flag bit differs. */
    roster.bytes[28 + 8] = 9; /* And a counter outside the member list. */
    bridge_forward(roster.bytes, 140);
    bridge_forward(poll(1, 0x0000, 0).bytes, 140);
    CHECK(!host_event(LINK_RX, &event));
    roster.bytes[28 + 20] = 8; /* A member changed. */
    bridge_forward(roster.bytes, 140);
    CHECK(host_event(LINK_RX, &event));

    /* The host introduces itself; we answer with our profile. */
    platform.now += 10;
    bridge_receive(relayed_profile(HOST, 0, 0).bytes, 28 + 102);
    bridge_profile_transfer();

    /* A drawing uploaded over USB is sent, echoed and reported delivered. */
    for (size_t position = 0; position < sizeof(drawing); position += LINK_MAX_DRAW_CHUNK) {
        size_t n = sizeof(drawing) - position < LINK_MAX_DRAW_CHUNK ? sizeof(drawing) - position
                                                                    : LINK_MAX_DRAW_CHUNK;
        link_put16(draw + LINK_DRAW_POSITION_OFFSET, (uint16_t)position);
        memcpy(draw + LINK_DRAW_DATA_OFFSET, drawing + position, n);
        CHECK_EQ(host_request(LINK_DRAW, draw, LINK_DRAW_DATA_OFFSET + n), LINK_STATUS_OK);
    }
    uint8_t two[84];
    memcpy(two, beacon, beacon_len);
    two[beacon_len - 3] = 2;
    bridge_receive(two, beacon_len);
    bridge_prepare_reply();
    CHECK_EQ(platform.prepared_len, 24 + body_lengths[0][0]);
    for (int stage = 0; stage < STAGES; stage++) {
        radio_t sent = bridge_round();
        CHECK_EQ(sent.len, 24 + body_lengths[0][stage]);
        CHECK_BYTES(sent.frame + 24, bodies[0][stage], body_lengths[0][stage]);
        if (stage == 0) { /* The host opens the transfer. */
            platform.now += 10;
            bridge_receive(relayed(bodies[0][0], body_lengths[0][0]).bytes, 28 + body_lengths[0][0]);
        }
    }
    platform.usb_len = 0;
    platform.now += 250001;
    bridge_tick();
    CHECK(host_event(LINK_STATUS, &event)); /* Progress while sending. */
    CHECK_EQ(event.id, 77);
    CHECK_EQ(event.payload[LINK_STATUS_SENT_OFFSET], CLIENT_MAX_FRAGMENTS);
    CHECK_EQ(event.payload[LINK_STATUS_TOTAL_OFFSET], CLIENT_MAX_FRAGMENTS);
    CHECK_EQ(event.payload[LINK_STATUS_ECHOED_OFFSET], 0);
    CHECK(!host_event(LINK_DELIVERED, &event));
    for (int stage = 0; stage + 1 < STAGES; stage++) {
        frame_t echo = host_frame(0x0228, COMMAND, (uint16_t)(500 + stage));
        memcpy(echo.bytes + 28, bodies[0][stage], body_lengths[0][stage]);
        echo.bytes[29] = (echo.bytes[29] & 0x7f) | 0x10;
        platform.now += 10;
        bridge_receive(echo.bytes, 28 + body_lengths[0][stage]);
    }
    bridge_tick();
    CHECK(host_event(LINK_DELIVERED, &event));
    CHECK_EQ(event.id, 77);

    /* The DS closing the session is reported with its reason. */
    frame_t closed = host_frame(0xc0, EVERYONE, 1);
    closed.bytes[24] = 3;
    platform.usb_len = 0;
    bridge_receive(closed.bytes, 26);
    bridge_tick();
    CHECK(host_event(LINK_LEFT, &event));
    CHECK_EQ(event.payload[LINK_LEFT_REASON_OFFSET], LINK_LEFT_KICKED);
    CHECK_EQ(link_get16(event.payload + LINK_LEFT_CODE_OFFSET), 3);
}

static void bridge_leaves_when_the_host_goes_silent(void) {
    event_t event;
    uint8_t version = LINK_PROTOCOL_VERSION, join_payload[LINK_JOIN_LEN] = {0};
    memset(&platform, 0, sizeof(platform));
    bridge_init();
    CHECK_EQ(host_send(LINK_HELLO, ++request_id, &version, 1).payload[0], LINK_STATUS_OK);
    CHECK_EQ(host_request(LINK_JOIN, join_payload, sizeof(join_payload)), LINK_STATUS_OK);
    bridge_receive(beacon, beacon_len);
    frame_t auth = host_frame(0xb0, STATION, 1);
    auth.bytes[26] = 2;
    bridge_receive(auth.bytes, 30);
    frame_t assoc = host_frame(0x10, STATION, 2);
    assoc.bytes[28] = 0x01;
    assoc.bytes[29] = 0xc0;
    bridge_receive(assoc.bytes, 30);

    platform.radio = (radio_t){0};
    platform.usb_len = 0;
    platform.now += 6000000;
    bridge_receive(beacon, beacon_len); /* The DS is still there; the host is not. */
    bridge_tick();
    bridge_tick();
    CHECK_EQ(platform.radio.len, 26);
    CHECK_EQ(platform.radio.frame[0], 0xc0);
    platform.now += 2000000;
    bridge_tick();
    CHECK(!host_event(LINK_STATUS, &event)); /* Nobody attached to report to. */
    CHECK(!host_event(LINK_LEFT, &event));
    CHECK_EQ(host_request(LINK_PING, NULL, 0), LINK_STATUS_NOT_ATTACHED);
}

int main(void) {
    beacon_len = load("room-a-beacon.bin", beacon, sizeof(beacon));
    load_drawing_fixtures();

    RUN(link_crc_matches_standard_vector);
    RUN(link_framing_recovers_after_noise_corruption_and_truncation);
    RUN(link_matches_the_fixture_shared_with_the_rust_host);

    RUN(discovery_reads_the_captured_beacon_and_rejects_truncation);
    RUN(discovery_ignores_beacons_heard_on_a_neighboring_channel);
    RUN(discovery_keeps_the_last_plausible_count);

    RUN(join_ignores_other_rooms);
    RUN(search_gives_up_when_no_host_is_found);
    RUN(authentication_retries_then_times_out_and_allows_restart);
    RUN(management_response_before_completion_defers_association);
    RUN(broadcast_cleanup_before_membership_does_not_abort_join);
    RUN(host_broadcast_leave_ends_session);
    RUN(polls_from_another_host_or_for_other_slots_do_not_transmit);
    RUN(shared_polls_are_answered_in_the_relays_own_window);
    RUN(short_window_polls_get_a_bare_reply_or_none);
    RUN(reply_duration_covers_the_rest_of_the_round);
    RUN(acknowledgement_is_awaited_until_every_polled_client_has_replied);
    RUN(malformed_frames_do_not_transmit);
    RUN(leaving_sends_one_deauthentication_before_reuse);
    RUN(session_survives_while_the_host_beacons_and_ends_when_it_stops);

    RUN(joining_sends_what_a_real_ds_sends);
    RUN(roster_answer_counts_the_clients_and_waits_to_be_listed);
    RUN(announcement_is_repeated_until_the_host_opens_the_transfer);
    RUN(newcomers_are_answered_with_our_profile_once);
    RUN(failed_stale_or_unrelated_ack_does_not_advance);

    RUN(completion_and_host_ack_are_both_required_in_either_order);
    RUN(in_flight_blocks_new_round_and_failure_retains_reply);
    RUN(missing_completion_fails_closed_and_late_completion_cannot_commit);
    RUN(deferred_tx_accepts_the_later_round_result);
    RUN(early_positive_ack_survives_completion_delay);

    RUN(uploads_are_validated);
    RUN(drawings_are_sent_exactly_as_the_codec_reference);
    RUN(everything_we_send_names_our_slot);
    RUN(next_drawing_uploads_while_one_is_being_sent);
    RUN(missed_fragment_is_resent);
    RUN(acknowledgement_of_an_idle_reply_does_not_advance_a_new_drawing);
    RUN(altered_echoes_do_not_count_as_delivery);

    RUN(bridge_runs_a_session_from_usb_commands);
    RUN(bridge_leaves_when_the_host_goes_silent);

    if (failures) {
        printf("%d check(s) failed\n", failures);
        return 1;
    }
    printf("firmware tests passed\n");
    return 0;
}
