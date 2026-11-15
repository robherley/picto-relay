/* ESP-IDF hardware boundary: Wi-Fi setup, radio callbacks and the UART.
 * Protocol state lives in client.c; USB commands in bridge.c. */
#include <stdatomic.h>
#include <stdint.h>
#include <string.h>
#include "driver/uart.h"
#include "esp_attr.h"
#include "esp_event.h"
#include "esp_log.h"
#include "esp_timer.h"
#include "esp_wifi.h"
#include "freertos/FreeRTOS.h"
#include "freertos/queue.h"
#include "freertos/task.h"
#include "nvs_flash.h"
#include "bridge.h"
#include "diag.h"
#include "link.h"
#include "radio.h"

/* Frames copied out of the Wi-Fi callback for the control task: host polls to
 * forward over USB and, while capture is on, every PictoChat frame. */
#define COPY_BYTES 256
typedef struct {
    uint16_t len;       /* Bytes copied. */
    uint16_t air_len;   /* Length of the frame on the air, without FCS. */
    uint32_t timestamp; /* Radio clock, microseconds. */
    bool forward, sent;
    uint8_t bytes[COPY_BYTES];
} rx_packet_t;
/* Encoded packets waiting for the blocking UART writer. */
typedef struct {
    size_t len;
    uint8_t bytes[LINK_MAX_EVENT_ENCODED];
} usb_packet_t;
static QueueHandle_t input, output;
#define QUEUE_DEPTH 64
#define UART_RX_BUFFER_BYTES (2 * LINK_ENCODED_CAPACITY(LINK_MAX_COMMAND_PAYLOAD))
#define UART_TX_BUFFER_BYTES 2048

static int64_t callback_started;
/* True while the Wi-Fi task is inside the receive callback: the only context
 * allowed to send prepared replies or drain the transmit queues inline. */
static bool in_receive;

uint32_t IRAM_ATTR picto_platform_since_rx(void) { return esp_timer_get_time() - callback_started; }
void picto_platform_mac(uint8_t mac[6]) { ESP_ERROR_CHECK(esp_wifi_get_mac(WIFI_IF_STA, mac)); }
int64_t IRAM_ATTR picto_platform_now(void) { return esp_timer_get_time(); }
bool picto_platform_channel(uint8_t channel) {
    return esp_wifi_set_channel(channel, WIFI_SECOND_CHAN_NONE) == ESP_OK;
}

static inline __attribute__((always_inline)) bool is_reply(const uint8_t *frame, size_t len) {
    return len >= 24 && frame[0] == 0x18;
}

/* Hands a frame to the control task; counts a drop when its queue is full. */
static void IRAM_ATTR copy_frame(const uint8_t *frame, size_t len, uint32_t timestamp, bool forward,
                                 bool sent) {
    rx_packet_t copy = {
        .len = len < COPY_BYTES ? len : COPY_BYTES,
        .air_len = (uint16_t)len,
        .timestamp = timestamp,
        .forward = forward,
        .sent = sent,
    };
    memcpy(copy.bytes, frame, copy.len);
    if (xQueueSend(input, &copy, 0) != pdTRUE) bridge_usb_drop();
}

/* PictoChat traffic worth capturing: local-multiplayer data (Nintendo's group
 * addresses appear as receiver or as BSSID) and the join/leave management frames. */
static inline __attribute__((always_inline)) bool is_pictochat(const uint8_t *frame, size_t len) {
    static const uint8_t group[3] = {3, 9, 0xbf};
    if (len < 24) return false;
    uint8_t subtype = frame[0] & 0xfc;
    if ((frame[0] & 0x0c) == 0x08) return !memcmp(frame + 4, group, 3) || !memcmp(frame + 16, group, 3);
    return subtype == 0x00 || subtype == 0x10 || subtype == 0xa0 || subtype == 0xb0 || subtype == 0xc0;
}

