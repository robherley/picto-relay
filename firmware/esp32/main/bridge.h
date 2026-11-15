/* Glue between the hardware (platform.c), the protocol (client.c) and the USB
 * host (link.c). Owns the single client instance and serializes access to it. */
#pragma once
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* ---- Called by platform.c ---------------------------------------------- */

void bridge_init(void);

/* Wi-Fi task: every received management/data frame, FCS stripped. */
void bridge_receive(const uint8_t *frame, size_t len);
/* Wi-Fi task: driver finished transmitting the frame with this sequence. */
void bridge_tx_complete(uint16_t sequence, bool success);
/* Wi-Fi task: stage the next drawing reply ahead of its poll. */
void bridge_prepare_reply(void);

/* Control task: one byte from the host. */
void bridge_usb_byte(uint8_t byte);
/* Control task: periodic work and status reporting. */
void bridge_tick(void);
/* Control task: a host poll frame copied out of the Wi-Fi callback, to forward. */
void bridge_forward(const uint8_t *frame, size_t len);
/* Any task: the RX copy queue was full. */
void bridge_usb_drop(void);

/* ---- Provided by platform.c -------------------------------------------- */

void picto_platform_mac(uint8_t mac[6]);
int64_t picto_platform_now(void);
bool picto_platform_channel(uint8_t channel);
/* Submits a raw 802.11 frame; false if the driver rejected it. */
bool picto_platform_tx(const uint8_t *frame, size_t len);
/* Queues encoded bytes for the host without blocking; false if full. */
bool picto_platform_usb(const uint8_t *bytes, size_t len);
/* Stages `frame` as the prepared reply; len 0 discards any staged buffer. */
void picto_radio_prepare_reply(const uint8_t *frame, size_t len);
