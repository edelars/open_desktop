/*
 * S4-34a: minimal KVM host runner (single file, plain C11).
 *
 * Creates a VM through /dev/kvm, maps 2 MiB of anonymous guest RAM at GPA 0,
 * loads a raw freestanding image at GPA 0x1000, builds the guest's own GDT and
 * identity page tables (PML4 0x2000, PDPT 0x3000, PD 0x4000, one 2 MiB leaf),
 * enters Long64/CPL0 at RIP 0x1000 and runs KVM_RUN. UART bytes are collected
 * from I/O exits on port 0x3f8 (write only). Execution is VT-x of the host CPU;
 * no software MMU/CPU is involved. No irqchip is created: the guest runs
 * without interrupts and HLT is reported to userspace.
 *
 * Output: one JSON line on stdout. Diagnostics go to stderr.
 * run_count counts KVM_RUN returns that carried a real exit; intr_count counts
 * EINTR / KVM_EXIT_INTR returns caused by the SIGALRM watchdog. The budget limit
 * applies to run_count + intr_count.
 *
 * Build: gcc -O2 -Wall -Wextra -Werror -std=c11 -o kvm_runner kvm_runner.c
 * Usage: kvm_runner <image.bin> [--budget N] [--deadline-ms MS]
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/kvm.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/time.h>
#include <sys/utsname.h>
#include <time.h>
#include <unistd.h>

#define GUEST_RAM_SIZE (2u * 1024u * 1024u)
#define IMAGE_GPA 0x1000u
#define IMAGE_MAX 0x1000u
#define PML4_GPA 0x2000u
#define PDPT_GPA 0x3000u
#define PD_GPA 0x4000u
#define GDT_GPA 0x5000u
#define STACK_TOP 0x8000u
#define UART_PORT 0x3f8u
#define MAX_UART 4096u
#define MAX_EXITS 256u
#define CPUID_MAX_ENT 256u

#define CR0_VALUE 0x80000033u /* PG | NE | ET | MP | PE */
#define CR4_VALUE 0x20u       /* PAE */
#define EFER_VALUE 0x500u     /* LME | LMA */

struct exit_rec {
    const char *kind;
    int has_io;
    unsigned port;
    unsigned size;
    unsigned dir_out;
    uint64_t data;
};

static struct exit_rec exits[MAX_EXITS];
static unsigned exit_count;
static unsigned exit_dropped;
static unsigned char uart[MAX_UART];
static unsigned uart_len;

static void on_alarm(int sig)
{
    (void)sig;
}

static void add_exit(const char *kind)
{
    if (exit_count < MAX_EXITS) {
        memset(&exits[exit_count], 0, sizeof(exits[0]));
        exits[exit_count].kind = kind;
        exit_count++;
    } else {
        exit_dropped++;
    }
}

static void add_io_exit(unsigned port, unsigned size, unsigned dir_out, uint64_t data)
{
    if (exit_count < MAX_EXITS) {
        struct exit_rec *e = &exits[exit_count++];
        e->kind = "io";
        e->has_io = 1;
        e->port = port;
        e->size = size;
        e->dir_out = dir_out;
        e->data = data;
    } else {
        exit_dropped++;
    }
}

static void json_string(const char *s)
{
    putchar('"');
    for (; *s; s++) {
        unsigned char c = (unsigned char)*s;
        if (c == '"' || c == '\\') {
            printf("\\%c", c);
        } else if (c == '\n') {
            fputs("\\n", stdout);
        } else if (c == '\r') {
            fputs("\\r", stdout);
        } else if (c == '\t') {
            fputs("\\t", stdout);
        } else if (c < 0x20 || c >= 0x7f) {
            printf("\\u%04x", c);
        } else {
            putchar(c);
        }
    }
    putchar('"');
}