bool IRAM_ATTR picto_platform_tx(const uint8_t *frame, size_t len, uint32_t delay_us) {
    bool reply = is_reply(frame, len);
    /* When the DS host polls several clients at once they answer in slot
     * order, each in its own window. Sit out the windows of the slots before
     * ours, measured from the moment the poll reached this callback, so that
     * our reply keeps the same offset into its window as an immediate one. The
     * wait is a few milliseconds at most and other clients' replies, which we
     * do not need, queue in the driver meanwhile. */
    if (in_receive && delay_us > 0) {
        int64_t due = callback_started + delay_us;
        while (esp_timer_get_time() < due) {
        }
    }
    if (reply) {
        DIAG_ADD(DIAG_REPLY_COUNT, 1);
        DIAG_ADD(DIAG_REPLY_PREP_SUM, picto_platform_since_rx());
        DIAG_MAX(DIAG_REPLY_PREP_MAX, picto_platform_since_rx());
    }
    /* Fast path: this exact reply was staged before the poll arrived. */
    int prepared = in_receive ? picto_radio_try_prepared(frame, len) : -1;
    bool ok = prepared >= 0 ? prepared != 0
                            : esp_wifi_80211_tx(WIFI_IF_STA, frame, len, false) == ESP_OK;
    if (reply) DIAG_SET(DIAG_API_TIME, picto_platform_since_rx());
    /* Poll replies originate only in receive(), on the Wi-Fi task. Push them
     * through the driver queues now instead of waiting for its event loop.
     * Management frames from the control task keep the ordinary deferred path. */
    if (ok && prepared < 0 && reply && frame[1] == 0x11 && in_receive) {
        picto_radio_flush_reply();
        DIAG_SET(DIAG_FLUSH_TIME, picto_platform_since_rx());
    }
    /* Captures include what we send; the radio does not hear its own frames. */
    if (ok && bridge_tracing()) copy_frame(frame, len, hal_now(), false, true);
    return ok;
}

bool picto_platform_usb(const uint8_t *bytes, size_t len) {
    usb_packet_t packet = {.len = len};
    if (len > sizeof(packet.bytes)) return false;
    memcpy(packet.bytes, bytes, len);
    return xQueueSend(output, &packet, 0) == pdTRUE;
}

static void IRAM_ATTR receive(void *buffer, wifi_promiscuous_pkt_type_t type) {
    if (type != WIFI_PKT_MGMT && type != WIFI_PKT_DATA) return;
    const wifi_promiscuous_pkt_t *packet = buffer;
    if (packet->rx_ctrl.rx_state || packet->rx_ctrl.sig_len < 4) return;
    size_t len = packet->rx_ctrl.sig_len - 4; /* Strip the FCS. */
    callback_started = esp_timer_get_time();
    if (len >= 28 && packet->payload[0] == 0x28) {
        DIAG_SET(DIAG_RX_AGE, hal_now() - packet->rx_ctrl.timestamp);
        DIAG_SET(DIAG_RX_LENGTH, len);
    }
    in_receive = true;
    bridge_receive(packet->payload, len);
    bridge_prepare_reply();
    in_receive = false;

    static const uint8_t command[6] = {3, 9, 0xbf, 0, 0, 0};
    bool forward = len >= 34 && len <= COPY_BYTES && !memcmp(packet->payload + 4, command, 6);
    if (forward || (bridge_tracing() && is_pictochat(packet->payload, len)))
        copy_frame(packet->payload, len, packet->rx_ctrl.timestamp, forward, false);
}

static void IRAM_ATTR tx_done(const esp_80211_tx_info_t *info) {
    /* Our frames only: authentication, association, reply, deauthentication. */
    const uint8_t *frame = info->data;
    if (frame && (frame[0] == 0xb0 || frame[0] == 0 || frame[0] == 0x18 || frame[0] == 0xc0) &&
        (frame[1] & ~8) == (frame[0] == 0x18 ? 0x11 : 0x10))
        bridge_tx_complete(((uint16_t)frame[22] | (uint16_t)frame[23] << 8) >> 4,
                           info->tx_status == WIFI_SEND_SUCCESS);
    bridge_prepare_reply();
}

