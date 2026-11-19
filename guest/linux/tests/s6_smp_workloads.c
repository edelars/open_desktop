#define _GNU_SOURCE

#include <errno.h>
#include <inttypes.h>
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

enum {
    WORKERS = 2,
    COUNTER_ITERATIONS = 10000,
    CONDITION_ROUNDS = 128,
    EXCHANGE_ROUNDS = 64,
    PACKET_SIZE = 4096,
    MAPPING_ROUNDS = 32,
    PROCESS_ROUNDS = 16,
};

static const char *nonce = "invalid";
static const char *scenario = "setup";

static _Noreturn void fail(const char *operation, int error)
{
    fprintf(stderr, "OD_S6_22_FAIL nonce=%s case=%s operation=%s%s%s\n",
            nonce, scenario, operation, error ? ": " : "", error ? strerror(error) : "");
    exit(EXIT_FAILURE);
}

static void check_pthread(int error, const char *operation)
{
    if (error != 0)
        fail(operation, error);
}

static void require(int condition, const char *operation)
{
    if (!condition)
        fail(operation, 0);
}

static int on_cpu(int expected)
{
    unsigned current;
    // Check the Linux scheduler's CPU directly, including under x86 translation.
    return syscall(SYS_getcpu, &current, NULL, NULL) == 0 && current == (unsigned)expected;
}

static void pin(int cpu)
{
    cpu_set_t mask;
    CPU_ZERO(&mask);
    CPU_SET(cpu, &mask);
    if (sched_setaffinity(0, sizeof(mask), &mask) != 0)
        fail("sched_setaffinity", errno);
    require(on_cpu(cpu), "cpu-affinity");
}

static void rendezvous(pthread_barrier_t *barrier)
{
    int rc = pthread_barrier_wait(barrier);
    if (rc != PTHREAD_BARRIER_SERIAL_THREAD)
        check_pthread(rc, "pthread_barrier_wait");
}

struct worker {
    int cpu;
    unsigned index;
    void (*run)(unsigned, void *);
    void *state;
};

static void *run_worker(void *argument)
{
    struct worker *worker = argument;
    pin(worker->cpu);
    worker->run(worker->index, worker->state);
    require(on_cpu(worker->cpu), "cpu-affinity-after-work");
    return NULL;
}

static void run_workers(const int cpus[WORKERS], void (*run)(unsigned, void *), void *state)
{
    pthread_t threads[WORKERS];
    struct worker workers[WORKERS];
    for (unsigned i = 0; i < WORKERS; ++i) {
        workers[i] = (struct worker){.cpu = cpus[i], .index = i, .run = run, .state = state};
        check_pthread(pthread_create(&threads[i], NULL, run_worker, &workers[i]), "pthread_create");
    }
    for (unsigned i = 0; i < WORKERS; ++i)
        check_pthread(pthread_join(threads[i], NULL), "pthread_join");
}

struct counter_state {
    _Atomic uint64_t value;
    pthread_barrier_t start;
};

static void increment_counter(unsigned index, void *argument)
{
    (void)index;
    struct counter_state *state = argument;
    rendezvous(&state->start);
    for (unsigned i = 0; i < COUNTER_ITERATIONS; ++i)
        atomic_fetch_add_explicit(&state->value, 1, memory_order_relaxed);
}

static uint64_t counter(const int cpus[WORKERS])
{
    struct counter_state state = {.value = 0};
    require(atomic_is_lock_free(&state.value), "lock-free-counter");
    check_pthread(pthread_barrier_init(&state.start, NULL, WORKERS), "pthread_barrier_init");
    run_workers(cpus, increment_counter, &state);
    check_pthread(pthread_barrier_destroy(&state.start), "pthread_barrier_destroy");
    uint64_t value = atomic_load_explicit(&state.value, memory_order_relaxed);
    require(value == WORKERS * COUNTER_ITERATIONS, "counter-sum");
    return value;
}

struct condition_state {
    pthread_mutex_t mutex;
    pthread_cond_t changed;
    unsigned turn;
    unsigned value;
};

