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

#define EPD_SPI_HZ (8 * 1000 * 1000)

/* Rows pushed per epd_poll() call. 48 rows is 4800 bytes, ~4.8 ms of SPI at
 * 8 MHz, which keeps each poll well inside TinyUSB's servicing tolerance. */
#define TX_ROWS_PER_POLL 48

/* Length of the single drive phase in the fast partial LUT, in frames of
 * ~20 ms (50 Hz PLL default). 20 frames measured 0.5 s of BUSY at close to
 * full-refresh contrast; docs/fast-refresh.md has the sweep. */
#define FAST_LUT_FRAMES 20

/* Each LUT register takes 6 groups of {level, T1, T2, T3, T4, repeat}. Only
 * the first group is used here; the rest stay zero. */
#define LUT_BYTES 42

uint8_t epd_framebuffer[EPD_FRAME_BYTES];

typedef enum {
    LUT_OTP,
    LUT_FAST,
} lut_mode_t;

typedef enum {
    ST_IDLE,
    ST_POF_BUSY,
    ST_PON_BUSY,
    ST_TX_OLD,
    ST_TX_NEW,
    ST_REFRESH_DELAY,
    ST_REFRESH_BUSY,
    ST_SLEEP_BUSY,
} state_t;

static state_t state = ST_IDLE;
static lut_mode_t lut_mode = LUT_OTP;
static bool panel_ready;
/* Partial refreshes diff against the panel's own old-data RAM, which holds
 * nothing meaningful until a refresh has filled it. Until then a partial would
 * drive pixels against garbage, so the first one is promoted to a full. */
static bool old_plane_valid;
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

static void wait_idle_blocking(uint32_t timeout_ms) {
    absolute_time_t limit = make_timeout_time_ms(timeout_ms);
    while (!panel_idle()) {
        if (absolute_time_diff_us(get_absolute_time(), limit) <= 0)
            return;
        sleep_ms(1);
    }
}

static void panel_reset(void) {
    gpio_put(PIN_RST, 1);
    sleep_ms(20);
    gpio_put(PIN_RST, 0);
    sleep_ms(2);
    gpio_put(PIN_RST, 1);
    sleep_ms(20);
}

/* Reset, configure and power the panel. Runs once at startup and again only
 * after a deep sleep, which is the one thing that drops the configuration;
 * every refresh afterwards reuses the powered panel. */
static void panel_boot(void) {
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
    data1(0x29);
    data1(0x07);

    cmd(0x60);
    data1(0x22);

    cmd(0x04);
    sleep_ms(100);
    wait_idle_blocking(2000);

    lut_mode = LUT_OTP;
    panel_ready = true;
    old_plane_valid = false;
}

static void write_lut(uint8_t reg, uint8_t level) {
    uint8_t lut[LUT_BYTES] = {level, FAST_LUT_FRAMES, 0, 0, 0, 1};
    cmd(reg);
    data(lut, sizeof lut);
}

/* Swap the OTP waveform for a single-phase one held in registers. The panel
 * spec does not document 0x20..0x25; the level encoding (2 bits per phase,
 * MSB first: 01 drives black, 10 drives white) was established on hardware.
 * Pixels whose old and new values match hit LUTWW/LUTKK, which drive nothing,
 * so nothing outside the changed glyphs flickers. */
static void enter_fast_lut_mode(void) {
    cmd(0x00);
    data1(0x3F);

    cmd(0x82);
    data1(0x26);

    /* BDV=11 leaves the border at whatever it already shows; N2OCP=1 makes the
     * panel copy new into old itself, which is why partials never send 0x10. */
    cmd(0x50);
    data1(0x39);
    data1(0x07);

    write_lut(0x20, 0x00); /* LUTC   VCOM stays at VCOM_DC */
    write_lut(0x21, 0x00); /* LUTWW  white -> white, no drive */
    write_lut(0x22, 0x80); /* LUTKW  black -> white */
    write_lut(0x23, 0x40); /* LUTWK  white -> black */
    write_lut(0x24, 0x00); /* LUTKK  black -> black, no drive */
    write_lut(0x25, 0x00); /* LUTBD  border */

    lut_mode = LUT_FAST;
}

static void set_window(void) {
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

    panel_boot();
}

bool epd_is_busy(void) { return state != ST_IDLE; }

void epd_framebuffer_fill(bool black) {
    memset(epd_framebuffer, black ? 0xFF : 0x00, sizeof epd_framebuffer);
}

void epd_start_full_refresh(void) {
    if (state != ST_IDLE)
        return;
    if (!panel_ready)
        panel_boot();

    job_is_partial = false;
    job_x_start = 0;
    job_x_end = EPD_ROW_BYTES;
    job_y_start = 0;
    job_y_end = EPD_HEIGHT;

    /* The OTP waveform only reaches full contrast if the panel is
     * power-cycled first; without this the frame comes out faint. */
    cmd(0x02);
    deadline = make_timeout_time_ms(10);
    state = ST_POF_BUSY;
}

