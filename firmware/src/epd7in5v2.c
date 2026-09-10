#include "epd7in5v2.h"

#include <string.h>

#include "hardware/gpio.h"
#include "hardware/spi.h"
#include "pico/stdlib.h"

#define PIN_MISO 4
#define PIN_CS   5
#define PIN_SCK  6
#define PIN_MOSI 7
#define PIN_DC   8
#define PIN_RST  9
#define PIN_BUSY 10

/* The spec allows 10 MHz; 4 MHz is the rate the panel was verified at over
 * jumper wires. */
#define EPD_SPI_HZ (4 * 1000 * 1000)

/* Rows pushed per epd_poll() call. 48 rows is 4800 bytes, ~9.6 ms of SPI at
 * 4 MHz, which keeps each poll well inside TinyUSB's servicing tolerance. */
#define TX_ROWS_PER_POLL 48

uint8_t epd_framebuffer[EPD_FRAME_BYTES];

/* Mirror of what the panel last displayed, used as the "old" plane so a
 * partial refresh only transitions pixels that actually changed. */
static uint8_t epd_old[EPD_FRAME_BYTES];

typedef enum {
    INIT_NONE,
    INIT_FULL,
    INIT_PARTIAL,
} init_mode_t;

typedef enum {
    ST_IDLE,
    ST_POWER_BUSY,
    ST_TX_OLD,
    ST_TX_NEW,
    ST_REFRESH_DELAY,
    ST_REFRESH_BUSY,
    ST_SLEEP_BUSY,
} state_t;

static init_mode_t current_init = INIT_NONE;
static state_t state = ST_IDLE;
static bool job_is_partial;
static uint16_t job_x_start, job_x_end;
static uint16_t job_y_start, job_y_end;
static uint16_t tx_row;
static absolute_time_t deadline;

static void cmd(uint8_t c) {
    gpio_put(PIN_DC, 0);
    gpio_put(PIN_CS, 0);
    spi_write_blocking(spi0, &c, 1);
    gpio_put(PIN_CS, 1);
}

static void data1(uint8_t d) {
    gpio_put(PIN_DC, 1);
    gpio_put(PIN_CS, 0);
    spi_write_blocking(spi0, &d, 1);
    gpio_put(PIN_CS, 1);
}

static void data(const uint8_t *buf, size_t len) {
    gpio_put(PIN_DC, 1);
    gpio_put(PIN_CS, 0);
    spi_write_blocking(spi0, buf, len);
    gpio_put(PIN_CS, 1);
}

/* The panel only updates BUSY after a 0x71 poll command. */
static bool panel_idle(void) {
    cmd(0x71);
    return gpio_get(PIN_BUSY) != 0;
}

static void panel_reset(void) {
    gpio_put(PIN_RST, 1);
    sleep_ms(20);
    gpio_put(PIN_RST, 0);
    sleep_ms(2);
    gpio_put(PIN_RST, 1);
    sleep_ms(20);
}

static void begin_init_full(void) {
    panel_reset();

    cmd(0x01);
    data1(0x07);
    data1(0x07);
    data1(0x3F);
    data1(0x3F);

    cmd(0x06);
    data1(0x17);
    data1(0x17);
    data1(0x28);
    data1(0x17);

    cmd(0x04);
    current_init = INIT_FULL;
}

static void finish_init_full(void) {
    cmd(0x00);
    data1(0x1F);

    cmd(0x61);
    data1(0x03);
    data1(0x20);
    data1(0x01);
    data1(0xE0);

    cmd(0x15);
    data1(0x00);

    cmd(0x50);
    data1(0x10);
    data1(0x07);

    cmd(0x60);
    data1(0x22);
}

static void begin_init_partial(void) {
    panel_reset();

    cmd(0x00);
    data1(0x1F);

    cmd(0x04);
    current_init = INIT_PARTIAL;
}

static void finish_init_partial(void) {
    /* Force the temperature index that selects the partial OTP waveform. */
    cmd(0xE0);
    data1(0x02);
    cmd(0xE5);
    data1(0x6E);

    cmd(0x50);
    data1(0xA9);
    data1(0x07);

    cmd(0x91);
    cmd(0x90);
    /* HRST/HRED are pixel columns and HRED is inclusive. Reduce to the last
     * pixel before splitting into bytes: upstream splits first and computes
     * x_end%256-1, which underflows whenever x_end is a multiple of 256. */
    uint16_t hrst = (uint16_t)(job_x_start * 8);
    uint16_t hred = (uint16_t)(job_x_end * 8 - 1);
    data1((uint8_t)(hrst >> 8));
    data1((uint8_t)(hrst & 0xFF));
    data1((uint8_t)(hred >> 8));
    data1((uint8_t)(hred & 0xFF));
    data1((uint8_t)(job_y_start >> 8));
    data1((uint8_t)(job_y_start & 0xFF));
    data1((uint8_t)((job_y_end - 1) >> 8));
    data1((uint8_t)((job_y_end - 1) & 0xFF));
    data1(0x01);
}

void epd_init_hardware(void) {
    spi_init(spi0, EPD_SPI_HZ);
    spi_set_format(spi0, 8, SPI_CPOL_0, SPI_CPHA_0, SPI_MSB_FIRST);
    gpio_set_function(PIN_SCK, GPIO_FUNC_SPI);
    gpio_set_function(PIN_MOSI, GPIO_FUNC_SPI);
    gpio_set_function(PIN_MISO, GPIO_FUNC_SPI);

    gpio_init(PIN_CS);
    gpio_set_dir(PIN_CS, GPIO_OUT);
    gpio_put(PIN_CS, 1);

    gpio_init(PIN_DC);
    gpio_set_dir(PIN_DC, GPIO_OUT);
    gpio_put(PIN_DC, 0);

    gpio_init(PIN_RST);
    gpio_set_dir(PIN_RST, GPIO_OUT);
    gpio_put(PIN_RST, 1);

    gpio_init(PIN_BUSY);
    gpio_set_dir(PIN_BUSY, GPIO_IN);
    gpio_pull_up(PIN_BUSY);

    memset(epd_framebuffer, 0x00, sizeof epd_framebuffer);
    memset(epd_old, 0x00, sizeof epd_old);
}