static void take_turn(unsigned index, void *argument)
{
    struct condition_state *state = argument;
    for (unsigned round = 0; round < CONDITION_ROUNDS; ++round) {
        check_pthread(pthread_mutex_lock(&state->mutex), "pthread_mutex_lock");
        while (state->turn != index)
            check_pthread(pthread_cond_wait(&state->changed, &state->mutex), "pthread_cond_wait");
        require(state->value == WORKERS * round + index, "condition-sequence");
        ++state->value;
        state->turn = 1 - index;
        check_pthread(pthread_cond_signal(&state->changed), "pthread_cond_signal");
        check_pthread(pthread_mutex_unlock(&state->mutex), "pthread_mutex_unlock");
    }
}

static uint64_t condition(const int cpus[WORKERS])
{
    struct condition_state state = {
        .mutex = PTHREAD_MUTEX_INITIALIZER,
        .changed = PTHREAD_COND_INITIALIZER,
    };
    run_workers(cpus, take_turn, &state);
    check_pthread(pthread_mutex_destroy(&state.mutex), "pthread_mutex_destroy");
    check_pthread(pthread_cond_destroy(&state.changed), "pthread_cond_destroy");
    require(state.value == WORKERS * CONDITION_ROUNDS && state.turn == 0, "condition-final-value");
    return state.value;
}

struct exchange_state {
    pthread_barrier_t barrier;
    unsigned char packets[WORKERS][PACKET_SIZE];
};

static unsigned char packet_byte(unsigned sender, unsigned round, unsigned offset)
{
    return (unsigned char)(sender * 193 + round * 17 + offset + (offset >> 8));
}

static uint64_t hash_byte(uint64_t hash, unsigned char byte)
{
    return (hash ^ byte) * UINT64_C(1099511628211);
}

static void exchange_packets(unsigned index, void *argument)
{
    struct exchange_state *state = argument;
    unsigned peer = 1 - index;
    for (unsigned round = 0; round < EXCHANGE_ROUNDS; ++round) {
        for (unsigned offset = 0; offset < PACKET_SIZE; ++offset)
            state->packets[index][offset] = packet_byte(index, round, offset);
        rendezvous(&state->barrier);

        uint64_t actual = UINT64_C(14695981039346656037);
        uint64_t expected = actual;
        for (unsigned offset = 0; offset < PACKET_SIZE; ++offset) {
            actual = hash_byte(actual, state->packets[peer][offset]);
            expected = hash_byte(expected, packet_byte(peer, round, offset));
        }
        require(actual == expected, "peer-packet-hash");
        rendezvous(&state->barrier);
    }
}

static uint64_t exchange(const int cpus[WORKERS])
{
    struct exchange_state state;
    check_pthread(pthread_barrier_init(&state.barrier, NULL, WORKERS), "pthread_barrier_init");
    run_workers(cpus, exchange_packets, &state);
    check_pthread(pthread_barrier_destroy(&state.barrier), "pthread_barrier_destroy");
    return WORKERS * EXCHANGE_ROUNDS;
}

struct mapping_state {
    pthread_barrier_t barrier;
    size_t page_size;
    volatile uint64_t *page;
};

