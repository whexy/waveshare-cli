/* Host-side tests for protocol.c. Built and run by firmware/test/run.sh with
 * the native compiler. */
#include <stdio.h>
#include <string.h>

#include "protocol.h"

static int failures;

#define CHECK(cond)                                                \
    do {                                                           \
        if (!(cond)) {                                             \
            printf("FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond); \
            failures++;                                            \
        }                                                          \
    } while (0)

static void test_crc(void) {
    /* CRC-16/CCITT-FALSE check value for "123456789" is 0x29B1. */
    CHECK(proto_crc16((const uint8_t *)"123456789", 9) == 0x29B1);
    CHECK(proto_crc16((const uint8_t *)"", 0) == 0xFFFF);
    CHECK(proto_crc16((const uint8_t *)"A", 1) == 0xB915);
}

int main(void) {
    test_crc();
    if (failures) {
        printf("%d check(s) failed\n", failures);
        return 1;
    }
    printf("all protocol checks passed\n");
    return 0;
}