bool epd_is_busy(void) { return state != ST_IDLE; }

void epd_framebuffer_fill(bool black) {
    memset(epd_framebuffer, black ? 0xFF : 0x00, sizeof epd_framebuffer);
}

void epd_start_full_refresh(void) {
    if (state != ST_IDLE) return;
    job_is_partial = false;
    job_x_start = 0;
    job_x_end = EPD_ROW_BYTES;
    job_y_start = 0;
    job_y_end = EPD_HEIGHT;
    begin_init_full();
    deadline = make_timeout_time_ms(100);
    state = ST_POWER_BUSY;
}

void epd_start_partial_refresh(uint16_t x_byte_start, uint16_t x_byte_end,
                               uint16_t y_start, uint16_t y_end) {
    if (state != ST_IDLE) return;
    if (x_byte_end > EPD_ROW_BYTES) x_byte_end = EPD_ROW_BYTES;
    if (y_end > EPD_HEIGHT) y_end = EPD_HEIGHT;
    if (x_byte_start >= x_byte_end) return;
    if (y_start >= y_end) return;
    job_is_partial = true;
    job_x_start = x_byte_start;
    job_x_end = x_byte_end;
    job_y_start = y_start;
    job_y_end = y_end;
    begin_init_partial();
    deadline = make_timeout_time_ms(100);
    state = ST_POWER_BUSY;
}

void epd_start_clear(bool black) {
    epd_framebuffer_fill(black);
    epd_start_full_refresh();
}

void epd_start_sleep(void) {
    if (state != ST_IDLE) return;
    if (current_init == INIT_NONE) return;
    cmd(0x50);
    data1(0xF7);
    cmd(0x02);
    state = ST_SLEEP_BUSY;
}

static void start_data_phase(void) {
    tx_row = job_y_start;
    if (job_is_partial) {
        finish_init_partial();
        /* Partial mode writes both planes inside the window so the waveform
         * sees a true old-vs-new comparison. */
        cmd(0x10);
        state = ST_TX_OLD;
    } else {
        finish_init_full();
        cmd(0x10);
        state = ST_TX_OLD;
    }
}

static void tx_chunk(bool old_plane) {
    uint16_t end = tx_row + TX_ROWS_PER_POLL;
    if (end > job_y_end) end = job_y_end;
    size_t w = (size_t)(job_x_end - job_x_start);

    for (uint16_t y = tx_row; y < end; y++) {
        const uint8_t *src = (old_plane ? epd_old : epd_framebuffer) +
                             (size_t)y * EPD_ROW_BYTES + job_x_start;
        /* The panel planes are 1 = white. The full-refresh OTP waveform is the
         * exception: upstream EPD_7IN5_V2_Display feeds 0x13 inverted so every
         * pixel transitions, and the framebuffer is already in that (1 = black)
         * form. Everything else -- the 0x10 plane, and both planes under the
         * partial waveform -- must be inverted (verified on the panel: sending
         * 1 = black to a partial window renders it inverted). */
        bool invert = old_plane || job_is_partial;
        if (old_plane && !job_is_partial)
            src = epd_framebuffer + (size_t)y * EPD_ROW_BYTES + job_x_start;
        if (invert) {
            uint8_t inverted[EPD_ROW_BYTES];
            for (size_t i = 0; i < w; i++) inverted[i] = (uint8_t)~src[i];
            data(inverted, w);
        } else {
            data(src, w);
        }
    }
    tx_row = end;
}

void epd_poll(void) {
    switch (state) {
        case ST_IDLE:
            break;

        case ST_POWER_BUSY:
            if (absolute_time_diff_us(get_absolute_time(), deadline) > 0) break;
            if (panel_idle()) start_data_phase();
            break;

        case ST_TX_OLD:
            tx_chunk(true);
            if (tx_row >= job_y_end) {
                tx_row = job_y_start;
                cmd(0x13);
                state = ST_TX_NEW;
            }
            break;

        case ST_TX_NEW:
            tx_chunk(false);
            if (tx_row >= job_y_end) {
                cmd(0x12);
                deadline = make_timeout_time_ms(100);
                state = ST_REFRESH_DELAY;
            }
            break;

        case ST_REFRESH_DELAY:
            if (absolute_time_diff_us(get_absolute_time(), deadline) <= 0)
                state = ST_REFRESH_BUSY;
            break;

        case ST_REFRESH_BUSY:
            if (panel_idle()) {
                /* Only the refreshed window reached the panel, so the rest of
                 * epd_old must keep describing what is still displayed. */
                size_t w = (size_t)(job_x_end - job_x_start);
                for (uint16_t y = job_y_start; y < job_y_end; y++) {
                    size_t off = (size_t)y * EPD_ROW_BYTES + job_x_start;
                    memcpy(epd_old + off, epd_framebuffer + off, w);
                }
                state = ST_IDLE;
            }
            break;

        case ST_SLEEP_BUSY:
            if (panel_idle()) {
                cmd(0x07);
                data1(0xA5);
                current_init = INIT_NONE;
                state = ST_IDLE;
            }
            break;
    }
}