/* A separate task owns blocking UART writes; Wi-Fi and protocol tasks never wait. */
static void usb_writer(void *arg) {
    (void)arg;
    usb_packet_t packet;
    for (;;)
        if (xQueueReceive(output, &packet, portMAX_DELAY) == pdTRUE)
            uart_write_bytes(UART_NUM_0, packet.bytes, packet.len);
}

void app_main(void) {
    esp_log_level_set("*", ESP_LOG_NONE); /* UART0 carries only link packets. */
    ESP_ERROR_CHECK(nvs_flash_init());
    ESP_ERROR_CHECK(esp_event_loop_create_default());
    input = xQueueCreate(QUEUE_DEPTH, sizeof(rx_packet_t));
    output = xQueueCreate(QUEUE_DEPTH, sizeof(usb_packet_t));
    configASSERT(input && output);
    /* The receive buffer must hold one whole command while it is being decoded. */
    ESP_ERROR_CHECK(uart_driver_install(UART_NUM_0, UART_RX_BUFFER_BYTES, UART_TX_BUFFER_BYTES, 0,
                                        NULL, 0));
    ESP_ERROR_CHECK(uart_set_baudrate(UART_NUM_0, LINK_BAUD));
    configASSERT(xTaskCreate(usb_writer, "usb_writer", 3072, NULL, 5, NULL) == pdPASS);

    /* A station that never associates through the stack: 802.11b at 2 Mbit/s
     * like the DS, promiscuous RX and raw TX. */
    wifi_init_config_t init = WIFI_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_wifi_init(&init));
    ESP_ERROR_CHECK(esp_wifi_set_storage(WIFI_STORAGE_RAM));
    ESP_ERROR_CHECK(esp_wifi_set_mode(WIFI_MODE_STA));
    ESP_ERROR_CHECK(esp_wifi_set_protocol(WIFI_IF_STA, WIFI_PROTOCOL_11B));
    ESP_ERROR_CHECK(esp_wifi_start());
    ESP_ERROR_CHECK(esp_wifi_set_ps(WIFI_PS_NONE));
    ESP_ERROR_CHECK(esp_wifi_set_channel(1, WIFI_SECOND_CHAN_NONE));
    ESP_ERROR_CHECK(esp_wifi_config_80211_tx_rate(WIFI_IF_STA, WIFI_PHY_RATE_2M_S));
    ESP_ERROR_CHECK(esp_wifi_register_80211_tx_cb(tx_done));
    wifi_promiscuous_filter_t filter = {.filter_mask = WIFI_PROMIS_FILTER_MASK_MGMT |
                                                       WIFI_PROMIS_FILTER_MASK_DATA};
    ESP_ERROR_CHECK(esp_wifi_set_promiscuous_filter(&filter));
    ESP_ERROR_CHECK(esp_wifi_set_promiscuous_rx_cb(receive));
    bridge_init();
    ESP_ERROR_CHECK(esp_wifi_set_promiscuous(true));

    for (;;) {
        /* The 1 ms wait bounds how late a tick can notice a TX completion. */
        uint8_t bytes[64];
        int n = uart_read_bytes(UART_NUM_0, bytes, sizeof(bytes), pdMS_TO_TICKS(1));
        for (int i = 0; i < n; i++) bridge_usb_byte(bytes[i]);
        bridge_tick();
        rx_packet_t packet;
        for (unsigned i = 0; i < 8 && xQueueReceive(input, &packet, 0) == pdTRUE; i++) {
            if (packet.forward) bridge_forward(packet.bytes, packet.len);
            bridge_trace(packet.bytes, packet.len, packet.air_len, packet.timestamp, packet.sent);
        }
    }
}
