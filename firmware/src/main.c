#include <stdio.h>
#include <string.h>

#include "bsp/board.h"
#include "epd7in5v2.h"
#include "pico/binary_info.h"
#include "pico/bootrom.h"
#include "pico/stdlib.h"
#include "protocol.h"
#include "tusb.h"

#define FW_VERSION "0.3.0"

/* Holds a full 4 KB frame plus header/CRC several times over so the host can
 * keep streaming while a refresh occupies the main loop. Must stay a power of
 * two: ring_used() relies on unsigned wraparound being exact modulo the size. */
#define RX_RING_BYTES 32768
_Static_assert((RX_RING_BYTES & (RX_RING_BYTES - 1)) == 0,
               "RX_RING_BYTES must be a power of two");

#define BLIT_HEADER_BYTES 8

static uint8_t rx_ring[RX_RING_BYTES];
static size_t ring_head, ring_tail;

static uint8_t frame[PROTO_HEADER_BYTES + PROTO_MAX_PAYLOAD + 2];

static bool image_open;

/* Region of the framebuffer written since the last refresh, x in byte columns
 * and y in rows, both half-open. */
static bool dirty_valid;
static uint16_t dirty_x0, dirty_y0, dirty_x1, dirty_y1;

static uint16_t partials_since_full;
static uint32_t last_full_ms;

static size_t ring_used(void) {
    return (ring_head - ring_tail) % RX_RING_BYTES;
}

static void ring_push(const uint8_t *data, size_t len) {
    for (size_t i = 0; i < len; i++) {
        size_t next = (ring_head + 1) % RX_RING_BYTES;
        if (next == ring_tail) return; /* full: drop, host retries on timeout */
        rx_ring[ring_head] = data[i];
        ring_head = next;
    }
}

static uint8_t ring_peek(size_t offset) {
    return rx_ring[(ring_tail + offset) % RX_RING_BYTES];
}

static void ring_drop(size_t count) {
    ring_tail = (ring_tail + count) % RX_RING_BYTES;
}

static void usb_pump(void) {
    tud_task();
    if (!tud_cdc_available()) return;
    uint8_t buf[64];
    while (tud_cdc_available()) {
        uint32_t n = tud_cdc_read(buf, sizeof buf);
        if (!n) break;
        ring_push(buf, n);
    }
}

static void cdc_write_all(const uint8_t *data, size_t len) {
    size_t sent = 0;
    while (sent < len) {
        if (!tud_cdc_connected()) return;
        uint32_t n = tud_cdc_write(data + sent, (uint32_t)(len - sent));
        sent += n;
        tud_cdc_write_flush();
        if (!n) usb_pump();
    }
    tud_cdc_write_flush();
}

static void send_frame(uint8_t type, uint8_t seq, const uint8_t *payload,
                       size_t len) {
    uint8_t hdr[PROTO_HEADER_BYTES];
    hdr[0] = PROTO_MAGIC0;
    hdr[1] = PROTO_MAGIC1;
    hdr[2] = type;
    hdr[3] = seq;
    hdr[4] = (uint8_t)(len & 0xFF);
    hdr[5] = (uint8_t)(len >> 8);

    uint16_t crc = 0xFFFF;
    {
        /* CRC covers type, seq, len and payload. */
        uint8_t meta[4] = {hdr[2], hdr[3], hdr[4], hdr[5]};
        crc = proto_crc16(meta, 4);
        for (size_t i = 0; i < len; i++) {
            crc ^= (uint16_t)payload[i] << 8;
            for (int b = 0; b < 8; b++)
                crc = (crc & 0x8000) ? (uint16_t)((crc << 1) ^ 0x1021)
                                     : (uint16_t)(crc << 1);
        }
    }

    cdc_write_all(hdr, sizeof hdr);
    if (len) cdc_write_all(payload, len);
    uint8_t tail[2] = {(uint8_t)(crc & 0xFF), (uint8_t)(crc >> 8)};
    cdc_write_all(tail, 2);
}

static void send_ack(uint8_t seq, const uint8_t *payload, size_t len) {
    send_frame(RSP_ACK, seq, payload, len);
}

static void send_nak(uint8_t seq, uint8_t code) {
    send_frame(RSP_NAK, seq, &code, 1);
}

static void send_busy(uint8_t seq) { send_frame(RSP_BUSY, seq, NULL, 0); }

static uint16_t rd_u16(const uint8_t *p) {
    return (uint16_t)(p[0] | (p[1] << 8));
}

static void dirty_reset(void) { dirty_valid = false; }

static void dirty_add(uint16_t x0, uint16_t y0, uint16_t x1, uint16_t y1) {
    if (!dirty_valid) {
        dirty_x0 = x0;
        dirty_y0 = y0;
        dirty_x1 = x1;
        dirty_y1 = y1;
        dirty_valid = true;
        return;
    }
    if (x0 < dirty_x0) dirty_x0 = x0;
    if (y0 < dirty_y0) dirty_y0 = y0;
    if (x1 > dirty_x1) dirty_x1 = x1;
    if (y1 > dirty_y1) dirty_y1 = y1;
}

