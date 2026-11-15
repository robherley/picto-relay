/* Makes the stock ESP32 Wi-Fi driver answer DS polls in time. Four hooks:
 *
 *  1. ieee80211_raw_frame_sanity_check: also permit our authentication,
 *     association and deauthentication frames, which the public raw TX API rejects.
 *  2. ppTxPkt (wrapped): mark poll replies as not expecting an 802.11 ACK.
 *  3. hal_mac_tx_config_edca (wrapped): zero AIFS/backoff for poll replies.
 *  4. picto_radio_flush_reply: drain the transmit queues from the receive
 *     callback instead of waiting for the Wi-Fi task's event loop.
 *
 * No SDK instructions are patched and the installed libraries are never
 * modified; prepare_radio.py builds private copies with weakened symbols.
 * Buffer layouts below are private ESP32 ebuf/DMA/TX-metadata offsets. */
#include <stdbool.h>
#include <stdint.h>
#include <string.h>
#include "esp_attr.h"
#include "esp_idf_version.h"
#include "esp_wifi.h"
#include "diag.h"
#include "radio.h"

#if ESP_IDF_VERSION != ESP_IDF_VERSION_VAL(5, 5, 2) || !CONFIG_IDF_TARGET_ESP32
#error "The radio hooks require ESP32 and ESP-IDF v5.5.2"
#endif

int picto_stock_sanity(int ifx, const void *buffer, int len, bool sys_seq);
int __real_ppTxPkt(void *buffer, int argument);
int __real_hal_mac_tx_config_edca(void *queue);
int ieee80211_output_process(void);
int ppProcessTxQ(unsigned queue);
bool picto_radio_hold_tx(void *buffer, int argument);

/* Returns the 802.11 frame inside an SDK TX buffer if it is one of our poll
 * replies (data frame to Nintendo's reply group address), else NULL. */
static uint8_t *IRAM_ATTR poll_reply(void *buffer, uint8_t **metadata) {
    static const uint8_t reply[6] = {3, 9, 0xbf, 0, 0, 0x10};
    uint8_t *eb = buffer, *dma, *frame;
    memcpy(&dma, eb + 4, sizeof(dma));
    memcpy(metadata, eb + 44, sizeof(*metadata));
    if (!dma || !*metadata) return NULL;
    memcpy(&frame, dma + 4, sizeof(frame));
    if (!frame) return NULL;
    uint32_t flags;
    memcpy(&flags, *metadata, 4);
    if (flags & (1u << 18)) frame += 4;
    unsigned length = eb[20] + ((unsigned)eb[21] << 8) + eb[22] + ((unsigned)eb[23] << 8);
    if (length < 24 || frame[0] != 0x18 || frame[1] != 0x11 || memcmp(frame + 16, reply, 6))
        return NULL;
    return frame;
}

/* ---- Inline queue draining --------------------------------------------- */

/* Set while draining so the ppTxPkt wrapper can report which access-category
 * queue the reply landed on. */
static bool draining_reply, draining_prepared;
static unsigned reply_queue;

/* esp_wifi_80211_tx appends to the driver's cache queue and posts an event;
 * ieee80211_output_process is the consumer the Wi-Fi task would run for that
 * event. ppTxPkt then posts a second event for the access-category queue,
 * which ppProcessTxQ consumes. Running both here saves two trips through the
 * event loop. Stock allocation, frame processing and completion ownership are
 * retained; queue 4 (special) is never touched. */
void IRAM_ATTR picto_radio_flush_reply(void) {
    reply_queue = UINT32_MAX;
    draining_reply = true;
    (void)ieee80211_output_process();
    DIAG_ADD(DIAG_FLUSHES, 1);
    DIAG_SET(DIAG_SELECTED_QUEUE, reply_queue);
    if (reply_queue < 4) (void)ppProcessTxQ(reply_queue);
    draining_reply = false;
}

int __wrap_ppTxPkt(void *buffer, int argument);

int IRAM_ATTR picto_radio_submit_prepared(void *buffer, int argument) {
    uint32_t start = picto_platform_since_rx();
    draining_prepared = true;
    draining_reply = true;
    reply_queue = UINT32_MAX;
    int result = __wrap_ppTxPkt(buffer, argument);
    uint32_t queued = picto_platform_since_rx();
    if (result == 0 && reply_queue < 4) (void)ppProcessTxQ(reply_queue);
    DIAG_SET(DIAG_PREPARED_ENTRY, start);
    DIAG_SET(DIAG_PREPARED_ENQUEUE, queued - start);
    DIAG_SET(DIAG_PREPARED_DRAIN, picto_platform_since_rx() - queued);
    (void)start;
    (void)queued;
    draining_prepared = false;
    draining_reply = false;
    return result;
}