bool epd_start_partial_refresh(uint16_t x_byte_start, uint16_t x_byte_end,
                               uint16_t y_start, uint16_t y_end) {
    if (state != ST_IDLE)
        return false;
    if (x_byte_end > EPD_ROW_BYTES)
        x_byte_end = EPD_ROW_BYTES;
    if (y_end > EPD_HEIGHT)
        y_end = EPD_HEIGHT;
    if (x_byte_start >= x_byte_end)
        return false;
    if (y_start >= y_end)
        return false;

    if (!panel_ready)
        panel_boot();
    if (!old_plane_valid) {
        epd_start_full_refresh();
        return true;
    }
    if (lut_mode != LUT_FAST)
        enter_fast_lut_mode();

    job_is_partial = true;
    job_x_start = x_byte_start;
    job_x_end = x_byte_end;
    job_y_start = y_start;
    job_y_end = y_end;

    set_window();
    tx_row = job_y_start;
    cmd(0x13);
    state = ST_TX_NEW;
    return false;
}

void epd_start_clear(bool black) {
    epd_framebuffer_fill(black);
    epd_start_full_refresh();
}

void epd_start_sleep(void) {
    if (state != ST_IDLE)
        return;
    if (!panel_ready)
        return;
    cmd(0x50);
    data1(0xF7);
    cmd(0x02);
    state = ST_SLEEP_BUSY;
}

/* Old plane for a full refresh: all white, so every black pixel of the new
 * frame is driven through a white->black transition. */
static void tx_old_chunk(void) {
    uint8_t row[EPD_ROW_BYTES];
    memset(row, 0xFF, sizeof row);

    uint16_t end = tx_row + TX_ROWS_PER_POLL;
    if (end > job_y_end)
        end = job_y_end;
    for (uint16_t y = tx_row; y < end; y++)
        data(row, EPD_ROW_BYTES);
    tx_row = end;
}

/* New plane, window-sized. The framebuffer stores 1 = black; both panel RAM
 * planes take 1 = white, so every row is inverted on the way out. */
static void tx_new_chunk(void) {
    uint16_t end = tx_row + TX_ROWS_PER_POLL;
    if (end > job_y_end)
        end = job_y_end;
    size_t w = (size_t)(job_x_end - job_x_start);

    uint8_t row[EPD_ROW_BYTES];
    for (uint16_t y = tx_row; y < end; y++) {
        const uint8_t *src =
            epd_framebuffer + (size_t)y * EPD_ROW_BYTES + job_x_start;
        for (size_t i = 0; i < w; i++)
            row[i] = (uint8_t)~src[i];
        data(row, w);
    }
    tx_row = end;
}

void epd_poll(void) {
    switch (state) {
    case ST_IDLE:
        break;

    case ST_POF_BUSY:
        if (absolute_time_diff_us(get_absolute_time(), deadline) > 0)
            break;
        if (panel_idle()) {
            cmd(0x00);
            data1(0x1F);
            cmd(0x50);
            data1(0x29);
            data1(0x07);
            lut_mode = LUT_OTP;

            cmd(0x04);
            deadline = make_timeout_time_ms(100);
            state = ST_PON_BUSY;
        }
        break;

    case ST_PON_BUSY:
        if (absolute_time_diff_us(get_absolute_time(), deadline) > 0)
            break;
        if (panel_idle()) {
            tx_row = job_y_start;
            cmd(0x10);
            state = ST_TX_OLD;
        }
        break;

    case ST_TX_OLD:
        tx_old_chunk();
        if (tx_row >= job_y_end) {
            tx_row = job_y_start;
            cmd(0x13);
            state = ST_TX_NEW;
        }
        break;

    case ST_TX_NEW:
        tx_new_chunk();
        if (tx_row >= job_y_end) {
            cmd(0x12);
            /* Settle before the first BUSY poll: the panel takes a moment
             * to assert BUSY after DRF, and reading it too early would end
             * the refresh immediately. 10 ms is what the probe scripts
             * verified; a partial is short enough that the difference
             * shows, a full one is not. */
            deadline = make_timeout_time_ms(job_is_partial ? 10 : 100);
            state = ST_REFRESH_DELAY;
        }
        break;

    case ST_REFRESH_DELAY:
        if (absolute_time_diff_us(get_absolute_time(), deadline) <= 0)
            state = ST_REFRESH_BUSY;
        break;

    case ST_REFRESH_BUSY:
        if (panel_idle()) {
            if (job_is_partial)
                cmd(0x92);
            old_plane_valid = true;
            state = ST_IDLE;
        }
        break;

    case ST_SLEEP_BUSY:
        if (panel_idle()) {
            cmd(0x07);
            data1(0xA5);
            panel_ready = false;
            state = ST_IDLE;
        }
        break;
    }
}