static void replace_mapping(unsigned index, void *argument)
{
    struct mapping_state *state = argument;
    for (unsigned round = 0; round < MAPPING_ROUNDS; ++round) {
        int owner = index == round % WORKERS;
        uint64_t before = 2 * round + 1;
        uint64_t after = before + 1;
        if (owner) {
            void *page = mmap(NULL, state->page_size, PROT_READ | PROT_WRITE,
                              MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
            if (page == MAP_FAILED)
                fail("mmap", errno);
            state->page = page;
            *state->page = before;
        }
        rendezvous(&state->barrier);
        if (!owner)
            require(*state->page == before, "mapping-before-replacement");
        rendezvous(&state->barrier);

        if (owner) {
            void *base = (void *)state->page;
            uintptr_t address = (uintptr_t)base;
            if (mprotect(base, state->page_size, PROT_NONE) != 0)
                fail("mprotect-none", errno);
            /* Replace only the page that this worker owns and that the peer has just read. */
            void *page = mmap(base, state->page_size, PROT_READ | PROT_WRITE,
                              MAP_FIXED | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
            if (page == MAP_FAILED)
                fail("mmap-replacement", errno);
            require((uintptr_t)page == address, "replacement-address");
            state->page = page;
            require(*state->page == 0, "replacement-is-zero-filled");
            *state->page = after;
            if (mprotect(base, state->page_size, PROT_READ) != 0)
                fail("mprotect-read", errno);
        }
        rendezvous(&state->barrier);
        if (!owner)
            require(*state->page == after, "mapping-after-replacement");
        rendezvous(&state->barrier);
        if (owner && munmap((void *)state->page, state->page_size) != 0)
            fail("munmap", errno);
        rendezvous(&state->barrier);
    }
}

static uint64_t mapping(const int cpus[WORKERS])
{
    long page_size = sysconf(_SC_PAGESIZE);
    require(page_size >= (long)sizeof(uint64_t), "page-size");
    struct mapping_state state = {.page_size = (size_t)page_size};
    check_pthread(pthread_barrier_init(&state.barrier, NULL, WORKERS), "pthread_barrier_init");
    run_workers(cpus, replace_mapping, &state);
    check_pthread(pthread_barrier_destroy(&state.barrier), "pthread_barrier_destroy");
    return MAPPING_ROUNDS;
}

static uint64_t processes(const int cpus[WORKERS])
{
    cpu_set_t original;
    if (sched_getaffinity(0, sizeof(original), &original) != 0)
        fail("sched_getaffinity", errno);
    for (unsigned round = 0; round < PROCESS_ROUNDS; ++round) {
        pid_t children[WORKERS];
        for (unsigned i = 0; i < WORKERS; ++i) {
            pin(cpus[i]);
            children[i] = fork();
            if (children[i] < 0)
                fail("fork", errno);
            if (children[i] == 0)
                _exit(on_cpu(cpus[i]) ? 0 : 1);
        }
        for (unsigned i = 0; i < WORKERS; ++i) {
            int status;
            pid_t waited;
            do {
                waited = waitpid(children[i], &status, 0);
            } while (waited < 0 && errno == EINTR);
            if (waited < 0)
                fail("waitpid", errno);
            require(WIFEXITED(status) && WEXITSTATUS(status) == 0, "child-affinity-and-exit");
        }
    }
    if (sched_setaffinity(0, sizeof(original), &original) != 0)
        fail("restore-affinity", errno);
    return WORKERS * PROCESS_ROUNDS;
}

int main(int argc, char **argv)
{
    if (argc != 2 || !argv[1][0] || strlen(argv[1]) > 128 ||
        argv[1][strspn(argv[1], "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-")])
        fail("nonce", 0);
    nonce = argv[1];
    cpu_set_t mask;
    if (sched_getaffinity(0, sizeof(mask), &mask) != 0)
        fail("sched_getaffinity", errno);
    int cpus[WORKERS];
    unsigned selected = 0;
    for (int cpu = 0; cpu < CPU_SETSIZE && selected < WORKERS; ++cpu)
        if (CPU_ISSET(cpu, &mask))
            cpus[selected++] = cpu;
    require(selected == WORKERS, "requires-two-allowed-cpus");

    const struct {
        const char *name;
        uint64_t (*run)(const int cpus[WORKERS]);
    } cases[] = {
        {"counter", counter}, {"condition", condition}, {"exchange", exchange},
        {"mapping", mapping}, {"processes", processes},
    };
    for (size_t i = 0; i < sizeof(cases) / sizeof(cases[0]); ++i) {
        scenario = cases[i].name;
        uint64_t checked = cases[i].run(cpus);
        if (printf("OD_S6_22_PASS nonce=%s case=%s cpus=%d,%d checked=%" PRIu64 "\n",
                   nonce, scenario, cpus[0], cpus[1], checked) < 0 || fflush(stdout) == EOF)
            fail("write-result", errno);
    }
    return EXIT_SUCCESS;
}
