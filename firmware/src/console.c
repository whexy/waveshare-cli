#include "console.h"

#include <string.h>

#include "epd7in5v2.h"
#include "font8x16.h"

#define TAB_WIDTH 8
#define MAX_PARAMS 8

typedef enum {
    P_GROUND,
    P_ESC,
    P_CSI,
    P_OSC,
    P_OSC_ESC,
} parse_state_t;

static char cells[CONSOLE_ROWS][CONSOLE_COLS];
static bool row_dirty[CONSOLE_ROWS];
static uint8_t cur_row, cur_col;
static bool force_full;

static parse_state_t pstate;
static uint16_t params[MAX_PARAMS];
static uint8_t nparams;
static bool param_seen;
static bool csi_private;

static void mark(uint8_t row) {
    if (row < CONSOLE_ROWS) row_dirty[row] = true;
}

static void mark_all(void) {
    for (uint8_t r = 0; r < CONSOLE_ROWS; r++) row_dirty[r] = true;
}

static void clear_row(uint8_t row, uint8_t from, uint8_t to) {
    if (row >= CONSOLE_ROWS) return;
    for (uint8_t c = from; c < to && c < CONSOLE_COLS; c++) cells[row][c] = ' ';
    mark(row);
}

void console_reset(void) {
    for (uint8_t r = 0; r < CONSOLE_ROWS; r++)
        memset(cells[r], ' ', CONSOLE_COLS);
    cur_row = 0;
    cur_col = 0;
    pstate = P_GROUND;
    nparams = 0;
    param_seen = false;
    csi_private = false;
    force_full = true;
    mark_all();
}

static void scroll_up(void) {
    memmove(cells[0], cells[1], (size_t)(CONSOLE_ROWS - 1) * CONSOLE_COLS);
    memset(cells[CONSOLE_ROWS - 1], ' ', CONSOLE_COLS);
    mark_all();
    /* Every row shifted, so a partial waveform would ghost the old text. */
    force_full = true;
}

static void newline(void) {
    cur_col = 0;
    if (cur_row + 1 >= CONSOLE_ROWS)
        scroll_up();
    else
        cur_row++;
}

static void put_char(char ch) {
    if (cur_col >= CONSOLE_COLS) newline();
    cells[cur_row][cur_col] = ch;
    mark(cur_row);
    cur_col++;
}

static uint16_t param_or(uint8_t idx, uint16_t fallback) {
    if (idx >= nparams) return fallback;
    return params[idx] ? params[idx] : fallback;
}

static void csi_dispatch(uint8_t final) {
    if (csi_private) return; /* ESC [ ? ... h/l and friends are ignored. */

    switch (final) {
        case 'H':
        case 'f': {
            uint16_t row = param_or(0, 1);
            uint16_t col = param_or(1, 1);
            cur_row = (uint8_t)((row > CONSOLE_ROWS ? CONSOLE_ROWS : row) - 1);
            cur_col = (uint8_t)((col > CONSOLE_COLS ? CONSOLE_COLS : col) - 1);
            break;
        }
        case 'A': {
            uint16_t n = param_or(0, 1);
            cur_row = (uint8_t)(n >= cur_row ? 0 : cur_row - n);
            break;
        }
        case 'B': {
            uint16_t n = param_or(0, 1);
            uint16_t r = cur_row + n;
            cur_row = (uint8_t)(r >= CONSOLE_ROWS ? CONSOLE_ROWS - 1 : r);
            break;
        }
        case 'C': {
            uint16_t n = param_or(0, 1);
            uint16_t c = cur_col + n;
            cur_col = (uint8_t)(c >= CONSOLE_COLS ? CONSOLE_COLS - 1 : c);
            break;
        }
        case 'D': {
            uint16_t n = param_or(0, 1);
            cur_col = (uint8_t)(n >= cur_col ? 0 : cur_col - n);
            break;
        }
        case 'J': {
            uint16_t mode = nparams ? params[0] : 0;
            if (mode == 2) {
                for (uint8_t r = 0; r < CONSOLE_ROWS; r++)
                    memset(cells[r], ' ', CONSOLE_COLS);
                mark_all();
                force_full = true;
            } else if (mode == 0) {
                clear_row(cur_row, cur_col, CONSOLE_COLS);
                for (uint8_t r = (uint8_t)(cur_row + 1); r < CONSOLE_ROWS; r++)
                    clear_row(r, 0, CONSOLE_COLS);
            } else if (mode == 1) {
                clear_row(cur_row, 0, (uint8_t)(cur_col + 1));
                for (uint8_t r = 0; r < cur_row; r++) clear_row(r, 0, CONSOLE_COLS);
            }
            break;
        }
        case 'K': {
            uint16_t mode = nparams ? params[0] : 0;
            if (mode == 0)
                clear_row(cur_row, cur_col, CONSOLE_COLS);
            else if (mode == 1)
                clear_row(cur_row, 0, (uint8_t)(cur_col + 1));
            else if (mode == 2)
                clear_row(cur_row, 0, CONSOLE_COLS);
            break;
        }
        default:
            /* 'm' (SGR) and any other final byte are consumed and ignored. */
            break;
    }
}

