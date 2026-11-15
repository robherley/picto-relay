/* PictoChat client state machine: find and join a DS-hosted room, exchange
 * profiles, and send drawings by answering the host's polls. Pure C with no
 * ESP-IDF dependencies so it also builds and runs in the host tests (../test).
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

/* Drawing bytes carried by one radio fragment. */
#define CLIENT_FRAGMENT_BYTES 120
/* A fragment body: 14-byte envelope and header, data, 2-byte frame number. */
#define CLIENT_FRAGMENT_OVERHEAD 16
/* Largest application body in a reply. */
#define CLIENT_MAX_BODY (CLIENT_FRAGMENT_BYTES + CLIENT_FRAGMENT_OVERHEAD)
/* Most fragments in one drawing. */
#define CLIENT_MAX_FRAGMENTS \
    ((LINK_MAX_DRAWING_BYTES + CLIENT_FRAGMENT_BYTES - 1) / CLIENT_FRAGMENT_BYTES)
/* Largest 802.11 frame the client builds. */
#define CLIENT_MAX_FRAME 256
/* How long a join searches for the room before giving up, in microseconds.
 * bridge.c hops channels during this time. */
#define CLIENT_DISCOVER_TIMEOUT 7000000
/* Session phases reported in STATUS. */
#define CLIENT_PHASE_HELLO 0       /* Answering the roster. */
#define CLIENT_PHASE_INTRODUCING 1 /* Sending our profile. */

typedef enum {
    CLIENT_OFF = LINK_STATE_OFF,
    CLIENT_DISCOVER = LINK_STATE_DISCOVER,
    CLIENT_AUTHENTICATING = LINK_STATE_AUTHENTICATING,
    CLIENT_ASSOCIATING = LINK_STATE_ASSOCIATING,
    CLIENT_CONNECTED = LINK_STATE_CONNECTED,
} client_state_t;

/* Things the host must hear about, raised here and reported by bridge.c. */
enum {
    CLIENT_EVENT_READY = 1 << 0,     /* Profile exchange finished. */
    CLIENT_EVENT_DELIVERED = 1 << 1, /* See client_t.delivered_id. */
    CLIENT_EVENT_LEFT = 1 << 2,      /* See client_t.left_reason and left_code. */
};

typedef struct {
    uint32_t polls, replies, ack_ok, ack_missed;
} client_counters_t;

/* What a reply carried, so that the host's acknowledgement advances the
 * right thing and nothing else. */
typedef enum {
    CLIENT_REPLY_IDLE,
    CLIENT_REPLY_HELLO,    /* Our answer to the roster when joining. */
    CLIENT_REPLY_TRANSFER, /* One stage of the current transfer. */
} client_reply_t;

/* A reply awaiting the host's acknowledgement of one poll round. */
typedef struct {
    bool present;
    uint16_t sequence;
    int64_t sent_at, ack_by; /* The host's verdict must arrive in this interval. */
    client_reply_t carried;
    uint8_t stage;       /* For a transfer: which stage... */
    uint32_t generation; /* ...of which transfer. */
} client_pending_t;

/* The single frame owned by the radio driver until its completion callback. */
typedef struct {
    bool present;
    uint16_t sequence;
    int64_t submitted_at;
    client_pending_t reply;
    int8_t early_ack; /* -1 none, 0 rejected, 1 accepted */
} client_flight_t;

/* One native drawing, as uploaded by the host. */
typedef struct {
    uint8_t bytes[LINK_MAX_DRAWING_BYTES];
    uint16_t length, received;
    uint32_t id;
} client_drawing_t;

typedef enum {
    CLIENT_TRANSFER_NONE,
    CLIENT_TRANSFER_PROFILE, /* Our 84-byte profile record. */
    CLIENT_TRANSFER_DRAWING,
} client_transfer_kind_t;

/* Everything a DS client sends other than idle replies is a transfer: an
 * announcement, then the data in fragments, then a completion. The host
 * echoes the announcement to open the transfer and rebroadcasts each
 * fragment to the other members. */
typedef struct {
    client_transfer_kind_t kind;
    uint32_t generation;    /* Distinguishes successive transfers. */
    uint8_t stage;          /* 0 announcement, then fragments, then completion. */
    uint8_t fragment_count;
    uint16_t length;        /* Bytes of data. */
    uint16_t first_frame;   /* Frame number of the first fragment. */
    uint16_t counter;       /* Announcement's transfer counter; the completion's is one more. */
    uint32_t command, nonce; /* Announcement fields; see client.c. */
    uint8_t marker;         /* Announcement byte after the slot, as captured. */
    uint8_t profile_subtype; /* For a profile: 0 introduces us, 1 answers a newcomer. */
    bool opened;            /* The host has echoed our announcement. */
    int64_t open_by;        /* Re-announce if it has not by then. */
    uint32_t echoed[(CLIENT_MAX_FRAGMENTS + 31) / 32];
} client_transfer_t;

