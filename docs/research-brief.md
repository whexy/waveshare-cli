# Engineering Brief: RP2350 Firmware & macOS CLI for Waveshare 7.5" e-Paper (V2)

## 1. Hardware Pinout & Wiring (Author's Blog & Active Hardware)

**Primary Source:** [Whexy's Blog: Driving WaveShare E-Paper Display with a Raspberry Pi Pico in MicroPython](https://www.whexy.com/en/posts/pico_epaper) (`data/blog/pico_epaper.mdx`)

### Authoritative Pin Assignment

The board is a **Raspberry Pi Pico 2 (RP2350)** wired to a **Waveshare 7.5" e-Paper Display (V2, 800×480)**.

| Signal | Waveshare Hat Pin | Pico 2 GPIO | Physical Pin # | Pico Hardware Function | Notes |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **VCC** | VCC | 3V3 | Pin 36 | 3.3V Power Out | 3.3V logic & power |
| **GND** | GND | GND | Pin 38 | Ground | Common system ground |
| **DIN** | MOSI | **GP7** | Pin 10 | SPI0 TX | Hardware SPI0 MOSI |
| **CLK** | SCLK | **GP6** | Pin 9 | SPI0 SCK | Hardware SPI0 Clock |
| **CS** | CS / CSB | **GP5** | Pin 7 | SPI0 CSn (or GPIO) | Chip Select (Active LOW) |
| **DC** | DC | **GP8** | Pin 11 | GPIO | Data (HIGH) / Command (LOW) |
| **RST** | RST / RST_N | **GP9** | Pin 12 | GPIO | Panel Reset (Active LOW) |
| **BUSY**| BUSY / BUSY_N | **GP10**| Pin 14 | GPIO | Status: **0 = Busy, 1 = Idle** |
| *(MISO)*| *(MISO)* | **GP4** | Pin 6 | SPI0 RX | Unconnected (dummy assign for SPI init) |

*Active Hardware Note:* The target board currently runs **MicroPython 1.27 (RPI_PICO2)** with a Waveshare demo `main.py` using this exact pinout: `RST=GP9`, `DC=GP8`, `CS=GP5`, `BUSY=GP10`, `SCK=GP6`, `MOSI=GP7`, `MISO=GP4`, SPI0 at 4 MHz, Mode 0.

### Gotchas & Hardware Characteristics Identified
1. **BUSY Polling Polarity:** On the 7.5" V2 panel, the signal is active-low (`BUSY_N`). A value of `0` means the display driver IC is executing operations (waveform refresh, power sequence, or temperature read). The host MCU must wait while `BUSY == 0` until it transitions back to `1` (idle).
2. **Color Inversion / RAM Mapping:** The panel uses two RAM planes (DTM1 at command `0x10`, DTM2 at command `0x13`). For black & white updates via `0x13`, the display hardware expects `0 = White, 1 = Black`. Standard host image buffers often use `1 = White, 0 = Black`, requiring a bitwise inversion (`~byte`) before transmission.
3. **RAM Constraints in MicroPython vs C:** An 800×480 1-bit frame requires $800 \times 480 / 8 = 48,000\text{ bytes}$ (46.875 KB). In MicroPython, allocating contiguous 48 KB buffers frequently throws `MemoryError` due to heap fragmentation, necessitating 512-byte chunked streaming. On the RP2350 in C (520 KB SRAM), a full 48 KB buffer (or dual shadow buffers) fits comfortably in memory.
4. **Full Refresh Duration:** Full e-paper refresh takes 4–8 seconds and flashes heavily. Partial updates take ~0.3–0.5s with no full-screen flashing, but require specific register sequences.

---

## 2. Waveshare Official Driver (EPD_7in5_V2) Sequences & Register Details

**Primary Source:** `waveshareteam/e-Paper` repository (`RaspberryPi_JetsonNano/c/lib/e-Paper/EPD_7in5_V2.c` and `.h`). Note that upstream does not provide a separate `Pico/c` file for the 7.5" V2; the Jetson/RPi C driver is the canonical reference.

### Hardware Reset Sequence
```c
static void EPD_Reset(void)
{
    DEV_Digital_Write(EPD_RST_PIN, 1);
    DEV_Delay_ms(20);
    DEV_Digital_Write(EPD_RST_PIN, 0);
    DEV_Delay_ms(2);
    DEV_Digital_Write(EPD_RST_PIN, 1);
    DEV_Delay_ms(20);
}
```

### Busy Polling Routine
```c
static void EPD_WaitUntilIdle(void)
{
    Debug("e-Paper busy\r\n");
    do {
        DEV_Delay_ms(5);
    } while (!(DEV_Digital_Read(EPD_BUSY_PIN))); // Wait while pin == 0
    DEV_Delay_ms(5);
    Debug("e-Paper busy release\r\n");
}
```

### Full Refresh Initialization (`EPD_7IN5_V2_Init`)
```c
UBYTE EPD_7IN5_V2_Init(void)
{
    EPD_Reset();

    EPD_SendCommand(0x01); // POWER SETTING
    EPD_SendData(0x07);
    EPD_SendData(0x07);    // VGH=20V, VGL=-20V
    EPD_SendData(0x3f);    // VDH=15V
    EPD_SendData(0x3f);    // VDL=-15V

    // Enhanced display drive (Booster Soft Start)
    EPD_SendCommand(0x06); // Booster Soft Start
    EPD_SendData(0x17);
    EPD_SendData(0x17);
    EPD_SendData(0x28);
    EPD_SendData(0x17);

    EPD_SendCommand(0x04); // POWER ON
    DEV_Delay_ms(100);
    EPD_WaitUntilIdle();

    EPD_SendCommand(0x00); // PANEL SETTING
    EPD_SendData(0x1F);    // KW-3f, KWR-2F, BWROTP 0f, BWOTP 1f

    EPD_SendCommand(0x61); // TRES (Resolution Setting)
    EPD_SendData(0x03);    // Source 800 >> 8 (0x03)
    EPD_SendData(0x20);    // Source 800 & 0xFF (0x20)
    EPD_SendData(0x01);    // Gate 480 >> 8 (0x01)
    EPD_SendData(0xE0);    // Gate 480 & 0xFF (0xE0)

    EPD_SendCommand(0x15); // Dual SPI Mode
    EPD_SendData(0x00);

    EPD_SendCommand(0x50); // VCOM and Data Interval Setting (CDI)
    EPD_SendData(0x10);
    EPD_SendData(0x07);

    EPD_SendCommand(0x60); // TCON SETTING
    EPD_SendData(0x22);

    return 0;
}
```

### Fast Refresh Initialization (`EPD_7IN5_V2_Init_Fast`)
Uses internal OTP LUT selection by forcing the temperature register `0xE5` to `0x5A`:
```c
UBYTE EPD_7IN5_V2_Init_Fast(void)
{
    EPD_Reset();

    EPD_SendCommand(0x00); // PANEL SETTING
    EPD_SendData(0x1F);

    EPD_SendCommand(0x50); // CDI
    EPD_SendData(0x10);
    EPD_SendData(0x07);

    EPD_SendCommand(0x04); // POWER ON
    DEV_Delay_ms(100);
    EPD_WaitUntilIdle();

    EPD_SendCommand(0x06); // Booster Soft Start
    EPD_SendData(0x27);
    EPD_SendData(0x27);
    EPD_SendData(0x18);
    EPD_SendData(0x17);

    EPD_SendCommand(0xE0); // Cascade Setting (CCSET)
    EPD_SendData(0x02);    // TSFIX=1
    EPD_SendCommand(0xE5); // Force Temperature (TSSET)
    EPD_SendData(0x5A);    // Temperature index for fast OTP LUT

    return 0;
}
```

### Partial Refresh Initialization (`EPD_7IN5_V2_Init_Part`)
Forces temperature register `0xE5` to `0x6E` to select the partial-refresh OTP waveform:
```c
UBYTE EPD_7IN5_V2_Init_Part(void)
{
    EPD_Reset();

    EPD_SendCommand(0x00); // PANEL SETTING
    EPD_SendData(0x1F);

    EPD_SendCommand(0x04); // POWER ON
    DEV_Delay_ms(100);
    EPD_WaitUntilIdle();

    EPD_SendCommand(0xE0); // Cascade Setting
    EPD_SendData(0x02);
    EPD_SendCommand(0xE5); // Force Temperature
    EPD_SendData(0x6E);    // Temperature index for partial OTP LUT

    return 0;
}
```

### Clear Screen (`EPD_7IN5_V2_Clear`)
Loads `0xFF` into `0x10` (old data / white) and `0x00` into `0x13` (new data / white), then refreshes:
```c
void EPD_7IN5_V2_Clear(void)
{
    UWORD Width = (EPD_7IN5_V2_WIDTH % 8 == 0) ? (EPD_7IN5_V2_WIDTH / 8) : (EPD_7IN5_V2_WIDTH / 8 + 1); // 100 bytes
    UWORD Height = EPD_7IN5_V2_HEIGHT; // 480 lines
    UBYTE image[EPD_7IN5_V2_WIDTH / 8];

    EPD_SendCommand(0x10);
    for (UWORD i = 0; i < Width; i++) image[i] = 0xFF;
    for (UWORD i = 0; i < Height; i++) EPD_SendData2(image, Width);

    EPD_SendCommand(0x13);
    for (UWORD i = 0; i < Width; i++) image[i] = 0x00;
    for (UWORD i = 0; i < Height; i++) EPD_SendData2(image, Width);

    EPD_7IN5_V2_TurnOnDisplay();
}
```

### Full Frame Display (`EPD_7IN5_V2_Display`)
`blackimage` in host memory uses `0 = Black, 1 = White`. `0x10` receives the original data, and `0x13` receives the bit-inverted buffer:
```c
void EPD_7IN5_V2_Display(UBYTE *blackimage)
{
    UDOUBLE Width = (EPD_7IN5_V2_WIDTH % 8 == 0) ? (EPD_7IN5_V2_WIDTH / 8) : (EPD_7IN5_V2_WIDTH / 8 + 1);
    UDOUBLE Height = EPD_7IN5_V2_HEIGHT;

    // Send original image to RAM 0x10 (DTM1)
    EPD_SendCommand(0x10);
    for (UDOUBLE j = 0; j < Height; j++) {
        EPD_SendData2((UBYTE *)(blackimage + j * Width), Width);
    }

    // Invert buffer for RAM 0x13 (DTM2)
    EPD_SendCommand(0x13);
    for (UDOUBLE j = 0; j < Height; j++) {
        for (UDOUBLE i = 0; i < Width; i++) {
            blackimage[i + j * Width] = ~blackimage[i + j * Width];
        }
    }
    for (UDOUBLE j = 0; j < Height; j++) {
        EPD_SendData2((UBYTE *)(blackimage + j * Width), Width);
    }
    EPD_7IN5_V2_TurnOnDisplay();
}
```

### Partial Window Display (`EPD_7IN5_V2_Display_Part`)
```c
void EPD_7IN5_V2_Display_Part(UBYTE *blackimage, UDOUBLE x_start, UDOUBLE y_start, UDOUBLE x_end, UDOUBLE y_end)
{
    UDOUBLE Width = ((x_end - x_start) % 8 == 0) ? ((x_end - x_start) / 8) : ((x_end - x_start) / 8 + 1);
    UDOUBLE Height = y_end - y_start;

    EPD_SendCommand(0x50); // CDI: Border floating / keep VCOM steady
    EPD_SendData(0xA9);
    EPD_SendData(0x07);

    EPD_SendCommand(0x91); // PTIN: Enter partial mode
    EPD_SendCommand(0x90); // PTL: Partial window coordinates
    EPD_SendData(x_start / 256);
    EPD_SendData(x_start % 256); // HRST (Horizontal Start)
    EPD_SendData(x_end / 256);
    EPD_SendData(x_end % 256 - 1); // HRED (Horizontal End)
    EPD_SendData(y_start / 256);
    EPD_SendData(y_start % 256); // VRST (Vertical Start)
    EPD_SendData(y_end / 256);
    EPD_SendData(y_end % 256 - 1); // VRED (Vertical End)
    EPD_SendData(0x01);          // PT_SCAN: Scan inside and outside

    EPD_SendCommand(0x13); // DTM2
    for (UDOUBLE j = 0; j < Height; j++) {
        EPD_SendData2((UBYTE *)(blackimage + j * Width), Width);
    }
    EPD_7IN5_V2_TurnOnDisplay();
}
```

### Sleep Command
```c
void EPD_7IN5_V2_Sleep(void)
{
    EPD_SendCommand(0x50); // CDI
    EPD_SendData(0xF7);    // Floating border
    EPD_SendCommand(0x02); // Power OFF
    EPD_WaitUntilIdle();
    EPD_SendCommand(0x07); // Deep Sleep
    EPD_SendData(0xA5);    // Check code
}
```

### Framebuffer Memory Layout & Bit Ordering
* **Dimensions:** 800 (Horizontal) $\times$ 480 (Vertical) pixels.
* **Row byte pitch:** $800 / 8 = 100\text{ bytes}$ per scanline. Total frame: $100 \times 480 = 48,000\text{ bytes}$.
* **Bit ordering:** Big-Endian / MSB first across each byte:
  * Bit 7 corresponds to Pixel $(x = 0)$.
  * Bit 0 corresponds to Pixel $(x = 7)$.
* **Pixel value conventions:**
  * **Host / GUI representation:** `1 = White`, `0 = Black`.
  * **Hardware 0x13 (DTM2):** `0 = White`, `1 = Black` (hence the byte inversion in `Display()`).

---

## 3. Panel Specifications & Timing Constraints

**Primary Source:** `docs/7.5inch_e-Paper_V2_Specification.pdf` (Rev 2.0, 2019/06/28)

* **BUSY_N Pin Polarity (Note 1.5-4):** Active LOW.
  * `BUSY_N = 0`: Driver IC is active (outputting waveform, programming OTP, communicating with sensor). Host **must not** send SPI commands.
  * `BUSY_N = 1`: Driver IC is IDLE and ready for commands.
* **Reset Pulse Width (Table 3-1 & Note 1.5-3):** Active LOW (`RST_N`).
  * Reset pulse low duration: Min $2\text{ ms}$ ($10\,\mu\text{s}$ hardware threshold, Waveshare uses $2\text{ ms}$).
  * Post-reset recovery time: Min $20\text{ ms}$ before issuing the first SPI command.
* **SPI Bus Timing Limits (Section 3.3-3):**
  * Serial clock write cycle time $t_{\text{scycw}} \ge 100\text{ ns} \implies$ **Max SPI Clock = 10 MHz**.
  * SCL pulse width high/low: $t_{\text{shw}} \ge 35\text{ ns}$, $t_{\text{slw}} \ge 35\text{ ns}$.
  * Chip select setup/hold: $t_{\text{css}} \ge 100\text{ ns}$, $t_{\text{csh}} \ge 100\text{ ns}$.
  * Data setup/hold: $t_{\text{sds}} \ge 30\text{ ns}$, $t_{\text{sdh}} \ge 30\text{ ns}$.
  * DC setup/hold: $t_{\text{cds}} \ge 20\text{ ns}$, $t_{\text{cdh}} \ge 20\text{ ns}$.
  * Recommended operating frequency: **4 MHz** (or 2 MHz) for maximum signal integrity over jumper wires.
  * SPI Mode: **Mode 0** ($\text{CPOL}=0, \text{CPHA}=0$).
* **Refresh Times (Section 3.2):**
  * Full image update time at 25°C: Typical **4.0 s**, Max **8.0 s**.
  * Fast update time: ~1.5–2.0 s.
  * Partial update time: ~0.3–0.5 s.
* **Refresh Interval & Lifespan Constraints (Section 6.1, Note 6-2 & Section 9):**
  * **Minimum update interval:** Note 6-2 specifies that each update interval should be at least **180 seconds** in typical continuous deployments.
  * **24-Hour Refresh Rule:** If the panel is not refreshed for 24 hours, persistent image sticking ("ghosting") occurs. The module must undergo a full white refresh at least once every 24 hours.
  * **Partial Refresh Limit:** After multiple partial updates (Waveshare recommends full refresh every 5–10 partials), particulate charge builds up, causing background greying. A full refresh is mandatory to restore optical contrast (Contrast Ratio typ 8:1).
  * Storage/transport: Panel should always be left in a fully cleared (white) state before sleeping or power removal.

---

## 4. Pico 2 / RP2350 & pico-sdk (2.x) Essentials

### CMake Configuration (`CMakeLists.txt`)
Targeting the **RP2350** Cortex-M33 core requires `PICO_PLATFORM=rp2350-arm-s` and `PICO_BOARD=pico2`:

```cmake
cmake_minimum_required(VERSION 3.13)

# Include pico-sdk init
include(pico_sdk_import.cmake)

project(epaper_pico2 C CXX ASM)
set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)

# RP2350 target board
set(PICO_BOARD pico2)
set(PICO_PLATFORM rp2350-arm-s)

pico_sdk_init()

add_executable(epaper_firmware
    src/main.c
    src/epd_7in5_v2.c
    src/font.c
)

# Hardware and SDK libraries
target_link_libraries(epaper_firmware
    pico_stdlib
    hardware_spi
    hardware_gpio
    pico_unique_id
)

# Enable USB CDC serial, disable physical UART
pico_enable_stdio_usb(epaper_firmware 1)
pico_enable_stdio_uart(epaper_firmware 0)

# Generate UF2, BIN, HEX, and map files
pico_add_extra_outputs(epaper_firmware)
```

### Hardware SPI Setup
```c
#include "hardware/spi.h"
#include "hardware/gpio.h"

#define PIN_MISO  4  // GP4 (dummy for SPI0)
#define PIN_CS    5  // GP5
#define PIN_SCK   6  // GP6
#define PIN_MOSI  7  // GP7
#define PIN_DC    8  // GP8
#define PIN_RST   9  // GP9
#define PIN_BUSY 10  // GP10

void epd_hw_init(void) {
    // 4 MHz baudrate, Mode 0 (CPOL=0, CPHA=0)
    spi_init(spi0, 4000000);
    spi_set_format(spi0, 8, SPI_CPOL_0, SPI_CPHA_0, SPI_MSB_FIRST);

    gpio_set_function(PIN_SCK, GPIO_FUNC_SPI);
    gpio_set_function(PIN_MOSI, GPIO_FUNC_SPI);

    // Control GPIOs
    gpio_init(PIN_CS);
    gpio_set_dir(PIN_CS, GPIO_OUT);
    gpio_put(PIN_CS, 1);

    gpio_init(PIN_DC);
    gpio_set_dir(PIN_DC, GPIO_OUT);

    gpio_init(PIN_RST);
    gpio_set_dir(PIN_RST, GPIO_OUT);
    gpio_put(PIN_RST, 1);

    gpio_init(PIN_BUSY);
    gpio_set_dir(PIN_BUSY, GPIO_IN);
}
```

### USB CDC Protocol: Non-Blocking Byte Reads
To accept commands and streaming pixel/text data over USB CDC without stalling the refresh loops:

```c
#include "pico/stdlib.h"
#include "pico/stdio_usb.h"

// Non-blocking poll using getchar_timeout_us
void poll_usb_command(void) {
    int c = getchar_timeout_us(0); // 0us timeout = immediate return
    if (c != PICO_ERROR_TIMEOUT) {
        uint8_t byte = (uint8_t)c;
        process_incoming_byte(byte);
    }
}

// Or event-driven via characters available callback:
void on_chars_available(void *param) {
    int c;
    while ((c = getchar_timeout_us(0)) != PICO_ERROR_TIMEOUT) {
        process_incoming_byte((uint8_t)c);
    }
}

void setup_usb_rx(void) {
    stdio_init_all();
    stdio_set_chars_available_callback(on_chars_available, NULL);
}
```

### Picotool & Flashing Workflow (RP2350)
* **Flashing UF2:**
  ```bash
  picotool load -f build/epaper_firmware.uf2
  picotool reboot
  ```
* **Rebooting into BOOTSEL over USB:**
  ```bash
  picotool reboot -f -u
  ```
* **RP2350 USB Reset Interface:** Picotool reboots running RP2350 chips over USB via the `pico_stdio_usb` vendor interface. For this to operate:
  1. `pico_enable_stdio_usb(target 1)` must be set in `CMakeLists.txt`.
  2. Binary information must be compiled into the binary (default in pico-sdk).
  3. `picotool 2.x` sends a standard vendor control transfer to the USB CDC interface that causes the RP2350 bootrom to enter BOOTSEL mode without requiring a physical button press.

---

## 5. macOS Nix Development Toolchain

The Nix environment on `aarch64-darwin` provides the complete toolchain directly without global Homebrew state.

### Verified Nixpkgs Attributes (Nixpkgs 26.05 / unstable)
* **`pkgs.pico-sdk`:** Evaluates to `pico-sdk-2.2.0`. To include TinyUSB and other submodules required for USB CDC:
  ```nix
  (pkgs.pico-sdk.override { withSubmodules = true; })
  ```
* **`pkgs.picotool`:** Evaluates to `picotool-2.2.0-a4` (includes full RP2350 / ARMv8-M support).
* **`pkgs.gcc-arm-embedded`:** Evaluates to `gcc-arm-embedded-15.2.rel1` (`arm-none-eabi-gcc`).
* **`pkgs.cmake`:** Evaluates to `cmake-4.1.6`.
* **`pkgs.python3`:** Evaluates to `python3-3.13.15`.
* **`pkgs.mpremote`:** Evaluates to `mpremote-1.25.0` (for direct inspection/reset of existing MicroPython environment).

### Devshell Configuration (`devshell.nix`)
```nix
{ pkgs, ... }:
let
  picoSdk = pkgs.pico-sdk.override { withSubmodules = true; };
in
pkgs.mkShell {
  packages = [
    pkgs.cmake
    pkgs.ninja
    pkgs.gcc-arm-embedded
    pkgs.picotool
    picoSdk
    pkgs.python3
    pkgs.mpremote
    pkgs.ffmpeg
    pkgs.imagemagick
    pkgs.coreutils
    pkgs.jq
  ];

  PICO_SDK_PATH = "${picoSdk}/lib/pico-sdk";

  shellHook = ''
    export PICO_SDK_PATH="${picoSdk}/lib/pico-sdk"
    export PS1="(waveshare-pico2) $PS1"
  '';
}
```

---

## 6. Monospace Font & Text Rendering on 1-bpp Buffer

### Monospace Font Strategy
For an 800×480 monochrome display:
* An **8×16** font provides a clean terminal grid of **100 columns $\times$ 30 lines** ($100 \times 8 = 800$, $30 \times 16 = 480$).
* A **6×12** or **6×11** font provides **133 columns $\times$ 40 lines**.
* Font Representation: 8×16 bitmaps require 16 bytes per ASCII glyph ($95 \times 16 = 1,520\text{ bytes}$ total for printable ASCII 0x20–0x7E), which easily resides in flash memory.

### Recommended Sources & Licenses
1. **Linux Kernel `font_8x16` (`lib/fonts/font_8x16.c`):** Dual licensed GPLv2 / Public Domain. High legibility, standard IBM VGA console font glyphs.
2. **Waveshare `font16.c` (`lib/Fonts/font16.c`):** Permissive MIT-like Waveshare copyright. 16 pixels high, 11 pixels wide (padded to 2 bytes per row).
3. **Public Domain / CC0 Fonts:** `terminus-font` or X11 fixed fonts converted to C static tables.

### 8×16 Font Embedding & Blitting Implementation
In an 8×16 font, each byte is 1 row of 8 horizontal pixels. Because our display framebuffer has 100 bytes per row ($800 / 8$), a character at column `col` ($0 \le \text{col} < 100$) and line `line` ($0 \le \text{line} < 30$) is byte-aligned horizontally:

```c
#include <stdint.h>
#include <string.h>

#define FB_WIDTH_BYTES  100  // 800 pixels / 8
#define FB_HEIGHT_LINES 480
#define FONT_WIDTH        8
#define FONT_HEIGHT      16

// Embedded 8x16 font table (16 bytes per glyph, ASCII 0x20 to 0x7E)
extern const uint8_t font_8x16[96][16];

// Framebuffer: 1 = White (background), 0 = Black (text)
uint8_t framebuffer[FB_WIDTH_BYTES * FB_HEIGHT_LINES];

void epd_draw_char_8x16(uint8_t col, uint8_t line, char c, bool invert) {
    if (col >= 100 || line >= 30) return;
    if (c < 32 || c > 126) c = ' ';

    const uint8_t *glyph = font_8x16[c - 32];
    uint32_t start_y = line * FONT_HEIGHT;

    for (uint32_t y = 0; y < FONT_HEIGHT; y++) {
        uint32_t fb_idx = (start_y + y) * FB_WIDTH_BYTES + col;
        uint8_t glyph_byte = glyph[y];

        // glyph_byte: 1 = character stroke (black), 0 = background (white)
        // Framebuffer convention: 0 = Black, 1 = White
        uint8_t pixel_byte = ~glyph_byte; // stroke becomes 0, bg becomes 1

        if (invert) {
            pixel_byte = ~pixel_byte;
        }
        framebuffer[fb_idx] = pixel_byte;
    }
}

void epd_draw_string(uint8_t col, uint8_t line, const char *str) {
    while (*str && col < 100) {
        epd_draw_char_8x16(col++, line, *str++, false);
    }
}
```

---

## 7. Key Architectural Decisions & Risks

| Area | Decision / Characteristic | Risk & Mitigation |
| :--- | :--- | :--- |
| **Partial Refresh Support** | **Supported via internal OTP LUT.** `EPD_7IN5_V2_Init_Part()` sets `0xE0 -> 0x02` and `0xE5 -> 0x6E`. `EPD_7IN5_V2_Display_Part()` confines refresh to window bounding box (`0x90`, `0x91`). | **Ghosting accumulation:** Partial refresh leaves DC residual charge. After **5–10 partial refreshes** or upon terminal clear, trigger a full refresh (`EPD_7IN5_V2_Init` + `EPD_7IN5_V2_Clear`). |
| **Refresh Interval Timing** | Enforce a minimum interval between full refreshes. | Spec recommends 180s between full updates to preserve panel lifetime. Partial updates can happen faster (~1 Hz) for terminal output, but rate-limit serial streaming to prevent queue overflows. |
| **Active MicroPython Board** | Target Pico 2 is currently running MicroPython 1.27. | Flashing a `.uf2` overwrites MicroPython. Picotool over USB (`picotool reboot -f -u`) can force BOOTSEL mode without pressing hardware buttons if serial REPL is active, or use `mpremote` to soft-reset into bootloader. |
| **Protocol Design (CLI to Pico)** | Frame-based binary protocol over USB CDC with bounding box headers: `[CMD][X_START][Y_START][X_END][Y_END][LEN][DATA...]`. | Pure raw character streaming causes partial screen updates on every keystroke. CLI should buffer line updates or terminal damage rectangles before issuing partial update commands. |
| **Border Glitching during Partial** | Set CDI (`0x50`) to `0xA9, 0x07` during partial updates. | If border logic isn't floated during partial refresh, white/black frame border flashes violently. The V2 driver's partial sequence explicitly mitigates this. |
