#define _GNU_SOURCE
#include <errno.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>

typedef int (*generated_fn)(void);

static void emit_return(unsigned char *page, int value) {
    page[0] = 0xB8; /* MOV EAX, imm32 */
    page[1] = (unsigned char)(value & 0xff);
    page[2] = (unsigned char)((value >> 8) & 0xff);
    page[3] = (unsigned char)((value >> 16) & 0xff);
    page[4] = (unsigned char)((value >> 24) & 0xff);
    page[5] = 0xC3; /* RET */
    __builtin___clear_cache((char *)page, (char *)page + 6);
}

static int call_page(void *page) {
    union {
        void *object;
        generated_fn function;
    } entry = {.object = page};
    return entry.function();
}

int main(int argc, char **argv) {
    if (argc != 2 || argv[1][0] == '\0') {
        return 64;
    }
    long page_size = sysconf(_SC_PAGESIZE);
    if (page_size <= 0) {
        return 1;
    }

    unsigned char *page = mmap(NULL, (size_t)page_size, PROT_READ | PROT_WRITE,
                               MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (page == MAP_FAILED) {
        perror("mmap writable code page");
        return 1;
    }
    emit_return(page, 41);
    if (mprotect(page, (size_t)page_size, PROT_READ | PROT_EXEC) != 0) {
        perror("mprotect executable page");
        return 1;
    }
    int first = call_page(page);

    if (mprotect(page, (size_t)page_size, PROT_READ | PROT_WRITE) != 0) {
        perror("mprotect writable page");
        return 1;
    }
    emit_return(page, 42);
    if (mprotect(page, (size_t)page_size, PROT_READ | PROT_EXEC) != 0) {
        perror("mprotect re-executable page");
        return 1;
    }
    int second = call_page(page);

    unsigned char *nx_page = mmap(NULL, (size_t)page_size,
                                  PROT_READ | PROT_WRITE,
                                  MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (nx_page == MAP_FAILED) {
        perror("mmap NX probe page");
        return 1;
    }
    emit_return(nx_page, 7);
    struct rlimit no_core = {.rlim_cur = 0, .rlim_max = 0};
    (void)setrlimit(RLIMIT_CORE, &no_core);
    pid_t child = fork();
    if (child < 0) {
        perror("fork NX probe");
        return 1;
    }
    if (child == 0) {
        (void)call_page(nx_page);
        _exit(0);
    }
    int status = 0;
    if (waitpid(child, &status, 0) != child) {
        perror("waitpid NX probe");
        return 1;
    }
    if (!WIFSIGNALED(status) || WTERMSIG(status) != SIGSEGV || first != 41 ||
        second != 42) {
        fprintf(stderr, "unexpected result first=%d second=%d status=%d errno=%d\n",
                first, second, status, errno);
        return 1;
    }
    printf("OD_S5_E_RESULT nonce=%s first=%d second=%d nx=denied\n", argv[1],
           first, second);
    (void)munmap(nx_page, (size_t)page_size);
    (void)munmap(page, (size_t)page_size);
    return 0;
}
