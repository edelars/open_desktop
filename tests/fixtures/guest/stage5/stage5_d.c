#include <stdio.h>

unsigned stage5_sum(void);

int main(int argc, char **argv) {
    if (argc != 2 || argv[1][0] == '\0') {
        return 64;
    }

    unsigned sum = stage5_sum();

    if (sum != 5050) {
        return 1;
    }
    printf("OD_S5_D_RESULT nonce=%s sum=%u\n", argv[1], sum);
    return 0;
}
