#ifndef EPD7IN5V2_H
#define EPD7IN5V2_H

#include <stdbool.h>
#include <stdint.h>

#define EPD_WIDTH       800
#define EPD_HEIGHT      480
#define EPD_ROW_BYTES   (EPD_WIDTH / 8)
#define EPD_FRAME_BYTES (EPD_ROW_BYTES * EPD_HEIGHT)

/* 4-gray carries two bits per pixel in its own buffer: 0 is white and 3 is
 * black, leftmost pixel in the high bits. */
#define EPD_GRAY_ROW_BYTES   (EPD_WIDTH / 4)
#define EPD_GRAY_FRAME_BYTES (EPD_GRAY_ROW_BYTES * EPD_HEIGHT)

/* A set bit is black, MSB is the leftmost pixel, first byte is the top-left of
 * the panel. Both panel RAM planes take the opposite convention, so rows are
 * inverted on the way out. */
extern uint8_t epd_framebuffer[EPD_FRAME_BYTES];

extern uint8_t epd_gray_framebuffer[EPD_GRAY_FRAME_BYTES];

void epd_init_hardware(void);

/* Refreshes run as a state machine so USB keeps being serviced while the
 * panel is busy; callers drive them with epd_poll() and watch epd_is_busy().
 * The panel stays powered and configured between refreshes: partial ~0.5 s,
 * full ~4.3 s. */
void epd_poll(void);
bool epd_is_busy(void);

void epd_start_full_refresh(void);

/* Push epd_gray_framebuffer as four tones. Always a whole-panel refresh of
 * about 2.6 s: gray needs both panel RAM planes for its two bits, so no plane
 * is left to diff a partial against. Leaves the panel's old-data RAM holding
 * gray bits, which is why the next mono partial must run as a full refresh. */
void epd_start_gray_refresh(void);
/* Window x bounds are byte columns, [x_byte_start, x_byte_end); the panel can
 * only address whole bytes horizontally. y bounds are rows, [y_start, y_end).
 * Returns true if the request had to run as a full refresh instead, which
 * happens for the first refresh after power-up or deep sleep. */
bool epd_start_partial_refresh(uint16_t x_byte_start, uint16_t x_byte_end,
                               uint16_t y_start, uint16_t y_end);
void epd_start_clear(bool black);
void epd_start_sleep(void);

void epd_framebuffer_fill(bool black);

#endif