static void handle_control(uint8_t c) {
    switch (c) {
        case '\n':
            newline();
            break;
        case '\r':
            cur_col = 0;
            break;
        case '\b':
            if (cur_col) cur_col--;
            break;
        case '\t': {
            uint8_t next = (uint8_t)((cur_col / TAB_WIDTH + 1) * TAB_WIDTH);
            cur_col = next >= CONSOLE_COLS ? (uint8_t)(CONSOLE_COLS - 1) : next;
            break;
        }
        default:
            break; /* BEL, DEL and other C0 codes are dropped. */
    }
}

void console_write(const uint8_t *buf, size_t len) {
    for (size_t i = 0; i < len; i++) {
        uint8_t c = buf[i];

        switch (pstate) {
            case P_GROUND:
                if (c == 0x1B) {
                    pstate = P_ESC;
                } else if (c >= 0x20 && c < 0x7F) {
                    put_char((char)c);
                } else {
                    handle_control(c);
                }
                break;

            case P_ESC:
                if (c == '[') {
                    pstate = P_CSI;
                    nparams = 0;
                    param_seen = false;
                    csi_private = false;
                    params[0] = 0;
                } else if (c == ']') {
                    pstate = P_OSC;
                } else if (c == 'c') {
                    console_reset();
                } else {
                    pstate = P_GROUND;
                }
                break;

            case P_CSI:
                if (c >= '0' && c <= '9') {
                    if (nparams < MAX_PARAMS) {
                        if (!param_seen) {
                            params[nparams] = 0;
                            param_seen = true;
                            nparams++;
                        }
                        params[nparams - 1] =
                            (uint16_t)(params[nparams - 1] * 10 + (c - '0'));
                    }
                } else if (c == ';') {
                    if (!param_seen && nparams < MAX_PARAMS) {
                        params[nparams] = 0;
                        nparams++;
                    }
                    param_seen = false;
                } else if (c == '?' || c == '>' || c == '<' || c == '=') {
                    csi_private = true;
                } else if (c >= 0x40 && c <= 0x7E) {
                    csi_dispatch(c);
                    pstate = P_GROUND;
                }
                break;

            case P_OSC:
                if (c == 0x07)
                    pstate = P_GROUND;
                else if (c == 0x1B)
                    pstate = P_OSC_ESC;
                break;

            case P_OSC_ESC:
                /* ST is ESC \; anything else resumes string collection. */
                pstate = (c == '\\') ? P_GROUND : P_OSC;
                break;
        }
    }
}

bool console_dirty(void) {
    for (uint8_t r = 0; r < CONSOLE_ROWS; r++)
        if (row_dirty[r]) return true;
    return false;
}

bool console_wants_full_refresh(void) { return force_full; }

static void render_row(uint8_t row) {
    for (uint8_t col = 0; col < CONSOLE_COLS; col++) {
        char ch = cells[row][col];
        const uint8_t *glyph = NULL;
        if (ch >= FONT_FIRST_CHAR && ch <= FONT_LAST_CHAR)
            glyph = font8x16[(uint8_t)ch - FONT_FIRST_CHAR];

        for (uint8_t y = 0; y < FONT_HEIGHT; y++) {
            size_t idx = ((size_t)row * FONT_HEIGHT + y) * EPD_ROW_BYTES + col;
            /* Framebuffer matches the panel wire format: a set bit is black,
             * and a set glyph bit is ink, so the glyph row copies straight in. */
            epd_framebuffer[idx] = glyph ? glyph[y] : 0x00;
        }
    }
}

bool console_render(uint16_t *y_start, uint16_t *y_end) {
    int first = -1, last = -1;
    for (uint8_t r = 0; r < CONSOLE_ROWS; r++) {
        if (!row_dirty[r]) continue;
        if (first < 0) first = r;
        last = r;
        render_row(r);
        row_dirty[r] = false;
    }
    force_full = false;
    if (first < 0) return false;

    *y_start = (uint16_t)(first * FONT_HEIGHT);
    *y_end = (uint16_t)((last + 1) * FONT_HEIGHT);
    return true;
}
