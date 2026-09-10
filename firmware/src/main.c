#include <stdio.h>
#include <string.h>

#include "bsp/board.h"
#include "console.h"
#include "epd7in5v2.h"
#include "pico/binary_info.h"
#include "pico/bootrom.h"
#include "pico/stdlib.h"
#include "protocol.h"
#include "tusb.h"

#define FW_VERSION "0.1.0"

/* Holds a full 4 KB frame plus header/CRC several times over so the host can
 * keep streaming while a refresh occupies the main loop. Must stay a power of
 * two: ring_used() relies on unsigned wraparound being exact modulo the size. */
#define RX_RING_BYTES 32768
_Static_assert((RX_RING_BYTES & (RX_RING_BYTES - 1)) == 0,
               "RX_RING_BYTES must be a power of two");

#define CONSOLE_IDLE_MS 250

enum { MODE_PICTURE = 0, MODE_CONSOLE = 1 };

static uint8_t rx_ring[RX_RING_BYTES];
static size_t ring_head, ring_tail;

static uint8_t frame[PROTO_HEADER_BYTES + PROTO_MAX_PAYLOAD + 2];
static size_t frame_len;

static uint8_t image_staging[EPD_FRAME_BYTES];
static bool image_open;

static uint8_t mode = MODE_CONSOLE;
static uint32_t partials_since_full;
static absolute_time_t console_idle_deadline;
static bool console_pending;

/* Ghosting builds up over successive partial waveforms; fall back to a full
 * refresh on this cadence (Waveshare recommends every 5-10). */
#define MAX_PARTIALS_BEFORE_FULL 10

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

static void schedule_console_flush(void) {
    console_pending = true;
    console_idle_deadline = make_timeout_time_ms(CONSOLE_IDLE_MS);
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
                             " panel=7in5_v2 w=%d h=%d mode=%s",
                             EPD_WIDTH, EPD_HEIGHT,
                             mode == MODE_CONSOLE ? "console" : "picture");
            send_ack(seq, (const uint8_t *)info, (size_t)n);
            break;
        }

        case CMD_SET_MODE:
            if (len != 1) {
                send_nak(seq, ERR_BAD_LENGTH);
            } else if (payload[0] > 1) {
                send_nak(seq, ERR_RANGE);
            } else {
                mode = payload[0];
                if (mode == MODE_CONSOLE) console_reset();
                send_ack(seq, NULL, 0);
            }
            break;

        case CMD_CLEAR:
            if (len != 1) {
                send_nak(seq, ERR_BAD_LENGTH);
            } else if (epd_is_busy()) {
                send_busy(seq);
            } else {
                if (mode == MODE_CONSOLE) console_reset();
                console_pending = false;
                epd_start_clear(payload[0] != 0);
                partials_since_full = 0;
                send_ack(seq, NULL, 0);
            }
            break;

        case CMD_IMG_BEGIN:
            if (len != 5) {
                send_nak(seq, ERR_BAD_LENGTH);
            } else {
                uint16_t w = (uint16_t)(payload[0] | (payload[1] << 8));
                uint16_t h = (uint16_t)(payload[2] | (payload[3] << 8));
                if (w != EPD_WIDTH || h != EPD_HEIGHT || payload[4] != 0) {
                    send_nak(seq, ERR_RANGE);
                } else {
                    memset(image_staging, 0, sizeof image_staging);
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
            } else {
                uint32_t off = (uint32_t)payload[0] | ((uint32_t)payload[1] << 8) |
                               ((uint32_t)payload[2] << 16) |
                               ((uint32_t)payload[3] << 24);
                size_t n = len - 4;
                if (off > EPD_FRAME_BYTES || n > EPD_FRAME_BYTES - off) {
                    send_nak(seq, ERR_RANGE);
                } else {
                    memcpy(image_staging + off, payload + 4, n);
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
                memcpy(epd_framebuffer, image_staging, sizeof image_staging);
                image_open = false;
                mode = MODE_PICTURE;
                console_pending = false;
                if (payload[0] == 1 &&
                    partials_since_full < MAX_PARTIALS_BEFORE_FULL) {
                    epd_start_partial_refresh(0, EPD_HEIGHT);
                    partials_since_full++;
                } else {
                    epd_start_full_refresh();
                    partials_since_full = 0;
                }
                send_ack(seq, NULL, 0);
            }
            break;

        case CMD_CONSOLE_WRITE:
            if (mode != MODE_CONSOLE) {
                send_nak(seq, ERR_BAD_STATE);
            } else {
                console_write(payload, len);
                schedule_console_flush();
                send_ack(seq, NULL, 0);
            }
            break;

        case CMD_CONSOLE_RESIZE:
            send_nak(seq, ERR_UNKNOWN);
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
        frame_len = total;

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

static void console_tick(void) {
    if (!console_pending || epd_is_busy()) return;
    if (absolute_time_diff_us(get_absolute_time(), console_idle_deadline) > 0)
        return;

    uint16_t y0, y1;
    bool want_full = console_wants_full_refresh();
    if (!console_render(&y0, &y1)) {
        console_pending = false;
        return;
    }
    console_pending = false;

    if (want_full || partials_since_full >= MAX_PARTIALS_BEFORE_FULL) {
        epd_start_full_refresh();
        partials_since_full = 0;
    } else {
        epd_start_partial_refresh(y0, y1);
        partials_since_full++;
    }
}

int main(void) {
    bi_decl(bi_program_description(
        "Waveshare 7.5in V2 e-paper driver: framed USB CDC protocol for "
        "picture and serial-console modes"));
    bi_decl(bi_1pin_with_name(5, "EPD CS"));
    bi_decl(bi_1pin_with_name(6, "EPD SCK"));
    bi_decl(bi_1pin_with_name(7, "EPD DIN"));
    bi_decl(bi_1pin_with_name(8, "EPD DC"));
    bi_decl(bi_1pin_with_name(9, "EPD RST"));
    bi_decl(bi_1pin_with_name(10, "EPD BUSY"));

    board_init();
    tusb_init();

    epd_init_hardware();
    console_reset();
    /* Nothing is displayed until the host asks; console_reset only marks the
     * grid dirty. */
    console_pending = false;

    for (;;) {
        usb_pump();
        parse_ring();
        epd_poll();
        console_tick();
    }
}
