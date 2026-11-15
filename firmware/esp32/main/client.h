/* PictoChat client state machine: join a DS-hosted room, exchange profiles, and
 * answer host polls with uploaded drawing bodies. Pure C with no ESP-IDF
 * dependencies so it also builds and runs in the host tests (../test).
 *
 * One owner at a time: bridge.c serializes radio RX, TX completion and USB
 * commands. Nothing here allocates, blocks or logs. */
#pragma once
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include "link.h"

/* Timing-critical routines run from instruction RAM on the ESP32. */
#ifdef ESP_PLATFORM
#include "esp_attr.h"
#define PICTO_HOT IRAM_ATTR
#else
#define PICTO_HOT
#endif
/* Small helpers on the reply path must not be emitted out of line into flash. */
#define PICTO_INLINE static inline __attribute__((always_inline))

#define CLIENT_MAX_BODIES 132
#define CLIENT_PHASE_READY 12
#define CLIENT_MAX_FRAME 256

typedef enum {
    CLIENT_OFF,
    CLIENT_DISCOVER,
    CLIENT_AUTHENTICATING,
    CLIENT_ASSOCIATING,
    CLIENT_CONNECTED,
} client_state_t;

typedef struct {
    uint32_t state, phase, message, slot;
    uint32_t polls, replies, ack_ok, ack_missed;
    uint32_t disconnect_reason, echoed_fragments, delivered;
} client_status_t;

/* A reply awaiting the host's acknowledgement of one poll round. */
typedef struct {
    bool present;
    uint16_t sequence;
    int64_t sent_at;
    uint8_t phase, message;
} client_pending_t;

/* The single frame owned by the radio driver until its completion callback. */
typedef struct {
    bool present;
    uint16_t sequence;
    int64_t submitted_at;
    client_pending_t reply;
    int8_t early_ack; /* -1 none, 0 rejected, 1 accepted */
} client_flight_t;

typedef struct {
    uint8_t station[6], host[6], session[6];
    client_state_t state;
    int64_t deadline;
    uint8_t attempts;
    uint16_t sequence;
    uint8_t slot, phase, message;
    bool member_seen, announcement_seen, leave_pending;
    uint32_t echoed[5];
    uint8_t room, nickname[20], profile_message[52], profile_color;
    client_flight_t in_flight;
    client_pending_t pending;
    struct {
        bool present;
        uint8_t bytes[LINK_MAX_BODY];
        size_t length;
        uint8_t next_phase;
    } retained_profile;
    struct {
        uint8_t bodies[CLIENT_MAX_BODIES][LINK_MAX_BODY];
        uint8_t lengths[CLIENT_MAX_BODIES];
        uint8_t count, received;
        uint32_t id;
        bool ready;
    } upload;
    client_status_t stats;
} client_t;

/* Returns whether the radio driver accepted the frame for transmission. */
typedef bool (*client_tx_fn)(void *context, const uint8_t *frame, size_t len);

void client_init(client_t *c, const uint8_t station[6]);
bool client_active(const client_t *c);
client_status_t client_status(const client_t *c);

/* Session control. configure fails while a session or leave is in progress. */
bool client_configure(client_t *c, uint8_t room, const uint8_t nickname[20],
                      const uint8_t message[52], uint8_t color);
void client_start(client_t *c, int64_t now);
void client_leave(client_t *c, int64_t now);

/* Drawing upload: begin, consecutive packed bodies, commit. */
bool client_begin_upload(client_t *c, uint32_t id, uint8_t count);
bool client_upload_bodies(client_t *c, uint32_t id, uint8_t first, const uint8_t *records, size_t len);
bool client_commit_upload(client_t *c, uint32_t id);

/* Radio events. Frames are FCS-stripped 802.11; times are microseconds. */
void client_receive(client_t *c, const uint8_t *frame, size_t len, int64_t now,
                    client_tx_fn tx, void *context);
void client_tick(client_t *c, int64_t now, client_tx_fn tx, void *context);
void client_tx_complete(client_t *c, uint16_t sequence, bool success);

/* Previews the next drawing reply without advancing state; 0 if none is due. */
size_t client_prepare_reply(const client_t *c, uint8_t out[CLIENT_MAX_FRAME]);

/* Profile-exchange reply body for a host poll; sets *next to the phase to
 * commit once the host acknowledges it. Exposed for the golden test. */
size_t client_profile_reply(const client_t *c, uint8_t phase, const uint8_t *poll, size_t len,
                            uint8_t out[LINK_MAX_BODY], uint8_t *next);

/* Passive beacon observation while idle: member counts for rooms A-D. */
typedef struct {
    atomic_uint rooms[4];
} discovery_t;

void discovery_clear(discovery_t *d);
/* Only counts beacons that advertise `channel`; neighbors can leak through. */
void discovery_observe(discovery_t *d, const uint8_t *frame, size_t len, uint32_t seconds,
                       uint8_t channel);
/* Each count is 1-16, or 255 when no beacon was seen in the last 3 seconds. */
void discovery_snapshot(const discovery_t *d, uint32_t seconds, uint8_t out[4]);
