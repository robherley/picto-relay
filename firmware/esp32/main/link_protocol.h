/* Generated from crates/picto-relay-link/src/lib.rs. Do not edit.
 * Regenerate with: just protocol-header */
#pragma once

/* Protocol version. HELLO fails with a clear error when the two sides differ. */
#define LINK_PROTOCOL_VERSION 3

/* UART speed, 8N1. */
#define LINK_BAUD 921600

/* Largest payload of a command (host to ESP32). */
#define LINK_MAX_COMMAND_PAYLOAD 1024

/* Largest payload of a response or event (ESP32 to host). */
#define LINK_MAX_EVENT_PAYLOAD 256

/* The firmware leaves the room after this long without any command, in milliseconds. */
#define LINK_HOST_TIMEOUT_MS 5000

/* Interval between STATUS events while idle, in milliseconds. */
#define LINK_STATUS_INTERVAL_MS 1000

/* Interval between STATUS events while a drawing is being sent, in milliseconds. */
#define LINK_STATUS_SENDING_INTERVAL_MS 250

/* Marks the start and end of every packet. */
#define LINK_DELIMITER 126

/* Precedes an escaped byte. */
#define LINK_ESCAPE 125

/* XORed with a byte to escape or unescape it. */
#define LINK_ESCAPE_XOR 32

/* Packet header: offset of the packet kind. */
#define LINK_KIND_OFFSET 0

/* Packet header: offset of the little-endian u32 request or transfer ID. */
#define LINK_ID_OFFSET 1

/* Packet header: offset of the little-endian u16 payload length. */
#define LINK_LENGTH_OFFSET 5

/* Packet header length; the payload starts here. */
#define LINK_PAYLOAD_OFFSET 7

/* Length of the little-endian IEEE CRC32 that follows the payload. */
#define LINK_CRC_LEN 4

/* Attach to the firmware. Payload: the host's protocol version. */
#define LINK_HELLO 1

/* Find and join a room. Payload: see the JOIN_* layout. */
#define LINK_JOIN 2

/* Leave the room, telling the DS host. */
#define LINK_LEAVE 3

/* Keep the session alive; any command does, this one has no other effect. */
#define LINK_PING 4

/* While idle, sweep all channels once and report a ROOMS event. */
#define LINK_SCAN 5

/* Upload part of a drawing. Payload: see the DRAW_* layout. */
#define LINK_DRAW 6

/* Turn frame capture on (payload 1) or off (payload 0). While on, every */
/* PictoChat radio frame heard or sent is reported as a FRAME event. */
#define LINK_TRACE 7

/* Answer to a command, with the same ID. Payload: status byte, then data. */
#define LINK_RESPONSE 128

/* Status: the command succeeded. */
#define LINK_STATUS_OK 0

/* Status: try again shortly (the device or its drawing buffer is occupied). */
#define LINK_STATUS_BUSY 1

/* Status: malformed command; retrying cannot help. */
#define LINK_STATUS_INVALID 2

/* Status: the command does not apply in the current session state. */
#define LINK_STATUS_WRONG_STATE 3

/* Status: HELLO from a host with a different protocol version. */
#define LINK_STATUS_VERSION_MISMATCH 4

/* Status: a command arrived before HELLO. */
#define LINK_STATUS_NOT_ATTACHED 5

/* HELLO response data: offset of the firmware's protocol version. */
#define LINK_HELLO_VERSION_OFFSET 0

/* HELLO response data: offset of the station MAC address. */
#define LINK_HELLO_MAC_OFFSET 1

/* HELLO response data: offset of the firmware build label (ASCII, to the end). */
#define LINK_HELLO_BUILD_OFFSET 7

/* Longest firmware build label. */
#define LINK_MAX_BUILD_LEN 32

/* Octets in a MAC address. */
#define LINK_MAC_LEN 6

/* PictoChat rooms A through D. */
#define LINK_ROOM_COUNT 4

/* Nickname field: ten UTF-16LE code units, zero-padded. */
#define LINK_NICKNAME_LEN 20

/* Personal message field: 26 UTF-16LE code units, zero-padded. */
#define LINK_PROFILE_MESSAGE_LEN 52

/* Highest DS profile color index. */
#define LINK_MAX_PROFILE_COLOR 15

/* Largest native drawing: 36 bytes of metadata and five 2048-byte bands. */
#define LINK_MAX_DRAWING_BYTES 10276

/* Smallest native drawing: metadata and one band. */
#define LINK_MIN_DRAWING_BYTES 2084

/* JOIN: offset of the room number, 0 to 3. */
#define LINK_JOIN_ROOM_OFFSET 0

/* JOIN: offset of the nickname. */
#define LINK_JOIN_NICKNAME_OFFSET 1

/* JOIN: offset of the personal message. */
#define LINK_JOIN_MESSAGE_OFFSET 21

/* JOIN: offset of the profile color. */
#define LINK_JOIN_COLOR_OFFSET 73

/* JOIN: payload length. */
#define LINK_JOIN_LEN 74

/* DRAW: offset of the little-endian u32 transfer ID chosen by the host. */
#define LINK_DRAW_ID_OFFSET 0

/* DRAW: offset of the little-endian u16 position of this chunk in the drawing. */
#define LINK_DRAW_POSITION_OFFSET 4

/* DRAW: offset of the little-endian u16 total drawing length. */
#define LINK_DRAW_TOTAL_OFFSET 6

