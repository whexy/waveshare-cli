#ifndef CONSOLE_H
#define CONSOLE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define CONSOLE_COLS 100
#define CONSOLE_ROWS 30

void console_reset(void);
void console_write(const uint8_t *buf, size_t len);

/* Renders dirty rows into epd_framebuffer and reports the row band that
 * changed. Returns false when nothing was dirty. */
bool console_render(uint16_t *y_start, uint16_t *y_end);

bool console_dirty(void);

/* Set when content scrolled off or the screen was cleared, i.e. when a
 * partial refresh would leave visible ghosting. Cleared by console_render. */
bool console_wants_full_refresh(void);

#endif