static void start_full(void) {
    epd_start_full_refresh();
    partials_since_full = 0;
    last_full_ms = to_ms_since_boot(get_absolute_time());
    dirty_reset();
}

/* The device may run a partial as a full one; keep the counters describing
 * what the panel actually did so the host's ghosting budget stays honest. */
static void start_partial(uint16_t x0, uint16_t x1, uint16_t y0, uint16_t y1) {
    if (epd_start_partial_refresh(x0, x1, y0, y1)) {
        partials_since_full = 0;
        last_full_ms = to_ms_since_boot(get_absolute_time());
    } else {
        partials_since_full++;
    }
    dirty_reset();
}

static void handle_blit(uint8_t seq, const uint8_t *payload, size_t len) {
    if (len < BLIT_HEADER_BYTES) {
        send_nak(seq, ERR_BAD_LENGTH);
        return;
    }
    uint16_t x = rd_u16(payload);
    uint16_t y = rd_u16(payload + 2);
    uint16_t w = rd_u16(payload + 4);
    uint16_t h = rd_u16(payload + 6);

    if (len != BLIT_HEADER_BYTES + (size_t)w * h) {
        send_nak(seq, ERR_BAD_LENGTH);
        return;
    }
    if (w == 0 || h == 0 || x + w > EPD_ROW_BYTES || y + h > EPD_HEIGHT) {
        send_nak(seq, ERR_RANGE);
        return;
    }

    const uint8_t *src = payload + BLIT_HEADER_BYTES;
    for (uint16_t row = 0; row < h; row++)
        memcpy(epd_framebuffer + (size_t)(y + row) * EPD_ROW_BYTES + x,
               src + (size_t)row * w, w);

    dirty_add(x, y, (uint16_t)(x + w), (uint16_t)(y + h));
    send_ack(seq, NULL, 0);
}

static void handle_status(uint8_t seq) {
    uint32_t since = to_ms_since_boot(get_absolute_time()) - last_full_ms;
    uint8_t s[PROTO_STATUS_BYTES];
    s[0] = epd_is_busy() ? 1 : 0;
    s[1] = (uint8_t)(partials_since_full & 0xFF);
    s[2] = (uint8_t)(partials_since_full >> 8);
    s[3] = (uint8_t)(since & 0xFF);
    s[4] = (uint8_t)((since >> 8) & 0xFF);
    s[5] = (uint8_t)((since >> 16) & 0xFF);
    s[6] = (uint8_t)((since >> 24) & 0xFF);
    s[7] = dirty_valid ? 1 : 0;
    s[8] = (uint8_t)(dirty_x0 & 0xFF);
    s[9] = (uint8_t)(dirty_x0 >> 8);
    s[10] = (uint8_t)(dirty_y0 & 0xFF);
    s[11] = (uint8_t)(dirty_y0 >> 8);
    s[12] = (uint8_t)(dirty_x1 & 0xFF);
    s[13] = (uint8_t)(dirty_x1 >> 8);
    s[14] = (uint8_t)(dirty_y1 & 0xFF);
    s[15] = (uint8_t)(dirty_y1 >> 8);
    send_ack(seq, s, sizeof s);
}

