/* Host-side tests for console.c and protocol.c. Built and run by
 * firmware/test/run.sh with the native compiler. */
#include <stdio.h>
#include <string.h>

#include "console.h"
#include "epd7in5v2.h"
#include "font8x16.h"
#include "protocol.h"

/* console.c renders into this buffer, which normally lives in the driver. */
uint8_t epd_framebuffer[EPD_FRAME_BYTES];

static int failures;

#define CHECK(cond)                                                      \
    do {                                                                 \
        if (!(cond)) {                                                   \
            printf("FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond);       \
            failures++;                                                  \
        }                                                                \
    } while (0)

static void write_str(const char *s) {
    console_write((const uint8_t *)s, strlen(s));
}

/* Reads back a cell by comparing rendered pixels against the font glyph. */
static int cell_is(uint8_t row, uint8_t col, char expect) {
    const uint8_t *glyph = NULL;
    if (expect >= FONT_FIRST_CHAR && expect <= FONT_LAST_CHAR)
        glyph = font8x16[(uint8_t)expect - FONT_FIRST_CHAR];
    for (int y = 0; y < 16; y++) {
        size_t idx = ((size_t)row * 16 + y) * EPD_ROW_BYTES + col;
        uint8_t want = glyph ? glyph[y] : 0x00;
        if (epd_framebuffer[idx] != want) return 0;
    }
    return 1;
}

static void render_all(void) {
    uint16_t a, b;
    console_render(&a, &b);
}

static void test_crc(void) {
    /* CRC-16/CCITT-FALSE check value for "123456789" is 0x29B1. */
    CHECK(proto_crc16((const uint8_t *)"123456789", 9) == 0x29B1);
    CHECK(proto_crc16((const uint8_t *)"", 0) == 0xFFFF);
}

static void test_plain_text(void) {
    console_reset();
    write_str("hello");
    render_all();
    CHECK(cell_is(0, 0, 'h'));
    CHECK(cell_is(0, 4, 'o'));
    CHECK(cell_is(0, 5, ' '));
}

static void test_newline_and_cr(void) {
    console_reset();
    write_str("ab\ncd");
    render_all();
    CHECK(cell_is(0, 0, 'a'));
    CHECK(cell_is(1, 0, 'c'));
    CHECK(cell_is(1, 1, 'd'));

    console_reset();
    write_str("abc\rX");
    render_all();
    CHECK(cell_is(0, 0, 'X'));
    CHECK(cell_is(0, 1, 'b'));
}

static void test_backspace_and_tab(void) {
    console_reset();
    write_str("abc\b\bZ");
    render_all();
    CHECK(cell_is(0, 1, 'Z'));
    CHECK(cell_is(0, 2, 'c'));

    console_reset();
    write_str("a\tb");
    render_all();
    CHECK(cell_is(0, 0, 'a'));
    CHECK(cell_is(0, 8, 'b'));
}

static void test_wrap_and_scroll(void) {
    console_reset();
    for (int i = 0; i < CONSOLE_COLS; i++) write_str("x");
    write_str("Y");
    render_all();
    CHECK(cell_is(0, CONSOLE_COLS - 1, 'x'));
    CHECK(cell_is(1, 0, 'Y'));

    console_reset();
    for (int r = 0; r < CONSOLE_ROWS; r++) write_str("r\n");
    write_str("last");
    render_all();
    /* The first line scrolled off, so the bottom row holds the newest text. */
    CHECK(cell_is(CONSOLE_ROWS - 1, 0, 'l'));
    CHECK(console_wants_full_refresh() == false);
}

static void test_csi_cursor(void) {
    console_reset();
    write_str("\x1b[3;5Hhi");
    render_all();
    CHECK(cell_is(2, 4, 'h'));
    CHECK(cell_is(2, 5, 'i'));

    console_reset();
    write_str("abc\x1b[2Dz");
    render_all();
    CHECK(cell_is(0, 1, 'z'));

    console_reset();
    write_str("\x1b[Hq");
    render_all();
    CHECK(cell_is(0, 0, 'q'));
}

static void test_csi_erase(void) {
    console_reset();
    write_str("abcdef\x1b[4G");
    write_str("\x1b[K");
    render_all();
    /* ESC[4G is not in the supported subset, so the cursor stayed put and
     * ESC[K cleared from there to end of line. */
    CHECK(cell_is(0, 0, 'a'));

    console_reset();
    write_str("hello\x1b[2J");
    render_all();
    CHECK(cell_is(0, 0, ' '));
    CHECK(cell_is(0, 1, ' '));

    console_reset();
    write_str("abcdef");
    write_str("\x1b[1;3H\x1b[K");
    render_all();
    CHECK(cell_is(0, 0, 'a'));
    CHECK(cell_is(0, 1, 'b'));
    CHECK(cell_is(0, 2, ' '));
    CHECK(cell_is(0, 5, ' '));
}

static void test_ignored_sequences(void) {
    console_reset();
    write_str("\x1b[1;32mgreen\x1b[0m!");
    render_all();
    CHECK(cell_is(0, 0, 'g'));
    CHECK(cell_is(0, 5, '!'));

    console_reset();
    write_str("\x1b[?25lhi");
    render_all();
    CHECK(cell_is(0, 0, 'h'));

    console_reset();
    write_str("\x1b]0;window title\x07ok");
    render_all();
    CHECK(cell_is(0, 0, 'o'));
    CHECK(cell_is(0, 1, 'k'));

    console_reset();
    write_str("\x1b]0;t\x1b\\ok");
    render_all();
    CHECK(cell_is(0, 0, 'o'));
}

static void test_dirty_band(void) {
    console_reset();
    render_all(); /* consume the reset-dirty state */

    write_str("\x1b[5;1Hmid");
    uint16_t y0 = 0, y1 = 0;
    CHECK(console_dirty());
    CHECK(console_render(&y0, &y1));
    CHECK(y0 == 4 * 16);
    CHECK(y1 == 5 * 16);
    CHECK(!console_dirty());
    CHECK(!console_render(&y0, &y1));
}

static void test_full_refresh_flag(void) {
    console_reset();
    CHECK(console_wants_full_refresh());
    render_all();
    CHECK(!console_wants_full_refresh());

    write_str("\x1b[2J");
    CHECK(console_wants_full_refresh());
    render_all();

    for (int r = 0; r < CONSOLE_ROWS + 2; r++) write_str("line\n");
    CHECK(console_wants_full_refresh());
}

int main(void) {
    test_crc();
    test_plain_text();
    test_newline_and_cr();
    test_backspace_and_tab();
    test_wrap_and_scroll();
    test_csi_cursor();
    test_csi_erase();
    test_ignored_sequences();
    test_dirty_band();
    test_full_refresh_flag();

    if (failures) {
        printf("%d check(s) failed\n", failures);
        return 1;
    }
    printf("all console/protocol checks passed\n");
    return 0;
}
