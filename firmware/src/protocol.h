#ifndef PROTOCOL_H
#define PROTOCOL_H

#include <stddef.h>
#include <stdint.h>

#define PROTO_MAGIC0 0xEB
#define PROTO_MAGIC1 0x90

#define PROTO_HEADER_BYTES  6
#define PROTO_MAX_PAYLOAD   4096

#define CMD_PING           0x01
#define CMD_INFO           0x02
#define CMD_SET_MODE       0x03
#define CMD_CLEAR          0x04
#define CMD_IMG_BEGIN      0x10
#define CMD_IMG_DATA       0x11
#define CMD_IMG_END        0x12
#define CMD_CONSOLE_WRITE  0x20
#define CMD_CONSOLE_RESIZE 0x21
#define CMD_SLEEP          0x30
#define CMD_RESET_BOOTSEL  0x31

#define RSP_ACK  0x80
#define RSP_NAK  0x81
#define RSP_BUSY 0x82

#define ERR_BAD_LENGTH 1
#define ERR_BAD_STATE  2
#define ERR_RANGE      3
#define ERR_UNKNOWN    4
#define ERR_CRC        5

uint16_t proto_crc16(const uint8_t *data, size_t len);

#endif
