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
static const uint16_t PROFILE_POLLS[12] = {0x1c34, 0x1e30, 0x1d0a, 0x1d0a, 0x1e30, 0x1c34,
                                           0x1c34, 0x1d0a, 0x1e30, 0x1c34, 0x1e30, 0x1c34};

static uint8_t beacon[128];
static size_t beacon_len;

typedef struct {
    uint8_t bytes[256];
} frame_t;

static uint16_t le16(const uint8_t *p) { return (uint16_t)(p[0] | p[1] << 8); }
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
    f.bytes[26] = 2;
    f.bytes[28] = (uint8_t)id;
    f.bytes[29] = (uint8_t)(id >> 8);
    f.bytes[42] = 3;
    f.bytes[43] = subtype;
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
} radio_t;

static bool radio_tx(void *context, const uint8_t *frame, size_t len) {
    radio_t *radio = context;
    radio->count++;
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

/* A client that has authenticated and associated as slot 1 in Room A. */
static void join_with(const char *nickname, const char *message, uint8_t color) {
    uint8_t name[20] = {0}, text[52] = {0};
    utf16(nickname, name);
    utf16(message, text);
    client_init(&client, STATION);
    CHECK(client_configure(&client, 0, name, text, color));
    client_start(&client, 0);

    radio_t radio = rx(beacon, beacon_len, 1, true);
    static const uint8_t open_system[6] = {0, 0, 1, 0, 0, 0};
    CHECK_EQ(radio.len, 30);
    CHECK_BYTES(radio.frame + 24, open_system, 6);

    frame_t auth = host_frame(0xb0, STATION, 1);
    auth.bytes[26] = 2;
    radio = rx(auth.bytes, 30, 3, true);
    static const uint8_t rates[4] = {1, 2, 0x82, 0x84};
    CHECK_EQ(client_status(&client).state, CLIENT_ASSOCIATING);
    CHECK_EQ(radio.len, 66);
    CHECK_BYTES(radio.frame + 62, rates, 4);

    frame_t assoc = host_frame(0x10, STATION, 2);
    assoc.bytes[28] = 0x01;
    assoc.bytes[29] = 0xc0;
    rx(assoc.bytes, 30, 5, false);
    CHECK_EQ(client_status(&client).state, CLIENT_CONNECTED);
    CHECK_EQ(client_status(&client).slot, 1);
}

static void join(void) { join_with("Relay", "", 0); }

/* Walk the whole profile exchange, acknowledging every step. */
static void exchange_profile(void) {
    for (int phase = 0; phase < 12; phase++) {
        uint16_t seq = (uint16_t)(phase * 2);
        rx(poll(seq, PROFILE_POLLS[phase], phase >= 8).bytes, 140, 100 + phase * 10, true);
        rx(ack(seq + 1, false).bytes, 28, 101 + phase * 10, false);
    }
    CHECK_EQ(client_status(&client).phase, CLIENT_PHASE_READY);
}

/* ---- Link -------------------------------------------------------------- */

static void link_crc_matches_standard_vector(void) {
    CHECK_EQ(link_crc32((const uint8_t *)"123456789", 9), 0xcbf43926u);
}

static void link_framing_recovers_after_noise_corruption_and_truncation(void) {
    uint8_t data[256], wire[LINK_MAX_ENCODED];
    for (int i = 0; i < 256; i++) data[i] = (uint8_t)i;
    size_t n = link_encode(LINK_BODIES, 0x7e7d1234, data, sizeof(data), wire);
    CHECK(n > 0 && n <= LINK_MAX_ENCODED);
    CHECK_EQ(link_encode(LINK_BODIES, 1, data, 257, wire), 0);

    link_decoder_t decoder = {0};
    link_packet_t packet;
    for (const char *noise = "boot log\r\n"; *noise; noise++)
        CHECK(!link_decode(&decoder, (uint8_t)*noise, &packet));
    for (size_t i = 0; i < n / 2; i++) CHECK(!link_decode(&decoder, wire[i], &packet));
    int complete = 0;
    for (size_t i = 0; i < n; i++) complete += link_decode(&decoder, wire[i], &packet);
    CHECK_EQ(complete, 1);
    CHECK_EQ(packet.kind, LINK_BODIES);
    CHECK_EQ(packet.id, 0x7e7d1234);
    CHECK_EQ(packet.len, 256);
    CHECK_BYTES(packet.payload, data, 256);

    wire[10] ^= 1;
    for (size_t i = 0; i < n; i++) CHECK(!link_decode(&decoder, wire[i], &packet));
}

static void link_rejects_malformed_records(void) {
    uint8_t out[LINK_MAX_BODY];
    const uint8_t zero_run[] = {0x81, 0}, zero_count[] = {0x82, 0, 0};
    const uint8_t oversized[] = {0x82, 0, LINK_MAX_BODY + 1}, undersized[] = {0x82, 0, 15};
    const uint8_t truncated[] = {0x82, 0}, short_raw[] = {0x90, 1};
    CHECK_EQ(link_unpack_body(zero_run, sizeof(zero_run), out), 0);
    CHECK_EQ(link_unpack_body(zero_count, sizeof(zero_count), out), 0);
    CHECK_EQ(link_unpack_body(oversized, sizeof(oversized), out), 0);
    CHECK_EQ(link_unpack_body(undersized, sizeof(undersized), out), 0);
    CHECK_EQ(link_unpack_body(truncated, sizeof(truncated), out), 0);
    CHECK_EQ(link_unpack_body(short_raw, sizeof(short_raw), out), 0);

    const uint8_t bare[] = {0}, small[] = {0, 127}, large[] = {0, LINK_MAX_BODY + 1};
    const uint8_t incomplete[] = {0, 128, 1};
    CHECK_EQ(link_record_len(bare, sizeof(bare)), 0);
    CHECK_EQ(link_record_len(small, sizeof(small)), 0);
    CHECK_EQ(link_record_len(large, sizeof(large)), 0);
    CHECK_EQ(link_record_len(incomplete, sizeof(incomplete)), 0);

    /* A stream may hold another record, but one record must not absorb it. */
    uint8_t record[LINK_MAX_BODY + 3];
    memset(record, 0x55, sizeof(record));
    record[0] = 0;
    record[1] = LINK_MAX_BODY;
    CHECK_EQ(link_record_len(record, sizeof(record)), LINK_MAX_BODY + 2);
    CHECK_EQ(link_unpack_body(record, sizeof(record), out), 0);
    CHECK_EQ(link_unpack_body(record, LINK_MAX_BODY + 2, out), LINK_MAX_BODY);
}

/* The fixture is produced by the host's Rust encoder and packer
 * (crates/picto-relay-link/tests/firmware_fixture.rs). */
static void link_matches_the_fixture_shared_with_the_rust_host(void) {
    uint8_t fixture[1024], bodies[3][LINK_MAX_BODY] = {{0}};
    size_t lengths[3] = {LINK_MAX_BODY, LINK_MAX_BODY, 24};
    size_t n = load("link.bin", fixture, sizeof(fixture));
    for (int i = 0; i < LINK_MAX_BODY; i++) bodies[1][i] = (uint8_t)(i + 1);
    for (int i = 0; i < 24; i++) bodies[2][i] = i % 2 == 0 ? 0x7e : 0x7d;

    link_decoder_t decoder = {0};
    link_packet_t packets[3];
    int count = 0;
    for (size_t i = 0; i < n && count < 3; i++) count += link_decode(&decoder, fixture[i], &packets[count]);
    CHECK_EQ(count, 3);
    CHECK_EQ(packets[0].kind, LINK_HELLO);
    CHECK_EQ(packets[0].id, 1);
    CHECK_EQ(packets[0].len, 0);
    CHECK_EQ(packets[1].kind, LINK_BODIES);
    CHECK_EQ(packets[1].id, 0x7e7d1234);
    CHECK_EQ(packets[2].kind, LINK_RESPONSE);
    CHECK_EQ(packets[2].len, 7);

    /* The Rust packer's records unpack to the original bodies. */
    const uint8_t *records = packets[1].payload + 5;
    size_t remaining = packets[1].len - 5;
    for (int i = 0; i < 3; i++) {
        uint8_t body[LINK_MAX_BODY];
        size_t record = link_record_len(records, remaining);
        CHECK(record > 0);
        CHECK_EQ(link_unpack_body(records, record, body), lengths[i]);
        CHECK_BYTES(body, bodies[i], lengths[i]);
        records += record;
        remaining -= record;
    }
    CHECK_EQ(remaining, 0);

    /* The C encoder reproduces the Rust encoder's bytes exactly. */
    uint8_t wire[LINK_MAX_ENCODED * 3];
    size_t total = 0;
    for (int i = 0; i < 3; i++)
        total += link_encode(packets[i].kind, packets[i].id, packets[i].payload, packets[i].len,
                             wire + total);
    CHECK_EQ(total, n);
    CHECK_BYTES(wire, fixture, n);
}

/* ---- Discovery --------------------------------------------------------- */

static void discovery_reads_the_captured_beacon_and_rejects_truncation(void) {
    discovery_t discovery = {0};
    uint8_t counts[4];
    const uint8_t none[4] = {255, 255, 255, 255}, room_a[4] = {1, 255, 255, 255};
    for (size_t n = 0; n < beacon_len; n++) discovery_observe(&discovery, beacon, n, 10, 1);
    discovery_snapshot(&discovery, 10, counts);
    CHECK_BYTES(counts, none, 4);

    discovery_observe(&discovery, beacon, beacon_len, 10, 1);
    discovery_snapshot(&discovery, 10, counts);
    CHECK_BYTES(counts, room_a, 4);
    discovery_snapshot(&discovery, 14, counts);
    CHECK_BYTES(counts, none, 4);
}

static void discovery_ignores_beacons_heard_on_a_neighboring_channel(void) {
    /* Promiscuous RX can decode adjacent channels; joining there would fail. */
    discovery_t discovery = {0};
    uint8_t counts[4], frame[73] = {0};
    const uint8_t none[4] = {255, 255, 255, 255}, room_c[4] = {255, 255, 1, 255};
    const uint8_t vendor[6] = {221, 32, 0, 9, 0xbf, 0}, channel[3] = {3, 1, 13};
    frame[0] = 0x80;
    memcpy(frame + 36, vendor, 6);
    frame[56] = 8;
    frame[57] = 1;
    frame[66] = 2; /* Room C. */
    frame[67] = 1; /* One member. */
    memcpy(frame + 70, channel, 3);
    discovery_observe(&discovery, frame, sizeof(frame), 1, 12);
    discovery_snapshot(&discovery, 1, counts);
    CHECK_BYTES(counts, none, 4);
    discovery_observe(&discovery, frame, sizeof(frame), 1, 13);
    discovery_snapshot(&discovery, 1, counts);
    CHECK_BYTES(counts, room_c, 4);
}

static void discovery_counts_update_expire_and_wrap(void) {
    discovery_t discovery = {0};
    uint8_t counts[4], frame[84];
    memcpy(frame, beacon, beacon_len);
    frame[beacon_len - 3] = 2;
    discovery_observe(&discovery, frame, beacon_len, 0xfffffe, 1);
    discovery_snapshot(&discovery, 0, counts); /* Seconds wrapped past 24 bits. */
    CHECK_EQ(counts[0], 2);
    discovery_snapshot(&discovery, 2, counts);
    CHECK_EQ(counts[0], 255);
    frame[beacon_len - 3] = 3;
    discovery_observe(&discovery, frame, beacon_len, 3, 1);
    frame[beacon_len - 3] = 17; /* Impossible count: keep the last good one. */
    discovery_observe(&discovery, frame, beacon_len, 4, 1);
    discovery_snapshot(&discovery, 4, counts);
    CHECK_EQ(counts[0], 3);
    discovery_clear(&discovery);
    discovery_snapshot(&discovery, 4, counts);
    CHECK_EQ(counts[0], 255);
}

/* ---- Joining ----------------------------------------------------------- */

static void join_ignores_other_rooms(void) {
    uint8_t other[84];
    memcpy(other, beacon, beacon_len);
    other[beacon_len - 4] = 1; /* Room B. */
    client_init(&client, STATION);
    client_start(&client, 0);
    CHECK_EQ(rx(other, beacon_len, 1, true).count, 0);
    CHECK_EQ(client_status(&client).state, CLIENT_DISCOVER);
    for (size_t n = 0; n < beacon_len; n++) CHECK_EQ(rx(beacon, n, 1, true).count, 0);
    CHECK_EQ(rx(beacon, beacon_len, 1, true).count, 1);
}

static void authentication_retries_then_times_out_and_allows_restart(void) {
    client_init(&client, STATION);
    client_start(&client, 0);
    radio_t radio = rx(beacon, beacon_len, 1, true);
    CHECK_EQ(radio.frame[1] & 8, 0);
    for (int attempt = 1; attempt < 3; attempt++) {
        radio = tick(attempt * 500000 + 2, true);
        CHECK_EQ(radio.count, 1);
        CHECK_EQ(radio.frame[1] & 8, 8); /* Retry flag. */
    }
    CHECK_EQ(tick(1500003, true).count, 0);
    CHECK(!client_active(&client));
    client_start(&client, 2000000);
    CHECK(client_active(&client));
    client_leave(&client, 2000001);
    CHECK(!client_active(&client));
}

static void management_response_before_completion_defers_association(void) {
    client_init(&client, STATION);
    client_start(&client, 0);
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
    client_init(&client, STATION);
    client_start(&client, 0);
    rx(beacon, beacon_len, 1, true);
    frame_t cleanup = host_frame(0xc0, EVERYONE, 1);
    cleanup.bytes[24] = 3;
    rx(cleanup.bytes, 26, 2, false);
    CHECK_EQ(client_status(&client).state, CLIENT_AUTHENTICATING);
    frame_t auth = host_frame(0xb0, STATION, 1);
    auth.bytes[26] = 2;
    rx(auth.bytes, 30, 3, true);
    rx(cleanup.bytes, 26, 4, false);
    CHECK_EQ(client_status(&client).state, CLIENT_ASSOCIATING);
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
    CHECK_EQ(client_status(&client).disconnect_reason, 3);
}

static void wrong_host_and_multislot_rounds_do_not_transmit(void) {
    join();
    frame_t p = poll(0, 0x1c34, 0);
    p.bytes[10] ^= 1;
    CHECK_EQ(rx(p.bytes, 140, 10, true).count, 0);
    p.bytes[10] ^= 1;
    p.bytes[26] = 6;
    CHECK_EQ(rx(p.bytes, 140, 11, true).count, 0);
    CHECK_EQ(client_status(&client).replies, 0);
}

static void malformed_frames_do_not_transmit(void) {
    join();
    uint8_t garbage[256], out[LINK_MAX_BODY], next;
    memset(garbage, 255, sizeof(garbage));
    for (size_t n = 0; n <= sizeof(garbage); n++) {
        CHECK_EQ(rx(garbage, n, 10, true).count, 0);
        client_profile_reply(&client, 0, garbage, n, out, &next);
    }
    /* Addressed correctly but truncated at every length. */
    frame_t p = poll(0, 0x1c34, 0);
    for (size_t n = 0; n < 28; n++) CHECK_EQ(rx(p.bytes, n, 10, true).count, 0);
}

/* ---- Profile exchange -------------------------------------------------- */

static void profile_matches_hardware_verified_bytes(void) {
    /* Frozen from the implementation that was verified against a real DS. */
    uint8_t fixture[512], actual[512], out[LINK_MAX_BODY], next;
    size_t expected = load("profile.bin", fixture, sizeof(fixture)), total = 0;
    uint8_t name[20] = {0}, message[52] = {0};
    utf16("ESP32", name);
    client_init(&client, STATION);
    CHECK(client_configure(&client, 0, name, message, 0));
    client.slot = 1;
    for (int phase = 0; phase < 12; phase++) {
        frame_t p = poll(0, PROFILE_POLLS[phase], phase >= 8);
        size_t n = client_profile_reply(&client, (uint8_t)phase, p.bytes, 140, out, &next);
        actual[total++] = (uint8_t)n;
        actual[total++] = next;
        memcpy(actual + total, out, n);
        total += n;
    }
    CHECK_EQ(total, expected);
    CHECK_BYTES(actual, fixture, expected);
}

static void full_profile_commits_only_on_matching_results(void) {
    static const size_t lengths[12] = {108, 24, 0, 100, 2, 0, 24, 24, 0, 100, 2, 24};
    join();
    for (int phase = 0; phase < 12; phase++) {
        uint16_t seq = (uint16_t)(phase * 2);
        radio_t radio = rx(poll(seq, PROFILE_POLLS[phase], phase >= 8).bytes, 140, 100 + phase * 10, true);
        CHECK_EQ(radio.len, 24 + lengths[phase]);
        CHECK_EQ(client_status(&client).phase, phase);
        rx(ack(seq + 1, false).bytes, 28, 101 + phase * 10, false);
        CHECK_EQ(client_status(&client).phase, phase + 1);
    }
    /* Ready, with nothing uploaded: idle replies only. */
    radio_t radio = rx(poll(90, 0x1c34, 0).bytes, 140, 400, true);
    CHECK_EQ(radio.len, 26);
    CHECK_EQ(client_status(&client).message, 0);
}

static void profile_carries_nickname_message_and_color_in_both_passes(void) {
    uint8_t name[20] = {0}, message[52] = {0};
    utf16("Relay", name);
    utf16("abcdefghijklmnopqrstuvwxyz", message);
    join_with("Relay", "abcdefghijklmnopqrstuvwxyz", 2);
    int passes = 0;
    for (int phase = 0; phase < 12; phase++) {
        uint16_t seq = (uint16_t)(phase * 2);
        radio_t radio = rx(poll(seq, PROFILE_POLLS[phase], phase >= 8).bytes, 140, 100 + phase * 10, true);
        const uint8_t *body = radio.frame + 24;
        if (radio.len == 124 && body[14] == 3) {
            CHECK_EQ(body[15], passes);
            CHECK_BYTES(body + 22, name, 20);
            CHECK_BYTES(body + 42, message, 52);
            CHECK_EQ(le16(body + 94), 2);
            passes++;
        }
        rx(ack(seq + 1, false).bytes, 28, 101 + phase * 10, false);
    }
    CHECK_EQ(passes, 2);
    CHECK(!client_configure(&client, 0, name, message, 3)); /* Not while live. */
    client_init(&client, STATION);
    CHECK(!client_configure(&client, 4, name, message, 0));
    CHECK(!client_configure(&client, 0, name, message, 16));
}

static void failed_stale_or_unrelated_ack_does_not_advance(void) {
    join();
    rx(poll(10, 0x1c34, 0).bytes, 140, 10, true);
    rx(ack(12, false).bytes, 28, 11, false); /* Wrong round. */
    CHECK_EQ(client_status(&client).phase, 0);
    rx(ack(11, true).bytes, 28, 12, false); /* Right round, but missed. */
    CHECK_EQ(client_status(&client).phase, 0);
    rx(poll(14, 0x1c34, 0).bytes, 140, 20, true);
    rx(ack(15, false).bytes, 28, 5020, false); /* Too late. */
    CHECK_EQ(client_status(&client).phase, 0);
    rx(poll(16, 0x1c34, 0).bytes, 140, 6000, false); /* Driver rejected the reply. */
    rx(ack(17, false).bytes, 28, 6001, false);
    CHECK_EQ(client_status(&client).phase, 0);
    rx(poll(4095, 0x1c34, 0).bytes, 140, 7000, true); /* Sequence wraps to 0. */
    rx(ack(0, false).bytes, 28, 7001, false);
    CHECK_EQ(client_status(&client).phase, 1);
}

static void unresolved_profile_retries_when_host_changes_poll_payload(void) {
    join();
    for (int phase = 0; phase < 3; phase++) {
        uint16_t seq = (uint16_t)(phase * 2);
        rx(poll(seq, PROFILE_POLLS[phase], 0).bytes, 140, 10 + phase * 10, true);
        rx(ack(seq + 1, false).bytes, 28, 11 + phase * 10, false);
    }
    CHECK_EQ(client_status(&client).phase, 3);
    radio_t identity = rx(poll(10, 0x1d0a, 0).bytes, 140, 100, true);
    CHECK_EQ(identity.len, 124);
    rx(ack(11, true).bytes, 28, 101, false);
    /* A real host switched to a roster while the identity was unresolved. */
    radio_t retry = rx(poll(12, 0x1c34, 0).bytes, 140, 110, false);
    CHECK_BYTES(retry.frame + 24, identity.frame + 24, 100);
    retry = rx(poll(14, 0x1c34, 0).bytes, 140, 120, true);
    CHECK_BYTES(retry.frame + 24, identity.frame + 24, 100);
    rx(ack(15, false).bytes, 28, 121, false);
    CHECK_EQ(client_status(&client).phase, 4);
    radio_t idle = rx(poll(16, 0x1c34, 0).bytes, 140, 130, true);
    CHECK_EQ(idle.len, 26);
    CHECK_EQ(idle.frame[25], 0x80);
}

/* ---- Driver completion ordering ---------------------------------------- */

static void completion_and_host_ack_are_both_required_in_either_order(void) {
    for (int ack_first = 0; ack_first < 2; ack_first++) {
        join();
        radio_t radio = rx_raw(poll(10, 0x1c34, 0).bytes, 140, 10, true);
        uint16_t sequence = sequence_of(radio.frame);
        client_tx_complete(&client, sequence + 1, true); /* Not ours. */
        if (ack_first) rx_raw(ack(11, false).bytes, 28, 11, false);
        CHECK_EQ(client_status(&client).phase, 0);
        client_tx_complete(&client, sequence, true);
        if (!ack_first) {
            CHECK_EQ(client_status(&client).phase, 0);
            rx_raw(ack(11, false).bytes, 28, 12, false);
        }
        CHECK_EQ(client_status(&client).phase, 1);
        client_tx_complete(&client, sequence, true); /* A duplicate cannot commit twice. */
        CHECK_EQ(client_status(&client).ack_ok, 1);
    }
}

static void in_flight_blocks_new_round_and_failure_retains_reply(void) {
    join();
    radio_t first = rx_raw(poll(10, 0x1c34, 0).bytes, 140, 10, true);
    CHECK_EQ(rx_raw(poll(12, 0x1e30, 0).bytes, 140, 20, true).count, 0);
    client_tx_complete(&client, sequence_of(first.frame), true);
    rx_raw(ack(11, false).bytes, 28, 21, false);
    CHECK_EQ(client_status(&client).phase, 0); /* The old round was invalidated. */
    radio_t second = rx_raw(poll(14, 0x1e30, 0).bytes, 140, 30, true);
    CHECK_EQ(second.len, first.len);
    CHECK_BYTES(second.frame + 24, first.frame + 24, first.len - 24);
    rx_raw(ack(15, false).bytes, 28, 31, false);
    client_tx_complete(&client, sequence_of(second.frame), false);
    CHECK_EQ(client_status(&client).phase, 0); /* Failed TX cannot use an early ACK. */
}

static void missing_completion_fails_closed_and_late_completion_cannot_commit(void) {
    join();
    radio_t radio = rx_raw(poll(10, 0x1c34, 0).bytes, 140, 10, true);
    uint16_t sequence = sequence_of(radio.frame);
    rx_raw(ack(11, false).bytes, 28, 11, false);
    CHECK_EQ(tick(100010, true).count, 0);
    CHECK(!client_active(&client));
    client_start(&client, 100011);
    CHECK(!client_active(&client)); /* Not over an outstanding driver buffer. */
    client_tx_complete(&client, sequence, true);
    CHECK_EQ(client_status(&client).phase, 0);
    client_start(&client, 100012);
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
    CHECK_EQ(client_status(&client).phase, 0);
    rx_raw(ack(4092, false).bytes, 28, 3619626, false);
    CHECK_EQ(client_status(&client).phase, 1);
}

static void early_positive_ack_survives_completion_delay(void) {
    for (int before_next_poll = 0; before_next_poll < 2; before_next_poll++) {
        join();
        radio_t radio = rx_raw(poll(4093, 0x1c34, 0).bytes, 140, 10, true);
        if (before_next_poll) rx_raw(ack(4094, false).bytes, 28, 11, false);
        CHECK_EQ(rx_raw(poll(4095, 0x1c34, 0).bytes, 140, 20, true).count, 0);
        if (!before_next_poll) rx_raw(ack(0, false).bytes, 28, 21, false);
        client_tx_complete(&client, sequence_of(radio.frame), true);
        CHECK_EQ(client_status(&client).phase, 1);
        CHECK_EQ(client_status(&client).ack_ok, 1);
    }
}

/* ---- Drawing upload and delivery --------------------------------------- */

/* Bodies shaped like the server's: announcement, fragments, completion. */
static uint8_t bodies[CLIENT_MAX_BODIES][LINK_MAX_BODY];
static size_t body_lengths[CLIENT_MAX_BODIES];

static size_t make_bodies(uint8_t transfer, size_t drawing_bytes, size_t fragment_bytes) {
    static const uint8_t announce[10] = {0x0a, 0x8d, 0, 0, 0x14, 0, 1, 0, 0xff, 0xff};
    static const uint8_t done[24] = {0x0a, 0x8d, 3,    0,    0x14, 0,    1,    4,
                                     0xff, 0xff, 0xdd, 0x93, 0x32, 2,    1,    0xff,
                                     0xff, 0xff, 0xbb, 0xc8, 0x36, 2,    6,    0};
    size_t count = 0;
    memset(bodies, 0, sizeof(bodies));
    memcpy(bodies[0], announce, sizeof(announce));
    bodies[0][10] = (uint8_t)drawing_bytes;
    bodies[0][11] = (uint8_t)(drawing_bytes >> 8);
    bodies[0][18] = transfer;
    body_lengths[count++] = 24;
    for (size_t offset = 0; offset < drawing_bytes; offset += fragment_bytes) {
        size_t n = drawing_bytes - offset < fragment_bytes ? drawing_bytes - offset : fragment_bytes;
        uint8_t *body = bodies[count];
        body[0] = (uint8_t)((n + 12) / 2);
        body[1] = 0x8e;
        body[2] = 2;
        body[4] = (uint8_t)(n + 12);
        body[6] = 1;
        body[7] = offset == 0 ? 0x6c : 4;
        body[8] = (uint8_t)n;
        body[9] = offset + n == drawing_bytes;
        body[10] = (uint8_t)offset;
        body[11] = (uint8_t)(offset >> 8);
        /* Mostly blank with some ink, like a real drawing. */
        for (size_t i = 0; i < n; i++)
            body[14 + i] = (offset + i) % 7 == 0 ? (uint8_t)(transfer + offset + i) : 0;
        body[14 + n] = (uint8_t)(count + 2);
        body_lengths[count++] = n + 16;
    }
    memcpy(bodies[count], done, sizeof(done));
    body_lengths[count++] = 24;
    return count;
}

/* The host-side packer (crates/picto-relay-link pack_body). */
static size_t pack(const uint8_t *body, size_t len, uint8_t *out) {
    uint8_t packed[LINK_MAX_BODY * 2];
    size_t n = 0;
    for (size_t i = 0; i < len;) {
        if (body[i] == 0) {
            size_t start = i;
            while (i < len && body[i] == 0) i++;
            packed[n++] = 0;
            packed[n++] = (uint8_t)(i - start);
        } else {
            packed[n++] = body[i++];
        }
    }
    if (n < len && n <= 127) {
        out[0] = (uint8_t)(0x80 | n);
        memcpy(out + 1, packed, n);
        return n + 1;
    }
    if (len > 127) {
        out[0] = 0;
        out[1] = (uint8_t)len;
        memcpy(out + 2, body, len);
        return len + 2;
    }
    out[0] = (uint8_t)len;
    memcpy(out + 1, body, len);
    return len + 1;
}

static void upload(uint32_t transfer, size_t count) {
    CHECK(client_begin_upload(&client, transfer, (uint8_t)count));
    CHECK(!client_commit_upload(&client, transfer)); /* Incomplete. */
    for (size_t i = 0; i < count;) {
        uint8_t records[LINK_MAX_PAYLOAD], first = (uint8_t)i;
        size_t used = 0;
        while (i < count) {
            uint8_t record[LINK_MAX_BODY + 2];
            size_t n = pack(bodies[i], body_lengths[i], record);
            if (used + n > LINK_MAX_PAYLOAD - 5 - 2) break;
            memcpy(records + used, record, n);
            used += n;
            i++;
        }
        /* A batch with a bad trailing record must not consume the good ones. */
        records[used] = 0x81;
        records[used + 1] = 0;
        CHECK(!client_upload_bodies(&client, transfer, first, records, used + 2));
        CHECK(!client_upload_bodies(&client, transfer + 1, first, records, used));
        CHECK(!client_upload_bodies(&client, transfer, first + 1, records, used));
        CHECK(client_upload_bodies(&client, transfer, first, records, used));
        CHECK(!client_upload_bodies(&client, transfer, first, records, used)); /* No replay. */
    }
    CHECK(client_commit_upload(&client, transfer));
    CHECK(!client_begin_upload(&client, 99, 3)); /* Not over an undelivered drawing. */
}

static void host_echoes(size_t count, int64_t now) {
    /* The host rebroadcasts the announcement and fragments, never the completion. */
    for (size_t i = 0; i + 1 < count; i++) {
        frame_t echo = host_frame(0x0228, COMMAND, (uint16_t)(500 + i));
        memcpy(echo.bytes + 28, bodies[i], body_lengths[i]);
        echo.bytes[29] = (echo.bytes[29] & 0x7f) | 0x10;
        if (i == 0) {
            echo.bytes[30] = 1;
            echo.bytes[50] = 9;
        }
        CHECK_EQ(rx(echo.bytes, 28 + body_lengths[i], now + (int64_t)i, true).count, 0);
    }
}

static void member_joins(void) {
    uint8_t two[84];
    memcpy(two, beacon, beacon_len);
    two[beacon_len - 3] = 2;
    rx(two, beacon_len, 300, false);
}

static void uploads_deliver_repeated_drawings_and_leave_cleanly(void) {
    static const size_t fragment_sizes[] = {LINK_FRAGMENT_BYTES, 80, LINK_FRAGMENT_BYTES};
    join();
    exchange_profile();
    member_joins();
    int64_t now = 1000;
    for (uint32_t transfer = 1; transfer <= 3; transfer++) {
        size_t count = make_bodies((uint8_t)transfer, 10276, fragment_sizes[transfer - 1]);
        upload(transfer, count);
        for (size_t i = 0; i < count; i++, now += 10) {
            uint16_t seq = (uint16_t)(100 + i * 2);
            uint32_t before = client_status(&client).message;
            uint8_t preview[CLIENT_MAX_FRAME];
            size_t preview_len = client_prepare_reply(&client, preview);
            CHECK_EQ(preview_len, 24 + body_lengths[i]);
            CHECK_EQ(client_status(&client).message, before); /* Preparing is not sending. */

            radio_t radio = rx(poll(seq, 0x1c34, 0).bytes, 140, now, true);
            CHECK_EQ(radio.len, preview_len);
            CHECK_BYTES(radio.frame, preview, preview_len);
            CHECK_BYTES(radio.frame + 24, bodies[i], body_lengths[i]);

            /* Until the host acknowledges, the same body is staged for a retry. */
            size_t retry_len = client_prepare_reply(&client, preview);
            CHECK_EQ(retry_len, preview_len);
            CHECK_BYTES(preview + 24, bodies[i], body_lengths[i]);
            CHECK_EQ(client_status(&client).message, before);
            rx(ack(seq + 1, false).bytes, 28, now + 1, false);
            CHECK_EQ(client_status(&client).message, before + 1);
        }
        CHECK_EQ(client_prepare_reply(&client, (uint8_t[CLIENT_MAX_FRAME]){0}), 0);
        CHECK_EQ(client_status(&client).delivered, 0); /* MAC ACKs are not delivery. */
        host_echoes(count, now);
        now += 1000;
        CHECK_EQ(client_status(&client).echoed_fragments, count - 2);
        CHECK_EQ(client_status(&client).delivered, 1);
    }
    /* A healthy session has no time limit while the host keeps beaconing. */
    for (int64_t second = 1; second <= 70; second++) {
        rx(beacon, beacon_len, second * 1000000, false);
        CHECK(client_active(&client));
    }
    /* Silence from the host ends it. */
    tick(76000000, false);
    CHECK(!client_active(&client));
}

static void leaving_sends_one_deauthentication_before_reuse(void) {
    uint8_t name[20] = {0}, message[52] = {0};
    join();
    client_leave(&client, 1000);
    CHECK(!client_active(&client));
    CHECK(!client_configure(&client, 0, name, message, 0)); /* Leave frame still owed. */
    radio_t radio = tick(1001, true);
    CHECK_EQ(radio.len, 26);
    CHECK_EQ(radio.frame[0], 0xc0);
    CHECK_EQ(le16(radio.frame + 24), 3);
    CHECK_BYTES(radio.frame + 4, HOST, 6);
    CHECK_EQ(tick(1002, true).count, 0);
    CHECK(client_configure(&client, 0, name, message, 0));
}

static void missed_drawing_fragment_is_resent(void) {
    join();
    exchange_profile();
    member_joins();
    size_t count = make_bodies(1, 300, LINK_FRAGMENT_BYTES);
    upload(1, count);
    radio_t first = rx(poll(100, 0x1c34, 0).bytes, 140, 1000, true);
    rx(ack(101, true).bytes, 28, 1001, false); /* Host missed it. */
    CHECK_EQ(client_status(&client).message, 0);
    CHECK_EQ(client_status(&client).ack_missed, 1);
    radio_t again = rx(poll(102, 0x1c34, 0).bytes, 140, 1010, true);
    CHECK_BYTES(again.frame + 24, first.frame + 24, first.len - 24);
    CHECK(sequence_of(again.frame) != sequence_of(first.frame));
}

/* ---- Bridge (USB commands), with a simulated platform ------------------- */

static struct {
    int64_t now;
    uint8_t channel;
    radio_t radio;
    bool complete_inline; /* Driver completes synchronously inside the TX call. */
    uint8_t usb[8192];
    size_t usb_len;
    uint8_t prepared[CLIENT_MAX_FRAME];
    size_t prepared_len;
} platform;

void picto_platform_mac(uint8_t mac[6]) { memcpy(mac, STATION, 6); }
int64_t picto_platform_now(void) { return platform.now; }
bool picto_platform_channel(uint8_t channel) {
    platform.channel = channel;
    return true;
}
bool picto_platform_tx(const uint8_t *frame, size_t len) {
    radio_tx(&platform.radio, frame, len);
    if (platform.complete_inline) bridge_tx_complete(sequence_of(frame), true);
    return true;
}
bool picto_platform_usb(const uint8_t *bytes, size_t len) {
    if (platform.usb_len + len > sizeof(platform.usb)) return false;
    memcpy(platform.usb + platform.usb_len, bytes, len);
    platform.usb_len += len;
    return true;
}
void picto_radio_prepare_reply(const uint8_t *frame, size_t len) {
    memcpy(platform.prepared, frame, len);
    platform.prepared_len = len;
}

static uint32_t request_id;

/* Sends one command; returns the response payload length (status included). */
static size_t host_send(uint8_t kind, uint32_t id, const uint8_t *data, size_t len,
                        uint8_t response[LINK_MAX_PAYLOAD]) {
    uint8_t wire[LINK_MAX_ENCODED];
    size_t n = link_encode(kind, id, data, len, wire);
    platform.usb_len = 0;
    for (size_t i = 0; i < n; i++) bridge_usb_byte(wire[i]);
    link_decoder_t decoder = {0};
    link_packet_t packet;
    for (size_t i = 0; i < platform.usb_len; i++)
        if (link_decode(&decoder, platform.usb[i], &packet) && packet.kind == LINK_RESPONSE) {
            CHECK_EQ(packet.id, id);
            memcpy(response, packet.payload, packet.len);
            return packet.len;
        }
    return 0;
}

static size_t host_request(uint8_t kind, const uint8_t *data, size_t len,
                           uint8_t response[LINK_MAX_PAYLOAD]) {
    return host_send(kind, ++request_id, data, len, response);
}

/* Finds the last packet of `kind` the firmware wrote since the buffer was cleared. */
static bool host_event(uint8_t kind, link_packet_t *out) {
    link_decoder_t decoder = {0};
    link_packet_t packet;
    bool found = false;
    for (size_t i = 0; i < platform.usb_len; i++)
        if (link_decode(&decoder, platform.usb[i], &packet) && packet.kind == kind) {
            *out = packet;
            found = true;
        }
    return found;
}

static void bridge_runs_a_session_from_usb_commands(void) {
    uint8_t response[LINK_MAX_PAYLOAD], join_payload[LINK_JOIN_LEN] = {0};
    link_packet_t event;
    bridge_init();
    platform.complete_inline = true;

    /* Nothing is accepted from, or reported to, a host that has not said hello. */
    CHECK_EQ(host_request(LINK_PING, NULL, 0, response), 1);
    CHECK_EQ(response[0], 1);
    CHECK_EQ(host_request(LINK_HELLO, NULL, 0, response), 7);
    CHECK_EQ(response[0], 0);
    CHECK_BYTES(response + 1, STATION, 6);
    CHECK_EQ(host_request(LINK_PING, NULL, 0, response), 1);
    CHECK_EQ(response[0], 0);

    /* Discovery tunes the radio and reports beacons heard on that channel. */
    const uint8_t channel = 1, room_a[4] = {1, 255, 255, 255};
    CHECK_EQ(host_request(LINK_DISCOVER, &channel, 1, response), 5);
    CHECK_EQ(platform.channel, 1);
    platform.now = 1000000;
    bridge_receive(beacon, beacon_len);
    CHECK_EQ(host_request(LINK_DISCOVER, &channel, 1, response), 5);
    CHECK_BYTES(response + 1, room_a, 4);

    /* Malformed joins are refused. */
    join_payload[1] = 1;
    utf16("Relay", join_payload + 2);
    CHECK_EQ(host_request(LINK_JOIN, join_payload, 22, response), 1);
    CHECK_EQ(response[0], 1);
    join_payload[1] = 15;
    CHECK_EQ(host_request(LINK_JOIN, join_payload, sizeof(join_payload), response), 1);
    CHECK_EQ(response[0], 1);
    join_payload[1] = 1;
    CHECK_EQ(host_request(LINK_JOIN, join_payload, sizeof(join_payload), response), 1);
    CHECK_EQ(response[0], 0);

    /* A retried request (same ID and payload) replays the response, not the command. */
    CHECK_EQ(host_send(LINK_JOIN, request_id, join_payload, sizeof(join_payload), response), 1);
    CHECK_EQ(response[0], 0);
    /* A second, different join while active is refused. */
    CHECK_EQ(host_request(LINK_JOIN, join_payload, sizeof(join_payload), response), 1);
    CHECK_EQ(response[0], 1);
    CHECK_EQ(host_request(LINK_DISCOVER, &channel, 1, response), 1);
    CHECK_EQ(response[0], 1);

    /* Radio: authenticate and associate. The driver completes each frame
     * synchronously from inside the TX call, exercising the mailbox. */
    platform.radio = (radio_t){0};
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

    /* Status: connected in slot 1, with both addresses. */
    platform.usb_len = 0;
    platform.now += 300000;
    bridge_tick();
    CHECK(host_event(LINK_STATUS, &event));
    CHECK_EQ(event.len, LINK_STATUS_LEN);
    CHECK_EQ(event.payload[0], CLIENT_CONNECTED);
    CHECK_EQ(event.payload[4], 1);
    CHECK_BYTES(event.payload + 8, STATION, 6);
    CHECK_BYTES(event.payload + 14, HOST, 6);

    /* Host polls are forwarded once each; idle polls and repeats are not. */
    frame_t roster = poll(0, 0x1c34, 0);
    roster.bytes[28 + 20] = 7;
    platform.usb_len = 0;
    bridge_forward(roster.bytes, 140);
    CHECK(host_event(LINK_RX, &event));
    CHECK_EQ(event.len, 112);
    platform.usb_len = 0;
    roster.bytes[29] |= 0x80; /* Only the flag bit differs. */
    bridge_forward(roster.bytes, 140);
    bridge_forward(poll(1, 0x0000, 0).bytes, 140);
    CHECK(!host_event(LINK_RX, &event));

    /* Uploads are refused until the profile exchange has finished. */
    const uint8_t begin[5] = {1, 0, 0, 0, 3};
    CHECK_EQ(host_request(LINK_BEGIN, begin, sizeof(begin), response), 1);
    CHECK_EQ(response[0], 1);

    /* A host that stops talking makes the firmware leave the room. */
    platform.radio = (radio_t){0};
    platform.now += 6000000;
    bridge_receive(beacon, beacon_len); /* The DS is still there. */
    bridge_tick();
    bridge_tick();
    CHECK_EQ(platform.radio.len, 26);
    CHECK_EQ(platform.radio.frame[0], 0xc0);
    platform.usb_len = 0;
    platform.now += 300000;
    bridge_tick();
    CHECK(!host_event(LINK_STATUS, &event)); /* Nobody attached to report to. */
}

int main(void) {
    beacon_len = load("room-a-beacon.bin", beacon, sizeof(beacon));

    RUN(link_crc_matches_standard_vector);
    RUN(link_framing_recovers_after_noise_corruption_and_truncation);
    RUN(link_rejects_malformed_records);
    RUN(link_matches_the_fixture_shared_with_the_rust_host);

    RUN(discovery_reads_the_captured_beacon_and_rejects_truncation);
    RUN(discovery_ignores_beacons_heard_on_a_neighboring_channel);
    RUN(discovery_counts_update_expire_and_wrap);

    RUN(join_ignores_other_rooms);
    RUN(authentication_retries_then_times_out_and_allows_restart);
    RUN(management_response_before_completion_defers_association);
    RUN(broadcast_cleanup_before_membership_does_not_abort_join);
    RUN(host_broadcast_leave_ends_session);
    RUN(wrong_host_and_multislot_rounds_do_not_transmit);
    RUN(malformed_frames_do_not_transmit);

    RUN(profile_matches_hardware_verified_bytes);
    RUN(full_profile_commits_only_on_matching_results);
    RUN(profile_carries_nickname_message_and_color_in_both_passes);
    RUN(failed_stale_or_unrelated_ack_does_not_advance);
    RUN(unresolved_profile_retries_when_host_changes_poll_payload);

    RUN(completion_and_host_ack_are_both_required_in_either_order);
    RUN(in_flight_blocks_new_round_and_failure_retains_reply);
    RUN(missing_completion_fails_closed_and_late_completion_cannot_commit);
    RUN(deferred_tx_accepts_the_later_round_result);
    RUN(early_positive_ack_survives_completion_delay);

    RUN(uploads_deliver_repeated_drawings_and_leave_cleanly);
    RUN(leaving_sends_one_deauthentication_before_reuse);
    RUN(missed_drawing_fragment_is_resent);

    RUN(bridge_runs_a_session_from_usb_commands);

    if (failures) {
        printf("%d check(s) failed\n", failures);
        return 1;
    }
    printf("firmware tests passed\n");
    return 0;
}
