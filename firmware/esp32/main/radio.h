/* Hooks into the pinned ESP-IDF v5.5.2 Wi-Fi blobs (radio_compat.c,
 * radio_prepared.c). Everything here depends on private SDK ABI; the build
 * verifies the library hashes (prepare_radio.py) before linking. */
#pragma once
#include <stddef.h>
#include <stdint.h>

/* Microseconds since the current receive callback began. */
uint32_t picto_platform_since_rx(void);
/* SDK hardware clock, comparable with rx_ctrl.timestamp. */
uint32_t hal_now(void);

/* Drains the driver's transmit queues for a just-submitted poll reply.
 * Wi-Fi task receive callback only. */
void picto_radio_flush_reply(void);

/* Sends `frame` from the staged buffer if it matches what was prepared.
 * Returns -1 when nothing matching is staged (use the normal path), otherwise
 * whether submission succeeded. Wi-Fi task only. */
int picto_radio_try_prepared(const uint8_t *frame, size_t len);

/* Submits a staged SDK buffer and drains its queue. Used by radio_prepared.c. */
int picto_radio_submit_prepared(void *buffer, int argument);