static void host_identity(char *cpu_model, size_t cpu_len, int *vmx)
{
    FILE *f = fopen("/proc/cpuinfo", "r");
    char line[1024];
    int have_model = 0;
    cpu_model[0] = '\0';
    *vmx = 0;
    if (!f)
        return;
    while (fgets(line, sizeof(line), f)) {
        if (!have_model && strncmp(line, "model name", 10) == 0) {
            char *colon = strchr(line, ':');
            if (colon) {
                colon++;
                while (*colon == ' ' || *colon == '\t')
                    colon++;
                colon[strcspn(colon, "\r\n")] = '\0';
                snprintf(cpu_model, cpu_len, "%s", colon);
                have_model = 1;
            }
        } else if (strncmp(line, "flags", 5) == 0) {
            if (strstr(line, " vmx") != NULL)
                *vmx = 1;
        }
    }
    fclose(f);
}

static void set_seg(struct kvm_segment *s, uint16_t sel, uint8_t type, int code64)
{
    memset(s, 0, sizeof(*s));
    s->base = 0;
    s->limit = 0xffffffffu;
    s->selector = sel;
    s->type = type;
    s->present = 1;
    s->dpl = 0;
    s->db = code64 ? 0 : 1;
    s->s = 1;
    s->l = code64 ? 1 : 0;
    s->g = 1;
    s->avl = 0;
}

struct entry_state {
    uint64_t rip, rsp, rflags, cr0, cr3, cr4, efer;
    unsigned cs_selector, cs_l, cs_db, cs_dpl, ss_selector;
};

static void print_json(const char *exit_kind, const char *error, const struct entry_state *en,
                       int kvm_api, unsigned run_count, unsigned intr_count,
                       const char *stop_reason)
{
    struct utsname un;
    char cpu_model[256];
    char uname_str[512];
    char text[MAX_UART + 1];
    int vmx;
    unsigned i;

    host_identity(cpu_model, sizeof(cpu_model), &vmx);
    memset(&un, 0, sizeof(un));
    un.nodename[0] = '\0';
    if (uname(&un) == 0)
        snprintf(uname_str, sizeof(uname_str), "%s %s %s %s %s", un.sysname, un.nodename,
                 un.release, un.version, un.machine);
    else
        snprintf(uname_str, sizeof(uname_str), "unknown");

    memcpy(text, uart, uart_len);
    text[uart_len] = '\0';

    printf("{\"exit\":\"%s\",\"uart_hex\":\"", exit_kind);
    for (i = 0; i < uart_len; i++)
        printf("%02x", uart[i]);
    printf("\",\"uart_text\":");
    json_string(text);
    printf(",\"exits\":[");
    for (i = 0; i < exit_count; i++) {
        const struct exit_rec *e = &exits[i];
        if (i)
            putchar(',');
        printf("{\"kind\":\"%s\"", e->kind);
        if (e->has_io)
            printf(",\"port\":\"0x%x\",\"size\":%u,\"dir\":\"%s\",\"data\":\"0x%llx\"", e->port,
                   e->size, e->dir_out ? "out" : "in", (unsigned long long)e->data);
        putchar('}');
    }
    printf("],\"exits_dropped\":%u", exit_dropped);
    if (en)
        printf(",\"entry\":{\"rip\":%llu,\"rsp\":%llu,\"rflags\":%llu,\"cr0\":%llu,\"cr3\":%llu,"
               "\"cr4\":%llu,\"efer\":%llu,\"cs_selector\":%u,\"cs_l\":%u,\"cs_db\":%u,"
               "\"cs_dpl\":%u,\"ss_selector\":%u}",
               (unsigned long long)en->rip, (unsigned long long)en->rsp,
               (unsigned long long)en->rflags, (unsigned long long)en->cr0,
               (unsigned long long)en->cr3, (unsigned long long)en->cr4,
               (unsigned long long)en->efer, en->cs_selector, en->cs_l, en->cs_db, en->cs_dpl,
               en->ss_selector);
    else
        printf(",\"entry\":null");
    printf(",\"kvm_api\":%d,\"run_count\":%u,\"intr_count\":%u,\"stop_reason\":", kvm_api,
           run_count, intr_count);
    json_string(stop_reason);
    if (error) {
        printf(",\"error\":");
        json_string(error);
    }
    printf(",\"hostname\":");
    json_string(un.nodename);
    printf(",\"uname\":");
    json_string(uname_str);
    printf(",\"cpu_model\":");
    json_string(cpu_model);
    printf(",\"vmx\":%s}\n", vmx ? "true" : "false");
}