/* A member whose introduction we recently answered, so that the host's
 * repeats of it are not answered again. */
typedef struct {
    uint8_t mac[6];
    int64_t at;
} client_introduction_t;

typedef struct {
    uint8_t station[6], host[6], session[6];
    client_state_t state;
    int64_t deadline;
    uint8_t attempts;
    uint16_t sequence; /* 802.11 sequence number of our next frame. */
    uint8_t slot, phase, channel;
    bool member_seen, leave_pending;
    uint8_t room, nickname[LINK_NICKNAME_LEN], profile_message[LINK_PROFILE_MESSAGE_LEN];
    uint8_t profile_color;
    client_flight_t in_flight;
    client_pending_t pending;
    /* Shape of the latest poll that included us, to predict the next reply. */
    uint16_t poll_window;  /* Per-client reply window, microseconds. */
    uint8_t poll_later;    /* Polled clients whose windows follow ours. */

    /* Joining: answer the roster, pause, then introduce ourselves. */
    bool hello_done, introduce_pending;
    int64_t settled_at;        /* Earliest time for the first transfer after the hello. */
    uint8_t answers_pending;   /* Newcomers' introductions still to answer with our profile. */
    uint16_t answers_sent;
    client_introduction_t answered[4];

    client_transfer_t transfer;
    /* Per-session sequences shared by all transfers. */
    uint16_t next_counter;     /* Transfer counter for the next announcement. */
    uint16_t next_frame;       /* Frame number for the next fragment. */
    uint16_t drawings_started;

    /* Two drawing buffers: one being sent, one being uploaded or waiting. */
    client_drawing_t drawings[2];
    uint8_t sending_slot; /* Index of the buffer being sent; the other is staged. */
    bool staged;          /* A complete drawing waits in the other buffer. */

    uint8_t events; /* CLIENT_EVENT_* not yet reported. */
    uint32_t delivered_id;
    uint8_t left_reason; /* LINK_LEFT_* */
    uint16_t left_code;
    client_counters_t counters;
} client_t;

/* Transmits a frame `delay_us` microseconds after the frame being received
 * arrived (0: as soon as possible). Returns whether the radio driver accepted it. */
typedef bool (*client_tx_fn)(void *context, const uint8_t *frame, size_t len, uint32_t delay_us);

void client_init(client_t *c, const uint8_t station[6]);
bool client_active(const client_t *c);

/* Session control. Both return a LINK_STATUS_* code. client_join starts
 * searching for the room; the caller tunes the radio and reports each channel
 * with client_set_channel. */
uint8_t client_join(client_t *c, uint8_t room, const uint8_t nickname[LINK_NICKNAME_LEN],
                    const uint8_t message[LINK_PROFILE_MESSAGE_LEN], uint8_t color, int64_t now);
void client_set_channel(client_t *c, uint8_t channel);
void client_leave(client_t *c, int64_t now);

/* Accepts the next chunk of a drawing. A chunk at position 0 starts a new
 * upload; the chunk that completes `total` bytes queues the drawing for
 * sending. Returns a LINK_STATUS_* code. */
uint8_t client_upload(client_t *c, uint32_t id, uint16_t position, uint16_t total,
                      const uint8_t *data, size_t len);

/* Progress of the drawing being sent, in fragments. */
typedef struct {
    uint32_t id; /* Zero when nothing is being sent. */
    uint8_t sent, total, echoed;
} client_progress_t;
client_progress_t client_progress(const client_t *c);

/* Radio events. Frames are FCS-stripped 802.11; times are microseconds. */
void client_receive(client_t *c, const uint8_t *frame, size_t len, int64_t now,
                    client_tx_fn tx, void *context);
void client_tick(client_t *c, int64_t now, client_tx_fn tx, void *context);
void client_tx_complete(client_t *c, uint16_t sequence, bool success);

/* Previews the next drawing reply without advancing state; 0 if none is due. */
size_t client_prepare_reply(const client_t *c, uint8_t out[CLIENT_MAX_FRAME]);

/* Exposed for the golden tests: one stage of the current transfer, and the
 * answer to a roster that lists `clients` clients. Both return the body length. */
size_t client_transfer_body(const client_t *c, uint8_t stage, uint8_t out[CLIENT_MAX_BODY]);
size_t client_hello_body(uint8_t clients, uint8_t out[CLIENT_MAX_BODY]);

/* Passive beacon observation while idle: member counts for rooms A-D on the
 * channel the radio is tuned to. */
typedef struct {
    atomic_uchar counts[LINK_ROOM_COUNT];
} discovery_t;

void discovery_clear(discovery_t *d);
/* Only counts beacons that advertise `channel`; neighbors can leak through. */
void discovery_observe(discovery_t *d, const uint8_t *frame, size_t len, uint8_t channel);
/* Each count is 1-16, or LINK_NO_BEACON when no beacon was seen since the clear. */
void discovery_snapshot(const discovery_t *d, uint8_t out[LINK_ROOM_COUNT]);
