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

/* Host polls copied out of the Wi-Fi callback for forwarding over USB. */
typedef struct {
    size_t len;
    uint8_t bytes[256];
} rx_packet_t;
/* Encoded packets waiting for the blocking UART writer. */
typedef struct {
    size_t len;
    uint8_t bytes[LINK_MAX_ENCODED];
} usb_packet_t;
static QueueHandle_t input, output;

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

bool IRAM_ATTR picto_platform_tx(const uint8_t *frame, size_t len) {
    bool reply = is_reply(frame, len);
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
    if (len >= 34 && len <= 256 && !memcmp(packet->payload + 4, command, 6)) {
        rx_packet_t copy = {.len = len};
        memcpy(copy.bytes, packet->payload, len);
        if (xQueueSend(input, &copy, 0) != pdTRUE) bridge_usb_drop();
    }
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
    input = xQueueCreate(64, sizeof(rx_packet_t));
    output = xQueueCreate(64, sizeof(usb_packet_t));
    configASSERT(input && output);
    ESP_ERROR_CHECK(uart_driver_install(UART_NUM_0, 2048, 2048, 0, NULL, 0));
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
        for (unsigned i = 0; i < 8 && xQueueReceive(input, &packet, 0) == pdTRUE; i++)
            bridge_forward(packet.bytes, packet.len);
    }
}