static void fail(const char *exit_kind, const char *what, const struct entry_state *en,
                 int kvm_api, unsigned runs)
{
    char msg[256];
    snprintf(msg, sizeof(msg), "%s: %s", what, strerror(errno));
    print_json(exit_kind, msg, en, kvm_api, runs, 0, "setup_error");
}

int main(int argc, char **argv)
{
    const char *image_path = NULL;
    unsigned budget = 1000;
    unsigned deadline_ms = 2000;
    int kvm = -1, vm = -1, vcpu = -1;
    int api = 0;
    size_t run_size;
    struct kvm_run *run = NULL;
    uint8_t *ram;
    struct kvm_userspace_memory_region region;
    struct kvm_sregs sregs;
    struct kvm_regs regs;
    struct entry_state en;
    unsigned run_count = 0; /* KVM_RUN returns that carried a real exit */
    unsigned intr_count = 0; /* KVM_RUN returns with EINTR / KVM_EXIT_INTR (SIGALRM) */
    const char *exit_kind = "budget";
    const char *stop_reason = "run_budget";
    struct timespec t0, t1;
    struct sigaction sa;
    struct itimerval tv;
    FILE *img;
    long img_len;
    int i;

    for (i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--budget") == 0 && i + 1 < argc)
            budget = (unsigned)strtoul(argv[++i], NULL, 10);
        else if (strcmp(argv[i], "--deadline-ms") == 0 && i + 1 < argc)
            deadline_ms = (unsigned)strtoul(argv[++i], NULL, 10);
        else if (!image_path)
            image_path = argv[i];
        else {
            fprintf(stderr, "usage: %s <image.bin> [--budget N] [--deadline-ms MS]\n", argv[0]);
            return 2;
        }
    }
    if (!image_path) {
        fprintf(stderr, "usage: %s <image.bin> [--budget N] [--deadline-ms MS]\n", argv[0]);
        return 2;
    }

    memset(&en, 0, sizeof(en));

    img = fopen(image_path, "rb");
    if (!img || fseek(img, 0, SEEK_END) != 0 || (img_len = ftell(img)) < 0 ||
        fseek(img, 0, SEEK_SET) != 0) {
        fail("internal", "open image", NULL, 0, 0);
        return 2;
    }
    if (img_len == 0 || (unsigned long)img_len > IMAGE_MAX) {
        errno = EFBIG;
        fail("internal", "image size must be 1..0x1000 bytes", NULL, 0, 0);
        return 2;
    }

    kvm = open("/dev/kvm", O_RDWR | O_CLOEXEC);
    if (kvm < 0) {
        fail("internal", "open /dev/kvm", NULL, 0, 0);
        return 2;
    }
    api = ioctl(kvm, KVM_GET_API_VERSION, 0);
    if (api != 12) {
        errno = ENOSYS;
        fail("internal", "unexpected KVM_GET_API_VERSION", NULL, api, 0);
        return 2;
    }
    vm = ioctl(kvm, KVM_CREATE_VM, 0);
    if (vm < 0) {
        fail("internal", "KVM_CREATE_VM", NULL, api, 0);
        return 2;
    }

    ram = mmap(NULL, GUEST_RAM_SIZE, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (ram == MAP_FAILED) {
        fail("internal", "mmap guest ram", NULL, api, 0);
        return 2;
    }
    memset(ram, 0, GUEST_RAM_SIZE);

    memset(&region, 0, sizeof(region));
    region.slot = 0;
    region.guest_phys_addr = 0;
    region.memory_size = GUEST_RAM_SIZE;
    region.userspace_addr = (uintptr_t)ram;
    if (ioctl(vm, KVM_SET_USER_MEMORY_REGION, &region) < 0) {
        fail("internal", "KVM_SET_USER_MEMORY_REGION", NULL, api, 0);
        return 2;
    }

    if (fread(ram + IMAGE_GPA, 1, (size_t)img_len, img) != (size_t)img_len) {
        errno = EIO;
        fail("internal", "read image", NULL, api, 0);
        return 2;
    }
    fclose(img);

    /* Guest page tables: identity map of the first 2 MiB with one PS leaf. */
    {
        uint64_t *pml4 = (uint64_t *)(ram + PML4_GPA);
        uint64_t *pdpt = (uint64_t *)(ram + PDPT_GPA);
        uint64_t *pd = (uint64_t *)(ram + PD_GPA);
        pml4[0] = PDPT_GPA | 0x3;  /* P | RW */
        pdpt[0] = PD_GPA | 0x3;    /* P | RW */
        pd[0] = 0x0 | 0x83;        /* P | RW | PS: 0..2 MiB */
    }
    /* Guest GDT: null, 64-bit code (0x08), data (0x10). */
    {
        uint64_t *gdt = (uint64_t *)(ram + GDT_GPA);
        gdt[0] = 0;
        gdt[1] = 0x00af9b000000ffffull;
        gdt[2] = 0x00cf93000000ffffull;
    }

    vcpu = ioctl(vm, KVM_CREATE_VCPU, 0);
    if (vcpu < 0) {
        fail("internal", "KVM_CREATE_VCPU", NULL, api, 0);
        return 2;
    }
    run_size = (size_t)ioctl(kvm, KVM_GET_VCPU_MMAP_SIZE, 0);
    run = mmap(NULL, run_size, PROT_READ | PROT_WRITE, MAP_SHARED, vcpu, 0);
    if (run == MAP_FAILED) {
        fail("internal", "mmap kvm_run", NULL, api, 0);
        return 2;
    }

    /* Without a guest CPUID KVM rejects EFER.LME; expose the host-supported set.
     * Start with 128 entries and grow on E2BIG up to CPUID_MAX_ENT. */
    {
        struct kvm_cpuid2 *cpuid = NULL;
        unsigned nent = 128;
        int got = 0;
        while (!got) {
            size_t sz = sizeof(*cpuid) + nent * sizeof(struct kvm_cpuid_entry2);
            free(cpuid);
            cpuid = calloc(1, sz);
            if (!cpuid) {
                fail("internal", "alloc cpuid", NULL, api, 0);
                return 2;
            }
            cpuid->nent = nent;
            if (ioctl(kvm, KVM_GET_SUPPORTED_CPUID, cpuid) == 0) {
                got = 1;
            } else if (errno == E2BIG && nent < CPUID_MAX_ENT) {
                nent = nent * 2 > CPUID_MAX_ENT ? CPUID_MAX_ENT : nent * 2;
            } else {
                fail("internal", "KVM_GET_SUPPORTED_CPUID", NULL, api, 0);
                return 2;
            }
        }
        if (ioctl(vcpu, KVM_SET_CPUID2, cpuid) < 0) {
            fail("internal", "KVM_SET_CPUID2", NULL, api, 0);
            return 2;
        }
        free(cpuid);
    }

    if (ioctl(vcpu, KVM_GET_SREGS, &sregs) < 0) {
        fail("internal", "KVM_GET_SREGS", NULL, api, 0);
        return 2;
    }
    set_seg(&sregs.cs, 0x08, 0xB, 1);
    set_seg(&sregs.ds, 0x10, 0x3, 0);
    sregs.es = sregs.ds;
    sregs.fs = sregs.ds;
    sregs.gs = sregs.ds;
    sregs.ss = sregs.ds;
    sregs.gdt.base = GDT_GPA;
    sregs.gdt.limit = 3 * 8 - 1;
    sregs.cr0 = CR0_VALUE;
    sregs.cr3 = PML4_GPA;
    sregs.cr4 = CR4_VALUE;
    sregs.efer = EFER_VALUE;
    if (ioctl(vcpu, KVM_SET_SREGS, &sregs) < 0) {
        fail("internal", "KVM_SET_SREGS", NULL, api, 0);
        return 2;
    }
    memset(&regs, 0, sizeof(regs));
    regs.rip = IMAGE_GPA;
    regs.rsp = STACK_TOP;
    regs.rflags = 0x2;
    if (ioctl(vcpu, KVM_SET_REGS, &regs) < 0) {
        fail("internal", "KVM_SET_REGS", NULL, api, 0);
        return 2;
    }

    /* Report the state KVM actually holds before the first run. */
    if (ioctl(vcpu, KVM_GET_SREGS, &sregs) < 0 || ioctl(vcpu, KVM_GET_REGS, &regs) < 0) {
        fail("internal", "read back entry state", NULL, api, 0);
        return 2;
    }
    en.rip = regs.rip;
    en.rsp = regs.rsp;
    en.rflags = regs.rflags;
    en.cr0 = sregs.cr0;
    en.cr3 = sregs.cr3;
    en.cr4 = sregs.cr4;
    en.efer = sregs.efer;
    en.cs_selector = sregs.cs.selector;
    en.cs_l = sregs.cs.l;
    en.cs_db = sregs.cs.db;
    en.cs_dpl = sregs.cs.dpl;
    en.ss_selector = sregs.ss.selector;

    /* A periodic SIGALRM (no SA_RESTART) forces KVM_RUN to return EINTR, so a
     * guest that never exits cannot hang the runner. */
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = on_alarm;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGALRM, &sa, NULL);
    memset(&tv, 0, sizeof(tv));
    tv.it_interval.tv_usec = 10000;
    tv.it_value.tv_usec = 10000;
    setitimer(ITIMER_REAL, &tv, NULL);
    clock_gettime(CLOCK_MONOTONIC, &t0);

    for (;;) {
        int rc;
        if (run_count + intr_count >= budget) {
            exit_kind = "budget";
            stop_reason = "run_budget";
            break;
        }
        clock_gettime(CLOCK_MONOTONIC, &t1);
        if ((uint64_t)(t1.tv_sec - t0.tv_sec) * 1000u +
                (uint64_t)((t1.tv_nsec - t0.tv_nsec) / 1000000) >
            deadline_ms) {
            exit_kind = "budget";
            stop_reason = "wall_deadline";
            break;
        }
        rc = ioctl(vcpu, KVM_RUN, 0);
        if (rc < 0) {
            if (errno == EINTR) {
                intr_count++;
                continue;
            }
            run_count++;
            exit_kind = "internal";
            stop_reason = "kvm_run_ioctl_failed";
            break;
        }
        if (run->exit_reason == KVM_EXIT_INTR) {
            intr_count++;
            continue;
        }
        run_count++;
        switch (run->exit_reason) {
        case KVM_EXIT_IO: {
            uint8_t *p = (uint8_t *)run + run->io.data_offset;
            unsigned size = run->io.size;
            unsigned n = run->io.count;
            unsigned k;
            if (run->io.direction == KVM_EXIT_IO_OUT) {
                for (k = 0; k < n; k++) {
                    uint64_t v = 0;
                    memcpy(&v, p + (size_t)k * size, size > 8 ? 8 : size);
                    add_io_exit(run->io.port, size, 1, v);
                    if (run->io.port == UART_PORT && uart_len < MAX_UART)
                        uart[uart_len++] = (unsigned char)(v & 0xff);
                }
            } else {
                /* Port 0x3f8 is write-only here; every IN returns all-ones. */
                memset(p, 0xff, (size_t)size * n);
                for (k = 0; k < n; k++)
                    add_io_exit(run->io.port, size, 0, size == 0 ? 0ull
                                       : 0xffffffffffffffffull >> (64 - 8 * (size > 8 ? 8 : size)));
            }
            continue;
        }
        case KVM_EXIT_HLT:
            add_exit("hlt");
            exit_kind = "hlt";
            stop_reason = "guest_hlt";
            goto done;
        case KVM_EXIT_SHUTDOWN:
            add_exit("shutdown");
            exit_kind = "shutdown";
            stop_reason = "triple_fault_or_shutdown";
            goto done;
        case KVM_EXIT_FAIL_ENTRY:
            add_exit("fail_entry");
            exit_kind = "fail_entry";
            stop_reason = "vm_entry_failed";
            goto done;
        case KVM_EXIT_INTERNAL_ERROR:
            add_exit("internal");
            exit_kind = "internal";
            stop_reason = "kvm_internal_error";
            goto done;
        default:
            add_exit("other");
            exit_kind = "internal";
            stop_reason = "unexpected_exit_reason";
            goto done;
        }
    }
done:
    memset(&tv, 0, sizeof(tv));
    setitimer(ITIMER_REAL, &tv, NULL);
    print_json(exit_kind, NULL, &en, api, run_count, intr_count, stop_reason);
    return 0;
}
