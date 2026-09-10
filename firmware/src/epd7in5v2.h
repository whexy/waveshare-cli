#ifndef EPD7IN5V2_H
#define EPD7IN5V2_H

#include <stdbool.h>
#include <stdint.h>

#define EPD_WIDTH        800
#define EPD_HEIGHT       480
#define EPD_ROW_BYTES    (EPD_WIDTH / 8)
#define EPD_FRAME_BYTES  (EPD_ROW_BYTES * EPD_HEIGHT)

/* Wire format for panel RAM 0x13: a set bit is black, MSB is the leftmost
 * pixel, first byte is the top-left of the panel. The framebuffer here uses
 * that same convention so no inversion is needed on the way out. */
extern uint8_t epd_framebuffer[EPD_FRAME_BYTES];

void epd_init_hardware(void);

/* Refreshes run as a state machine so USB keeps being serviced while the
 * panel is busy; callers drive them with epd_poll() and watch epd_is_busy(). */
void epd_poll(void);
bool epd_is_busy(void);

void epd_start_full_refresh(void);
/* Window x bounds are byte columns, [x_byte_start, x_byte_end); the panel can
 * only address whole bytes horizontally. y bounds are rows, [y_start, y_end). */
void epd_start_partial_refresh(uint16_t x_byte_start, uint16_t x_byte_end,
                               uint16_t y_start, uint16_t y_end);
void epd_start_clear(bool black);
void epd_start_sleep(void);

void epd_framebuffer_fill(bool black);

#endif
