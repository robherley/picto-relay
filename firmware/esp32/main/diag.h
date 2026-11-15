/* Radio timing counters for latency investigations. Compiled out unless the
 * firmware is built with -DPICTO_DIAGNOSTICS=ON, in which case they are sent
 * to the host as a LINK_DIAG packet of little-endian u32 values in this order.
 * These are latest sampled callback times in microseconds, not RF timestamps. */
#pragma once
#include <stdatomic.h>
#include <stdint.h>

enum {
    DIAG_REPLY_COUNT,      /* Replies submitted from the receive callback. */
    DIAG_REPLY_PREP_SUM,   /* Total RX-to-submission time; divide by count. */
    DIAG_REPLY_PREP_MAX,
    DIAG_API_TIME,         /* RX to return from the raw TX call. */
    DIAG_FLUSH_TIME,       /* RX to return from the inline queue drain. */
    DIAG_FLUSHES,
    DIAG_SELECTED_QUEUE,   /* Access-category queue used; UINT32_MAX if none. */
    DIAG_EDCA_REPLIES,     /* Replies given zero AIFS/backoff. */
    DIAG_EDCA_ORIGINAL,    /* The stock queue/AIFS/backoff word replaced. */
    DIAG_EDCA_DELAY,       /* RX to EDCA configuration. */
    DIAG_EDCA_DELAY_MAX,
    DIAG_PREPARED_HITS,    /* Replies sent from the prepared buffer. */
    DIAG_PREPARED_ENTRY,   /* RX to prepared submission. */
    DIAG_PREPARED_ENQUEUE, /* Duration of the SDK enqueue. */
    DIAG_PREPARED_DRAIN,   /* Duration of the queue drain. */
    DIAG_PREPARED_EDCA,    /* RX to EDCA configuration for a prepared reply. */
    DIAG_RX_AGE,           /* Hardware timestamp age at poll callback entry. */
    DIAG_RX_LENGTH,        /* That poll's MAC frame length. */
    DIAG_COUNT,
};

#if PICTO_DIAGNOSTICS
extern atomic_uint picto_diag[DIAG_COUNT];
#define DIAG_SET(i, v) atomic_store_explicit(&picto_diag[i], (v), memory_order_relaxed)
#define DIAG_ADD(i, v) atomic_fetch_add_explicit(&picto_diag[i], (v), memory_order_relaxed)
#define DIAG_MAX(i, v)                                                              \
    do {                                                                            \
        unsigned diag_value = (v);                                                  \
        if (diag_value > atomic_load_explicit(&picto_diag[i], memory_order_relaxed)) \
            DIAG_SET(i, diag_value);                                                \
    } while (0)
#else
#define DIAG_SET(i, v) ((void)0)
#define DIAG_ADD(i, v) ((void)0)
#define DIAG_MAX(i, v) ((void)0)
#endif
