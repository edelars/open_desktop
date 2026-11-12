#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 3 || argv[1][0] == '\0') {
        return 64;
    }
    char *end = NULL;
    errno = 0;
    unsigned long mib = strtoul(argv[2], &end, 10);
    if (errno != 0 || end == argv[2] || *end != '\0' || mib < 16 || mib > 128) {
        return 64;
    }
    long page_size = sysconf(_SC_PAGESIZE);
    if (page_size <= 0) {
        return 1;
    }
    size_t bytes = (size_t)mib * 1024U * 1024U;
    size_t stride = (size_t)page_size;
    unsigned char *memory = malloc(bytes);
    if (memory == NULL) {
        perror("allocate bounded memory");
        return 1;
    }
    uint64_t checksum = 0;
    size_t pages = 0;
    for (size_t offset = 0; offset < bytes; offset += stride) {
        unsigned char value = (unsigned char)((pages * 37U + 11U) % 251U);
        memory[offset] = value;
        checksum += value;
        ++pages;
    }
    uint64_t verified = 0;
    for (size_t page = 0, offset = 0; offset < bytes;
         ++page, offset += stride) {
        unsigned char expected = (unsigned char)((page * 37U + 11U) % 251U);
        if (memory[offset] != expected) {
            fprintf(stderr, "memory verification failed page=%zu\n", page);
            free(memory);
            return 1;
        }
        verified += memory[offset];
    }
    if (verified != checksum) {
        fprintf(stderr, "memory checksum mismatch\n");
        free(memory);
        return 1;
    }
    printf("OD_S5_G_READY nonce=%s mib=%lu pages=%zu checksum=%llu hold=1\n",
           argv[1], mib, pages, (unsigned long long)verified);
    fflush(stdout);
    for (;;) {
        pause();
    }
}
