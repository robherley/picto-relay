/* One drawing reply staged ahead of its poll, so the reply to that poll skips
 * the SDK's allocation and frame preparation.
 *
 * The buffer is allocated and configured by the stock SDK path: we call the
 * public raw TX API, intercept the buffer at two private points before it
 * reaches a hardware queue, and hold it. Only the Wi-Fi task touches this
 * state. A staged buffer has never been queued; once submitted, ownership
 * returns to the SDK. Depends on the pinned ESP-IDF v5.5.2 private ABI. */
#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include "esp_attr.h"
#include "esp_wifi.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "bridge.h"
#include "diag.h"
#include "radio.h"

int picto_stock_post_hmac_tx(void *buffer);
int ieee80211_output_raw_process(unsigned interface, void *buffer);
void ic_ebuf_recycle_tx(void *buffer);

static atomic_uintptr_t owner; /* The Wi-Fi task, learned on first use. */
static bool capturing, holding;
static void *allocated, *prepared;
static int prepared_argument;
static uint8_t expected[200];
static size_t expected_len;

static bool IRAM_ATTR on_owner(void) {
    return atomic_load_explicit(&owner, memory_order_relaxed) ==
           (uintptr_t)xTaskGetCurrentTaskHandle();
}

/* The 802.11 frame, its length and TX metadata inside an SDK buffer. */
static uint8_t *IRAM_ATTR buffer_frame(void *buffer, size_t *length, uint8_t **meta) {
    uint8_t *eb = buffer, *dma, *frame;
    memcpy(&dma, eb + 4, sizeof(dma));
    memcpy(meta, eb + 44, sizeof(*meta));
    if (!dma || !*meta) return NULL;
    memcpy(&frame, dma + 4, sizeof(frame));
    if (!frame) return NULL;
    uint32_t flags;
    memcpy(&flags, *meta, 4);
    if (flags & (1u << 18)) frame += 4;
    *length = eb[20] + ((unsigned)eb[21] << 8) + eb[22] + ((unsigned)eb[23] << 8);
    return frame;
}

static bool IRAM_ATTR matches(void *buffer) {
    size_t length;
    uint8_t *meta, *frame = buffer_frame(buffer, &length, &meta);
    return frame && length == expected_len && !memcmp(frame, expected, length);
}

/* Interception 1 (replaces the weakened SDK symbol): take the freshly
 * allocated buffer instead of letting it be posted to the Wi-Fi task. */
int IRAM_ATTR ieee80211_post_hmac_tx(void *buffer) {
    if (on_owner() && capturing) {
        if (!allocated && matches(buffer))
            allocated = buffer;
        else
            ic_ebuf_recycle_tx(buffer);
        return 0;
    }
    return picto_stock_post_hmac_tx(buffer);
}

/* Interception 2 (called from the ppTxPkt wrapper): keep the fully prepared
 * buffer instead of placing it on a hardware queue. */
bool IRAM_ATTR picto_radio_hold_tx(void *buffer, int argument) {
    if (!on_owner() || !holding) return false;
    if (!prepared && matches(buffer)) {
        prepared = buffer;
        prepared_argument = argument;
    } else {
        ic_ebuf_recycle_tx(buffer);
    }
    return true;
}

static void discard(void) {
    void *buffer = prepared;
    prepared = NULL;
    expected_len = 0;
    if (buffer) ic_ebuf_recycle_tx(buffer);
}

void picto_radio_prepare_reply(const uint8_t *frame, size_t len) {
    static const uint8_t reply[6] = {3, 9, 0xbf, 0, 0, 0x10};
    uintptr_t unset = 0;
    atomic_compare_exchange_strong_explicit(&owner, &unset,
                                            (uintptr_t)xTaskGetCurrentTaskHandle(),
                                            memory_order_relaxed, memory_order_relaxed);
    if (!on_owner() || capturing || holding) return;
    if (prepared && len == expected_len && !memcmp(frame, expected, len)) return; /* Already staged. */
    discard();
    if (len < 24 || len > sizeof(expected) || frame[0] != 0x18 || frame[1] != 0x11 ||
        memcmp(frame + 16, reply, 6))
        return;
    memcpy(expected, frame, len);
    expected_len = len;

    capturing = true;
    int result = esp_wifi_80211_tx(WIFI_IF_STA, frame, len, false);
    capturing = false;
    void *buffer = allocated;
    allocated = NULL;
    if (!buffer) return; /* The ordinary path owns any buffer not intercepted. */
    if (result != ESP_OK) {
        ic_ebuf_recycle_tx(buffer);
        return;
    }
    /* The public API has released its lock. Run the stock raw preparation,
     * intercepting only this exact reply before ppTxPkt takes ownership. */
    holding = true;
    (void)ieee80211_output_raw_process(WIFI_IF_STA, buffer);
    holding = false;
}

int IRAM_ATTR picto_radio_try_prepared(const uint8_t *frame, size_t len) {
    /* Everything but the sequence number (bytes 22..23) must match. */
    if (!on_owner() || !prepared || len != expected_len || len < 24 ||
        memcmp(frame, expected, 22) || memcmp(frame + 24, expected + 24, len - 24))
        return -1;
    size_t length;
    uint8_t *meta, *data = buffer_frame(prepared, &length, &meta);
    if (!data || length != len) return -1;
    /* The SDK calculated all PHY/DMA lengths from these same bytes. Patch in
     * the sequence and refresh the buffer's age before handing it over. */
    memcpy(data + 22, frame + 22, 2);
    uint32_t timestamp = hal_now();
    memcpy(meta + 24, &timestamp, 4);
    void *buffer = prepared;
    prepared = NULL;
    expected_len = 0;
    DIAG_ADD(DIAG_PREPARED_HITS, 1);
    return picto_radio_submit_prepared(buffer, prepared_argument) == 0;
}
