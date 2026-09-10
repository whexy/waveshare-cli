cmake_minimum_required(VERSION 3.13)

set(PICO_BOARD pico2 CACHE STRING "Board type")
set(PICO_PLATFORM rp2350-arm-s CACHE STRING "Platform")

include(pico_sdk_import.cmake)

project(epaper_fw C CXX ASM)
set(CMAKE_C_STANDARD 11)

pico_sdk_init()

add_executable(epaper_fw
    src/main.c
    src/epd7in5v2.c
    src/console.c
    src/protocol.c
    src/usb_descriptors.c
)

target_include_directories(epaper_fw PRIVATE ${CMAKE_CURRENT_LIST_DIR}/src)

target_link_libraries(epaper_fw
    pico_stdlib
    pico_unique_id
    hardware_spi
    tinyusb_device
    tinyusb_board
)

# The protocol owns the CDC endpoint directly, so stdio must not also claim USB.
pico_enable_stdio_usb(epaper_fw 0)
pico_enable_stdio_uart(epaper_fw 0)

pico_add_extra_outputs(epaper_fw)