/* DRAW: offset of the chunk's drawing bytes, which run to the end. */
#define LINK_DRAW_DATA_OFFSET 8

/* DRAW: most drawing bytes in one command. */
#define LINK_MAX_DRAW_CHUNK 1016

/* Periodic session report; the packet ID is the drawing being sent, if any. */
#define LINK_STATUS 129

/* A DS application packet from the room host, forwarded once. */
#define LINK_RX 130

/* Events were dropped because the output queue was full. Payload: u32 count. */
#define LINK_GAP 131

/* Radio timing counters, only from firmware built with PICTO_DIAGNOSTICS. */
#define LINK_DIAG 132

/* The profile exchange finished; drawings may be sent. */
#define LINK_READY 133

/* The DS confirmed the drawing whose transfer ID is the packet ID. */
#define LINK_DELIVERED 134

/* The session ended. Payload: see the LEFT_* layout. */
#define LINK_LEFT 135

/* Result of a SCAN: member counts of rooms A to D. */
#define LINK_ROOMS 136

/* STATUS: offset of the session state. */
#define LINK_STATUS_STATE_OFFSET 0

/* STATUS: offset of the profile-exchange phase. */
#define LINK_STATUS_PHASE_OFFSET 1

/* STATUS: offset of the participant slot. */
#define LINK_STATUS_SLOT_OFFSET 2

/* STATUS: offset of the room number. */
#define LINK_STATUS_ROOM_OFFSET 3

/* STATUS: offset of the Wi-Fi channel. */
#define LINK_STATUS_CHANNEL_OFFSET 4

/* STATUS: offset of the number of fragments the DS has acknowledged. */
#define LINK_STATUS_SENT_OFFSET 5

/* STATUS: offset of the number of fragments in the drawing being sent. */
#define LINK_STATUS_TOTAL_OFFSET 6

/* STATUS: offset of the number of fragments the DS has echoed back. */
#define LINK_STATUS_ECHOED_OFFSET 7

/* STATUS: offset of the little-endian u32 counters: polls, replies, */
/* positive acknowledgements, negative acknowledgements. */
#define LINK_STATUS_COUNTERS_OFFSET 8

/* STATUS: number of counters. */
#define LINK_STATUS_COUNTERS 4

/* STATUS: payload length. */
#define LINK_STATUS_LEN 24

/* Session state: idle. */
#define LINK_STATE_OFF 0

/* Session state: searching channels for the room. */
#define LINK_STATE_DISCOVER 1

/* Session state: authenticating with the DS host. */
#define LINK_STATE_AUTHENTICATING 2

/* Session state: associating with the DS host. */
#define LINK_STATE_ASSOCIATING 3

/* Session state: in the room. */
#define LINK_STATE_CONNECTED 4

/* Profile phase at which the exchange is complete. */
#define LINK_PHASE_READY 12

/* READY: offset of the DS host's MAC address. */
#define LINK_READY_HOST_OFFSET 0

/* READY: offset of the participant slot. */
#define LINK_READY_SLOT_OFFSET 6

/* READY: offset of the room number. */
#define LINK_READY_ROOM_OFFSET 7

/* READY: offset of the Wi-Fi channel. */
#define LINK_READY_CHANNEL_OFFSET 8

/* READY: payload length. */
#define LINK_READY_LEN 9

/* LEFT: offset of the reason. */
#define LINK_LEFT_REASON_OFFSET 0

/* LEFT: offset of the little-endian u16 IEEE status or reason code, if any. */
#define LINK_LEFT_CODE_OFFSET 1

/* LEFT: payload length. */
#define LINK_LEFT_LEN 3

/* Left because the host asked, or stopped sending commands. */
#define LINK_LEFT_REQUESTED 0

/* No DS is hosting the room on any channel. */
#define LINK_LEFT_NO_HOST 1

/* The DS host did not answer the join. */
#define LINK_LEFT_JOIN_TIMEOUT 2

/* The DS host refused the join; the code is its IEEE status. */
#define LINK_LEFT_REJECTED 3

/* The DS host removed us; the code is its IEEE reason. */
#define LINK_LEFT_KICKED 4

/* Nothing was heard from the DS host for the session timeout. */
#define LINK_LEFT_HOST_LOST 5

/* The radio driver never reported a transmission as finished. */
#define LINK_LEFT_RADIO_STALLED 6

/* ROOMS: member count for a room whose beacon was not heard. */
#define LINK_NO_BEACON 255

/* One captured 802.11 frame, without its FCS; sent only while TRACE is on. */
#define LINK_FRAME 137

/* FRAME: offset of the little-endian u32 radio clock in microseconds. */
#define LINK_FRAME_TIME_OFFSET 0

/* FRAME: offset of the flags. */
#define LINK_FRAME_FLAGS_OFFSET 4

/* FRAME: offset of the little-endian u16 length of the frame on the air. */
#define LINK_FRAME_LENGTH_OFFSET 5

/* FRAME: offset of the frame bytes, which may be cut short. */
#define LINK_FRAME_DATA_OFFSET 7

/* FRAME: most frame bytes carried; longer frames are truncated. */
#define LINK_FRAME_MAX_BYTES 249

/* FRAME flag: the relay transmitted this frame (otherwise it received it). */
#define LINK_FRAME_FLAG_SENT 1