static void handle_command(uint8_t type, uint8_t seq, const uint8_t *payload,
                           size_t len) {
    switch (type) {
        case CMD_PING:
            send_ack(seq, (const uint8_t *)"PONG", 4);
            break;

        case CMD_INFO: {
            char info[96];
            int n = snprintf(info, sizeof info,
                             "epaper-fw " FW_VERSION
                             " panel=7in5_v2 w=%d h=%d proto=2",
                             EPD_WIDTH, EPD_HEIGHT);
            send_ack(seq, (const uint8_t *)info, (size_t)n);
            break;
        }

        case CMD_STATUS:
            handle_status(seq);
            break;

        case CMD_CLEAR:
            if (len != 1) {
                send_nak(seq, ERR_BAD_LENGTH);
            } else if (epd_is_busy()) {
                send_busy(seq);
            } else {
                epd_framebuffer_fill(payload[0] != 0);
                start_full();
                send_ack(seq, NULL, 0);
            }
            break;

        case CMD_IMG_BEGIN:
            if (len != 5) {
                send_nak(seq, ERR_BAD_LENGTH);
            } else if (epd_is_busy()) {
                send_busy(seq);
            } else {
                uint16_t w = rd_u16(payload);
                uint16_t h = rd_u16(payload + 2);
                if (w != EPD_WIDTH || h != EPD_HEIGHT || payload[4] != 0) {
                    send_nak(seq, ERR_RANGE);
                } else {
                    image_open = true;
                    send_ack(seq, NULL, 0);
                }
            }
            break;

        case CMD_IMG_DATA:
            if (len < 4) {
                send_nak(seq, ERR_BAD_LENGTH);
            } else if (!image_open) {
                send_nak(seq, ERR_BAD_STATE);
            } else if (epd_is_busy()) {
                send_busy(seq);
            } else {
                uint32_t off = (uint32_t)payload[0] |
                               ((uint32_t)payload[1] << 8) |
                               ((uint32_t)payload[2] << 16) |
                               ((uint32_t)payload[3] << 24);
                size_t n = len - 4;
                if (off > EPD_FRAME_BYTES || n > EPD_FRAME_BYTES - off) {
                    send_nak(seq, ERR_RANGE);
                } else {
                    memcpy(epd_framebuffer + off, payload + 4, n);
                    send_ack(seq, NULL, 0);
                }
            }
            break;

        case CMD_IMG_END:
            if (len != 1) {
                send_nak(seq, ERR_BAD_LENGTH);
            } else if (!image_open) {
                send_nak(seq, ERR_BAD_STATE);
            } else if (epd_is_busy()) {
                send_busy(seq);
            } else {
                image_open = false;
                if (payload[0] == 1) {
                    start_partial(0, EPD_ROW_BYTES, 0, EPD_HEIGHT);
                } else {
                    start_full();
                }
                send_ack(seq, NULL, 0);
            }
            break;

        case CMD_BLIT:
            if (epd_is_busy()) {
                send_busy(seq);
            } else {
                handle_blit(seq, payload, len);
            }
            break;

        case CMD_REFRESH:
            if (len != 1) {
                send_nak(seq, ERR_BAD_LENGTH);
            } else if (epd_is_busy()) {
                send_busy(seq);
            } else if (payload[0] == 0) {
                start_full();
                send_ack(seq, NULL, 0);
            } else if (payload[0] != 1) {
                send_nak(seq, ERR_RANGE);
            } else if (!dirty_valid) {
                send_nak(seq, ERR_BAD_STATE);
            } else {
                start_partial(dirty_x0, dirty_x1, dirty_y0, dirty_y1);
                send_ack(seq, NULL, 0);
            }
            break;

        case CMD_SLEEP:
            if (epd_is_busy()) {
                send_busy(seq);
            } else {
                epd_start_sleep();
                send_ack(seq, NULL, 0);
            }
            break;

        case CMD_RESET_BOOTSEL:
            send_ack(seq, NULL, 0);
            /* Let the ACK reach the host before the USB stack disappears. */
            for (int i = 0; i < 50; i++) {
                usb_pump();
                sleep_ms(2);
            }
            reset_usb_boot(0, 0);
            break;

        default:
            send_nak(seq, ERR_UNKNOWN);
            break;
    }
}

/* Consumes whole frames from the ring, resyncing on the magic bytes after any
 * framing or CRC error. */
static void parse_ring(void) {
    for (;;) {
        while (ring_used() >= 1 && ring_peek(0) != PROTO_MAGIC0) ring_drop(1);
        if (ring_used() < 2) return;
        if (ring_peek(1) != PROTO_MAGIC1) {
            ring_drop(1);
            continue;
        }
        if (ring_used() < PROTO_HEADER_BYTES) return;

        uint8_t type = ring_peek(2);
        uint8_t seq = ring_peek(3);
        size_t len = (size_t)ring_peek(4) | ((size_t)ring_peek(5) << 8);

        if (len > PROTO_MAX_PAYLOAD) {
            ring_drop(2);
            continue;
        }
        size_t total = PROTO_HEADER_BYTES + len + 2;
        if (ring_used() < total) return;

        for (size_t i = 0; i < total; i++) frame[i] = ring_peek(i);

        uint16_t want = (uint16_t)(frame[PROTO_HEADER_BYTES + len] |
                                   (frame[PROTO_HEADER_BYTES + len + 1] << 8));
        uint16_t got = proto_crc16(frame + 2, 4 + len);

        if (want != got) {
            send_nak(seq, ERR_CRC);
            ring_drop(2);
            continue;
        }

        ring_drop(total);
        handle_command(type, seq, frame + PROTO_HEADER_BYTES, len);
    }
}

int main(void) {
    bi_decl(bi_program_description(
        "Waveshare 7.5in V2 e-paper framebuffer: framed USB CDC protocol with "
        "full-frame images and windowed partial refresh"));
    bi_decl(bi_1pin_with_name(5, "EPD CS"));
    bi_decl(bi_1pin_with_name(6, "EPD SCK"));
    bi_decl(bi_1pin_with_name(7, "EPD DIN"));
    bi_decl(bi_1pin_with_name(8, "EPD DC"));
    bi_decl(bi_1pin_with_name(9, "EPD RST"));
    bi_decl(bi_1pin_with_name(10, "EPD BUSY"));

    board_init();

    /* Resets and powers the panel, which blocks for a few hundred ms and longer
     * still if BUSY never releases. Done before the USB pull-up goes up so a
     * missing or wedged panel delays enumeration instead of failing it. */
    epd_init_hardware();

    tusb_init();

    for (;;) {
        usb_pump();
        parse_ring();
        epd_poll();
    }
}