/* ---- Wrapped SDK routines ---------------------------------------------- */

/* The stock raw-output routine sets TX metadata bits 0x402 for multicast,
 * meaning "do not wait for an ACK". Poll replies are unicast on the wire but
 * the DS acknowledges them in its next frame, not with an 802.11 ACK. */
int IRAM_ATTR __wrap_ppTxPkt(void *buffer, int argument) {
    if (picto_radio_hold_tx(buffer, argument)) return 0;
    uint8_t *metadata;
    if (poll_reply(buffer, &metadata)) {
        uint32_t flags;
        memcpy(&flags, metadata, 4);
        flags |= 0x402;
        memcpy(metadata, &flags, 4);
        if (draining_reply) reply_queue = (metadata[4] >> 4) & 0x0f;
    }
    return __real_ppTxPkt(buffer, argument);
}

/* lmacTxFrame passes its 36-byte queue instance here: offset 0 is the current
 * ebuf, 4 the hardware queue, 5 AIFS, 6..7 backoff. The stock HAL reads only
 * offsets 4..7. For poll replies, pass a local copy with zero AIFS and
 * backoff; the driver's persistent retry state is left alone. */
int IRAM_ATTR __wrap_hal_mac_tx_config_edca(void *queue) {
    uint8_t *eb, *metadata;
    memcpy(&eb, queue, sizeof(eb));
    if (!eb || !poll_reply(eb, &metadata)) return __real_hal_mac_tx_config_edca(queue);
    uint32_t local[2];
    memcpy(local, queue, sizeof(local));
    DIAG_ADD(DIAG_EDCA_REPLIES, 1);
    DIAG_SET(DIAG_EDCA_ORIGINAL, local[1]);
    DIAG_SET(DIAG_EDCA_DELAY, picto_platform_since_rx());
    DIAG_MAX(DIAG_EDCA_DELAY_MAX, picto_platform_since_rx());
    if (draining_prepared) DIAG_SET(DIAG_PREPARED_EDCA, picto_platform_since_rx());
    uint8_t *bytes = (uint8_t *)local;
    bytes[5] = 0;
    bytes[6] = 0;
    bytes[7] = 0;
    return __real_hal_mac_tx_config_edca(local);
}

/* Replaces the weakened stock validator. Permits exactly the three management
 * frames client.c sends (from our MAC to a unicast BSSID) and defers every
 * other frame to the stock check. */
int IRAM_ATTR ieee80211_raw_frame_sanity_check(int ifx, const void *buffer, int len, bool sys_seq) {
    static const uint8_t auth[] = {0, 0, 1, 0, 0, 0};
    static const uint8_t assoc[] = {0x21, 0, 1, 0, 0, 32};
    static const uint8_t rates[] = {1, 2, 0x82, 0x84};
    const uint8_t *p = buffer;
    uint8_t own[6];
    bool ours = ifx == WIFI_IF_STA && p && !sys_seq && len >= 24 &&
                esp_wifi_get_mac(WIFI_IF_STA, own) == ESP_OK &&
                !memcmp(p + 10, own, 6) && !memcmp(p + 4, p + 16, 6) && !(p[4] & 1) &&
                memcmp(p + 4, own, 6) && p[2] == 0xa2 && p[3] == 0 &&
                (p[1] == 0x10 || p[1] == 0x18) && !(p[22] & 0x0f);
    if (ours) {
        if (len == 26 && p[0] == 0xc0 && p[24] == 3 && p[25] == 0) return ESP_OK;
        if (len == 30 && p[0] == 0xb0 && !memcmp(p + 24, auth, 6)) return ESP_OK;
        if (len == 66 && p[0] == 0 && !memcmp(p + 24, assoc, 6) && !memcmp(p + 62, rates, 4)) {
            bool padded = true;
            for (int i = 36; i < 62; i++) padded &= p[i] == 0;
            if (padded) return ESP_OK;
        }
    }
    return picto_stock_sanity(ifx, buffer, len, sys_seq);
}
