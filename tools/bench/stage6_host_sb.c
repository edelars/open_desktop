#include <errno.h>
#include <inttypes.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#if !defined(__aarch64__)
#error "This probe requires an AArch64 host."
#endif

struct payload_cell {
    _Alignas(128) _Atomic uint32_t value;
};

struct round_cell {
    _Alignas(128) _Atomic uint64_t value;
};

struct shared_state {
    struct payload_cell payload[2];
    struct round_cell start;
    struct round_cell done[2];
    uint32_t observed[2];
    uint64_t rounds;
    bool fence;
};

struct worker_input {
    struct shared_state *state;
    unsigned cpu;
};

static void *worker(void *opaque) {
    struct worker_input *input = opaque;
    struct shared_state *state = input->state;
    unsigned cpu = input->cpu;
    _Atomic uint32_t *own = &state->payload[cpu].value;
    _Atomic uint32_t *other = &state->payload[1 - cpu].value;
    uint32_t one = 1;
    for (uint64_t round = 1; round <= state->rounds; ++round) {
        while (atomic_load_explicit(&state->start.value, memory_order_acquire) != round) {
        }
        uint32_t observed;
        if (state->fence) {
            __asm__ volatile("str %w[value], [%[own]]\n\t"
                             "dmb ish\n\t"
                             "ldr %w[observed], [%[other]]"
                             : [observed] "=&r" (observed)
                             : [value] "r" (one), [own] "r" (own), [other] "r" (other)
                             : "memory");
        } else {
            __asm__ volatile("str %w[value], [%[own]]\n\t"
                             "ldr %w[observed], [%[other]]"
                             : [observed] "=&r" (observed)
                             : [value] "r" (one), [own] "r" (own), [other] "r" (other)
                             : "memory");
        }
        state->observed[cpu] = observed;
        atomic_store_explicit(&state->done[cpu].value, round, memory_order_release);
    }
    return NULL;
}

int main(int argc, char **argv) {
    if (argc != 3 || (strcmp(argv[2], "relaxed") != 0 && strcmp(argv[2], "dmb") != 0)) {
        fprintf(stderr, "Usage: stage6_host_sb ROUNDS relaxed|dmb\n");
        return 2;
    }
    char *end;
    errno = 0;
    uintmax_t rounds = strtoumax(argv[1], &end, 10);
    if (errno != 0 || argv[1][0] < '0' || argv[1][0] > '9' || *end != '\0' ||
        rounds == 0 || rounds >= UINT64_MAX) {
        fprintf(stderr, "ROUNDS must be a positive integer smaller than UINT64_MAX\n");
        return 2;
    }

    static struct shared_state state;
    state.rounds = (uint64_t)rounds;
    state.fence = strcmp(argv[2], "dmb") == 0;
    pthread_t threads[2];
    struct worker_input inputs[2] = {{&state, 0}, {&state, 1}};
    for (unsigned cpu = 0; cpu < 2; ++cpu) {
        int error = pthread_create(&threads[cpu], NULL, worker, &inputs[cpu]);
        if (error != 0) {
            fprintf(stderr, "pthread_create: %s\n", strerror(error));
            return 3;
        }
    }

    uint64_t outcomes[2][2] = {{0}};
    for (uint64_t round = 1; round <= state.rounds; ++round) {
        atomic_store_explicit(&state.payload[0].value, 0, memory_order_relaxed);
        atomic_store_explicit(&state.payload[1].value, 0, memory_order_relaxed);
        atomic_store_explicit(&state.start.value, round, memory_order_release);
        for (unsigned cpu = 0; cpu < 2; ++cpu) {
            while (atomic_load_explicit(&state.done[cpu].value, memory_order_acquire) != round) {
            }
        }
        if (state.observed[0] > 1 || state.observed[1] > 1) {
            fprintf(stderr, "Unexpected SB outcome in round %" PRIu64 "\n", round);
            return 4;
        }
        outcomes[state.observed[0]][state.observed[1]]++;
    }
    for (unsigned cpu = 0; cpu < 2; ++cpu) {
        pthread_join(threads[cpu], NULL);
    }
    printf("mode=%s rounds=%" PRIu64 " outcomes=[%" PRIu64 ",%" PRIu64 ",%" PRIu64 ",%" PRIu64 "]\n",
           argv[2], state.rounds, outcomes[0][0], outcomes[0][1], outcomes[1][0], outcomes[1][1]);
    return 0;
}
