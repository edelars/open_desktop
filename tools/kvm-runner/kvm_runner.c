/*
 * S4-34a: minimal KVM host runner (single file, plain C11).
 *
 * Creates a VM through /dev/kvm, maps 2 MiB of anonymous guest RAM at GPA 0,
 * loads a raw freestanding image at GPA 0x1000, builds the guest's own GDT and
 * identity page tables (PML4 0x2000, PDPT 0x3000, PD 0x4000, one 2 MiB leaf),
 * enters Long64/CPL0 at RIP 0x1000, or starts in reset real mode at a requested
 * CS:IP, and runs KVM_RUN. UART bytes are collected from I/O exits on port 0x3f8
 * (write only). Execution is VT-x of the host CPU;
 * no software MMU/CPU is involved. No irqchip is created: the guest runs
 * without interrupts and HLT is reported to userspace.
 *
 * Output: one JSON line on stdout. Diagnostics go to stderr.
 * run_count counts KVM_RUN returns that carried a real exit; intr_count counts
 * EINTR / KVM_EXIT_INTR returns from the SIGALRM watchdog or immediate-exit I/O
 * completion. The budget limit applies to run_count + intr_count, except that
 * one final immediate-exit re-entry to complete pending I/O may exceed it by one.
 *
 * Build: gcc -O2 -Wall -Wextra -Werror -std=c11 -o kvm_runner kvm_runner.c
 * Usage: kvm_runner <image.bin> [--real-mode CS:IP] [--capture-final-state]
 *        [--result-block GPA:SIZE] [--inject-after-hlt VECTOR] [--budget N] [--deadline-ms MS]
 *        [--cpuid-profile od-x86-64-v1-neutral]
 * --cpuid-profile (S6-14o) replaces the guest CPUID table by exactly the leaves 0, 1, 0x80000000,
 * 0x80000001, 0x80000008 of the OdX8664V1Neutral profile (crates/od-isa/src/cpuid.rs). Without it
 * the behaviour and the output are byte for byte those of the earlier modes.
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
#define REAL_MODE_IMAGE_MAX 0x10000u
#define PML4_GPA 0x2000u
#define PDPT_GPA 0x3000u
#define PD_GPA 0x4000u
#define GDT_GPA 0x5000u
#define STACK_TOP 0x8000u
#define UART_PORT 0x3f8u
#define MAX_UART 4096u
#define MAX_EXITS 256u
#define CPUID_MAX_ENT 256u
#define RESULT_BLOCK_MAX 512u
#define RESULT_BLOCK_MAGIC 0x314d564b57483453ull
#define RESULT_BLOCK_VERSION 1u
#define RESULT_BLOCK_COMPLETE 0x434f4d50u

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
static struct kvm_cpuid_entry2 guest_cpuid[CPUID_MAX_ENT];
static unsigned guest_cpuid_count;
static int capture_final_state;
static int result_block_requested;
static int result_block_captured;
static uint64_t result_block_gpa;
static unsigned result_block_size;
static unsigned char result_block_bytes[RESULT_BLOCK_MAX];
static int inject_after_hlt_requested;
static uint32_t inject_vector;
static unsigned inject_attempts;
static int inject_ioctl_result;
static int inject_ioctl_errno;
static uint32_t inject_event_count_before;
static uint32_t inject_event_count_after;
static uint32_t inject_case_id;
static int inject_event_count_before_valid;
static int inject_event_count_after_valid;
static int inject_delivery_observed;

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

static void host_identity(char *cpu_model, size_t cpu_len, char *microcode, size_t microcode_len,
                          int *vmx)
{
    FILE *f = fopen("/proc/cpuinfo", "r");
    char line[1024];
    int have_model = 0;
    int have_microcode = 0;
    cpu_model[0] = '\0';
    microcode[0] = '\0';
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
        } else if (!have_microcode && strncmp(line, "microcode", 9) == 0) {
            char *colon = strchr(line, ':');
            if (colon) {
                colon++;
                while (*colon == ' ' || *colon == '\t')
                    colon++;
                colon[strcspn(colon, "\r\n")] = '\0';
                snprintf(microcode, microcode_len, "%s", colon);
                have_microcode = 1;
            }
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

struct final_state {
    struct kvm_regs regs;
    struct kvm_sregs sregs;
};

static struct final_state pre_injection_state;
static int pre_injection_state_valid;

struct result_block_header {
    uint64_t magic;
    uint32_t version;
    uint32_t case_id;
    uint32_t completion;
    uint32_t observation_words;
    uint32_t event_count;
    uint32_t reserved;
};

static void set_real_seg(struct kvm_segment *s, uint16_t selector, uint64_t base, uint8_t type)
{
    memset(s, 0, sizeof(*s));
    s->base = base;
    s->limit = 0xffffu;
    s->selector = selector;
    s->type = type;
    s->present = 1;
    s->dpl = 0;
    s->db = 0;
    s->s = 1;
    s->l = 0;
    s->g = 0;
    s->avl = 0;
    s->unusable = 0;
}

static int parse_cs_ip(const char *text, uint16_t *cs, uint16_t *ip)
{
    char *end;
    unsigned long selector, offset;
    const char *colon = strchr(text, ':');
    char left[32];
    size_t left_len;

    if (!colon || colon == text || colon[1] == '\0')
        return -1;
    left_len = (size_t)(colon - text);
    if (left_len >= sizeof(left))
        return -1;
    memcpy(left, text, left_len);
    left[left_len] = '\0';
    errno = 0;
    selector = strtoul(left, &end, 0);
    if (errno || *end || selector > 0xffffu)
        return -1;
    errno = 0;
    offset = strtoul(colon + 1, &end, 0);
    if (errno || *end || offset > 0xffffu)
        return -1;
    *cs = (uint16_t)selector;
    *ip = (uint16_t)offset;
    return 0;
}

static int parse_u64(const char *text, uint64_t *value)
{
    char *end;
    unsigned long long parsed;

    if (!text || !*text || *text == '-' || *text == '+')
        return -1;
    errno = 0;
    parsed = strtoull(text, &end, 0);
    if (errno || end == text || *end)
        return -1;
    *value = (uint64_t)parsed;
    return 0;
}

static int parse_vector(const char *text, uint32_t *vector)
{
    char *end;
    unsigned long parsed;
    int base = 10;

    if (!text || !*text || *text == '-' || *text == '+')
        return -1;
    if (text[0] == '0' && (text[1] == 'x' || text[1] == 'X'))
        base = 16;
    errno = 0;
    parsed = strtoul(text, &end, base);
    if (errno || end == text || *end || parsed < 32 || parsed > 255)
        return -1;
    *vector = (uint32_t)parsed;
    return 0;
}

static int parse_gpa_size(const char *text, uint64_t *gpa, unsigned *size)
{
    uint64_t parsed_gpa, parsed_size;
    const char *colon = strchr(text, ':');
    char gpa_text[32];
    size_t gpa_len;

    if (!colon || colon == text || colon[1] == '\0')
        return -1;
    gpa_len = (size_t)(colon - text);
    if (gpa_len >= sizeof(gpa_text))
        return -1;
    memcpy(gpa_text, text, gpa_len);
    gpa_text[gpa_len] = '\0';
    if (parse_u64(gpa_text, &parsed_gpa) != 0 ||
        parse_u64(colon + 1, &parsed_size) != 0 ||
        parsed_size == 0 || parsed_size > RESULT_BLOCK_MAX)
        return -1;
    *gpa = parsed_gpa;
    *size = (unsigned)parsed_size;
    return 0;
}

static int read_result_header(const uint8_t *ram, struct result_block_header *header)
{
    if (!result_block_requested || result_block_size < sizeof(*header))
        return -1;
    memcpy(header, ram + result_block_gpa, sizeof(*header));
    if (header->magic != RESULT_BLOCK_MAGIC || header->version != RESULT_BLOCK_VERSION ||
        header->case_id == 0 || header->reserved != 0 ||
        header->observation_words > (result_block_size - sizeof(*header)) / sizeof(uint64_t))
        return -1;
    return 0;
}

static int capture_result_block(const uint8_t *ram)
{
    if (!result_block_requested)
        return 0;
    memcpy(result_block_bytes, ram + result_block_gpa, result_block_size);
    result_block_captured = 1;
    return 0;
}

static void configure_real_mode(struct kvm_sregs *sregs, uint16_t cs)
{
    /* A conventional real-mode CS:IP pair has a segment base of CS * 16. */
    set_real_seg(&sregs->cs, cs, (uint64_t)cs << 4, 0xbu);
    set_real_seg(&sregs->ds, 0, 0, 0x3u);
    set_real_seg(&sregs->es, 0, 0, 0x3u);
    set_real_seg(&sregs->fs, 0, 0, 0x3u);
    set_real_seg(&sregs->gs, 0, 0, 0x3u);
    set_real_seg(&sregs->ss, 0, 0, 0x3u);
    /* INIT preserves CD/NW from the prior CR0 and sets ET (SMP_CONTRACT §4). */
    sregs->cr0 = (sregs->cr0 & 0x60000000u) | 0x10u;
    sregs->cr2 = 0;
    sregs->cr3 = 0;
    sregs->cr4 = 0;
    sregs->cr8 = 0;
    sregs->efer = 0;
    sregs->gdt.base = 0;
    sregs->gdt.limit = 0xffffu;
    sregs->idt.base = 0;
    sregs->idt.limit = 0xffffu;
}

static void service_io_exit(struct kvm_run *run)
{
    uint8_t *p = (uint8_t *)run + run->io.data_offset;
    unsigned size = run->io.size;
    unsigned count = run->io.count;
    unsigned i;

    if (run->io.direction == KVM_EXIT_IO_OUT) {
        for (i = 0; i < count; i++) {
            uint64_t value = 0;
            memcpy(&value, p + (size_t)i * size, size > 8 ? 8 : size);
            add_io_exit(run->io.port, size, 1, value);
            if (run->io.port == UART_PORT && uart_len < MAX_UART)
                uart[uart_len++] = (unsigned char)(value & 0xff);
        }
    } else {
        /* Port 0x3f8 is write-only here; every IN returns all-ones. */
        memset(p, 0xff, (size_t)size * count);
        for (i = 0; i < count; i++)
            add_io_exit(run->io.port, size, 0,
                        size == 0 ? 0ull
                                  : 0xffffffffffffffffull >> (64 - 8 * (size > 8 ? 8 : size)));
    }
}

/* Complete userspace I/O while preventing the guest from starting another instruction. */
static int drain_pending_io(int vcpu, struct kvm_run *run, unsigned *run_count,
                            unsigned *intr_count, int immediate_exit_supported)
{
    int rc;

    if (!immediate_exit_supported)
        return -3;
    run->immediate_exit = 1;
    rc = ioctl(vcpu, KVM_RUN, 0);
    run->immediate_exit = 0;
    if (rc < 0) {
        if (errno == EINTR) {
            (*intr_count)++;
            return 0;
        }
        return -1;
    }
    if (run->exit_reason == KVM_EXIT_INTR) {
        (*intr_count)++;
        return 0;
    }
    (*run_count)++;
    /* Any other exit means guest execution advanced; refuse a final snapshot. */
    return -2;
}

static void print_pre_state_json(const struct final_state *state)
{
    const struct kvm_regs *r = &state->regs;
    const struct kvm_sregs *s = &state->sregs;

    printf("{\"gpr\":{\"rax\":%llu,\"rbx\":%llu,\"rcx\":%llu,\"rdx\":%llu,"
           "\"rsi\":%llu,\"rdi\":%llu,\"rbp\":%llu,\"rsp\":%llu,\"r8\":%llu,\"r9\":%llu,"
           "\"r10\":%llu,\"r11\":%llu,\"r12\":%llu,\"r13\":%llu,\"r14\":%llu,\"r15\":%llu},"
           "\"rip\":%llu,\"rflags\":%llu,\"cs_selector\":%u,\"cs_l\":%u,\"cs_db\":%u,"
           "\"cs_dpl\":%u,\"ss_selector\":%u,\"cr0\":%llu,\"cr2\":%llu,\"cr3\":%llu,"
           "\"cr4\":%llu,\"cr8\":%llu,\"efer\":%llu}",
           (unsigned long long)r->rax, (unsigned long long)r->rbx,
           (unsigned long long)r->rcx, (unsigned long long)r->rdx,
           (unsigned long long)r->rsi, (unsigned long long)r->rdi,
           (unsigned long long)r->rbp, (unsigned long long)r->rsp,
           (unsigned long long)r->r8, (unsigned long long)r->r9,
           (unsigned long long)r->r10, (unsigned long long)r->r11,
           (unsigned long long)r->r12, (unsigned long long)r->r13,
           (unsigned long long)r->r14, (unsigned long long)r->r15,
           (unsigned long long)r->rip, (unsigned long long)r->rflags,
           s->cs.selector, s->cs.l, s->cs.db, s->cs.dpl, s->ss.selector,
           (unsigned long long)s->cr0, (unsigned long long)s->cr2,
           (unsigned long long)s->cr3, (unsigned long long)s->cr4,
           (unsigned long long)s->cr8, (unsigned long long)s->efer);
}

static void print_json(const char *exit_kind, const char *error, const struct entry_state *en,
                       int kvm_api, unsigned run_count, unsigned intr_count,
                       const char *stop_reason, const struct final_state *final, int real_mode)
{
    struct utsname un;
    char cpu_model[256];
    char microcode[64];
    char uname_str[512];
    char text[MAX_UART + 1];
    int vmx;
    unsigned i;

    host_identity(cpu_model, sizeof(cpu_model), microcode, sizeof(microcode), &vmx);
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
    printf("],\"exits_dropped\":%u,\"guest_cpuid_model\":[", exit_dropped);
    for (i = 0; i < guest_cpuid_count; i++) {
        const struct kvm_cpuid_entry2 *e = &guest_cpuid[i];
        if (i)
            putchar(',');
        printf("{\"function\":%u,\"index\":%u,\"flags\":%u,\"eax\":%u,\"ebx\":%u,\"ecx\":%u,\"edx\":%u}",
               e->function, e->index, e->flags, e->eax, e->ebx, e->ecx, e->edx);
    }
    putchar(']');
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
    if ((real_mode || capture_final_state || inject_after_hlt_requested) && final) {
        const struct kvm_regs *r = &final->regs;
        const struct kvm_sregs *s = &final->sregs;
        printf(",\"final_state\":{\"gpr\":{\"rax\":%llu,\"rbx\":%llu,\"rcx\":%llu,\"rdx\":%llu,"
               "\"rsi\":%llu,\"rdi\":%llu,\"rbp\":%llu,\"rsp\":%llu,\"r8\":%llu,\"r9\":%llu,"
               "\"r10\":%llu,\"r11\":%llu,\"r12\":%llu,\"r13\":%llu,\"r14\":%llu,\"r15\":%llu},"
               "\"rip\":%llu,\"rflags\":%llu,\"segments\":{",
               (unsigned long long)r->rax, (unsigned long long)r->rbx,
               (unsigned long long)r->rcx, (unsigned long long)r->rdx,
               (unsigned long long)r->rsi, (unsigned long long)r->rdi,
               (unsigned long long)r->rbp, (unsigned long long)r->rsp,
               (unsigned long long)r->r8, (unsigned long long)r->r9,
               (unsigned long long)r->r10, (unsigned long long)r->r11,
               (unsigned long long)r->r12, (unsigned long long)r->r13,
               (unsigned long long)r->r14, (unsigned long long)r->r15,
               (unsigned long long)r->rip, (unsigned long long)r->rflags);
#define PRINT_SEG(name, seg) \
        printf("\"%s\":{\"selector\":%u,\"base\":%llu,\"limit\":%u,\"type\":%u,\"dpl\":%u,\"present\":%u,\"db\":%u,\"l\":%u,\"g\":%u,\"unusable\":%u}", name, (seg).selector, (unsigned long long)(seg).base, (seg).limit, (seg).type, (seg).dpl, (seg).present, (seg).db, (seg).l, (seg).g, (seg).unusable)
        PRINT_SEG("cs", s->cs); putchar(',');
        PRINT_SEG("ds", s->ds); putchar(',');
        PRINT_SEG("es", s->es); putchar(',');
        PRINT_SEG("fs", s->fs); putchar(',');
        PRINT_SEG("gs", s->gs); putchar(',');
        PRINT_SEG("ss", s->ss); putchar(',');
        PRINT_SEG("tr", s->tr); putchar(',');
        PRINT_SEG("ldtr", s->ldt);
#undef PRINT_SEG
        printf("},\"tables\":{\"gdtr_base\":%llu,\"gdtr_limit\":%u,\"idtr_base\":%llu,\"idtr_limit\":%u},"
               "\"control\":{\"cr0\":%llu,\"cr2\":%llu,\"cr3\":%llu,\"cr4\":%llu,\"cr8\":%llu,\"efer\":%llu}}",
               (unsigned long long)s->gdt.base, s->gdt.limit,
               (unsigned long long)s->idt.base, s->idt.limit,
               (unsigned long long)s->cr0, (unsigned long long)s->cr2,
               (unsigned long long)s->cr3, (unsigned long long)s->cr4,
               (unsigned long long)s->cr8,
               (unsigned long long)s->efer);
    }
    else if (real_mode || capture_final_state || inject_after_hlt_requested)
        printf(",\"final_state\":null");
    if (inject_after_hlt_requested) {
        printf(",\"injection\":{\"requested\":true,\"vector\":%u,\"attempts\":%u,"
               "\"ioctl_result\":%d,\"ioctl_errno\":%d,\"event_count_before\":",
               inject_vector, inject_attempts, inject_ioctl_result, inject_ioctl_errno);
        if (inject_event_count_before_valid)
            printf("%u", inject_event_count_before);
        else
            printf("null");
        printf(",\"event_count_after\":");
        if (inject_event_count_after_valid)
            printf("%u", inject_event_count_after);
        else
            printf("null");
        printf(",\"delivery_observed\":%s,\"pre_state\":",
               inject_delivery_observed ? "true" : "false");
        if (pre_injection_state_valid)
            print_pre_state_json(&pre_injection_state);
        else
            printf("null");
        putchar('}');
    } else if (capture_final_state)
        printf(",\"injection\":null");
    if (result_block_requested) {
        printf(",\"result_block_gpa\":%llu,\"result_block_size\":%u,\"result_block_hex\":",
               (unsigned long long)result_block_gpa, result_block_size);
        if (result_block_captured) {
            putchar('"');
            for (i = 0; i < result_block_size; i++)
                printf("%02x", result_block_bytes[i]);
            putchar('"');
        } else
            printf("null");
    }
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
    printf(",\"host_microcode\":");
    json_string(microcode);
    printf(",\"vmx\":%s}\n", vmx ? "true" : "false");
}

static void fail(const char *exit_kind, const char *what, const struct entry_state *en,
                 int kvm_api, unsigned runs)
{
    char msg[256];
    snprintf(msg, sizeof(msg), "%s: %s", what, strerror(errno));
    print_json(exit_kind, msg, en, kvm_api, runs, 0, "setup_error", NULL, 0);
}

int main(int argc, char **argv)
{
    const char *image_path = NULL;
    int real_mode = 0;
    uint16_t real_cs = 0, real_ip = 0;
    uint64_t image_gpa = IMAGE_GPA;
    unsigned budget = 1000;
    unsigned deadline_ms = 2000;
    int kvm = -1, vm = -1, vcpu = -1;
    int api = 0;
    int immediate_exit_supported = 0;
    size_t run_size;
    struct kvm_run *run = NULL;
    uint8_t *ram;
    struct kvm_userspace_memory_region region;
    struct kvm_sregs sregs;
    struct kvm_regs regs;
    struct entry_state en;
    struct final_state final;
    uint32_t cpuid_signature = 0;
    int cpuid_leaf1_found = 0;
    int cpuid_profile_neutral = 0;
    int io_pending = 0;
    int final_state_valid = 0;
    int snapshot_safe = 1;
    const char *final_error = NULL;
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
        else if (strcmp(argv[i], "--capture-final-state") == 0)
            capture_final_state = 1;
        else if (strcmp(argv[i], "--result-block") == 0 && i + 1 < argc) {
            if (result_block_requested || parse_gpa_size(argv[++i], &result_block_gpa,
                                                         &result_block_size) != 0) {
                fprintf(stderr, "invalid --result-block GPA:SIZE (size must be 1..512 bytes)\n");
                return 2;
            }
            result_block_requested = 1;
            capture_final_state = 1;
        }
        else if (strcmp(argv[i], "--inject-after-hlt") == 0 && i + 1 < argc) {
            if (inject_after_hlt_requested || parse_vector(argv[++i], &inject_vector) != 0) {
                fprintf(stderr, "invalid --inject-after-hlt VECTOR (expected decimal or hex 32..255)\n");
                return 2;
            }
            inject_after_hlt_requested = 1;
            capture_final_state = 1;
        }
        else if (strcmp(argv[i], "--cpuid-profile") == 0 && i + 1 < argc) {
            if (cpuid_profile_neutral || strcmp(argv[++i], "od-x86-64-v1-neutral") != 0) {
                fprintf(stderr, "invalid --cpuid-profile (only od-x86-64-v1-neutral is known)\n");
                return 2;
            }
            cpuid_profile_neutral = 1;
        }
        else if (strcmp(argv[i], "--real-mode") == 0 && i + 1 < argc) {
            if (real_mode || parse_cs_ip(argv[++i], &real_cs, &real_ip) != 0) {
                fprintf(stderr, "invalid --real-mode CS:IP (expected 16-bit selector:offset)\n");
                return 2;
            }
            real_mode = 1;
        }
        else if (!image_path)
            image_path = argv[i];
        else {
            fprintf(stderr, "usage: %s <image.bin> [--real-mode CS:IP] [--capture-final-state] [--result-block GPA:SIZE] [--inject-after-hlt VECTOR] [--budget N] [--deadline-ms MS] [--cpuid-profile od-x86-64-v1-neutral]\n", argv[0]);
            return 2;
        }
    }
    if (!image_path) {
        fprintf(stderr, "usage: %s <image.bin> [--real-mode CS:IP] [--capture-final-state] [--result-block GPA:SIZE] [--inject-after-hlt VECTOR] [--budget N] [--deadline-ms MS] [--cpuid-profile od-x86-64-v1-neutral]\n", argv[0]);
        return 2;
    }
    if (result_block_requested &&
        (result_block_gpa > GUEST_RAM_SIZE ||
         result_block_size > GUEST_RAM_SIZE - result_block_gpa)) {
        fprintf(stderr, "--result-block GPA:SIZE must lie within guest RAM\n");
        return 2;
    }
    if (inject_after_hlt_requested &&
        (real_mode || !result_block_requested || result_block_gpa != 0x18000u ||
         result_block_size < sizeof(struct result_block_header))) {
        fprintf(stderr, "--inject-after-hlt requires Long64 and --result-block 0x18000:SIZE with a 32-byte header\n");
        return 2;
    }

    memset(&en, 0, sizeof(en));
    memset(&final, 0, sizeof(final));

    img = fopen(image_path, "rb");
    if (!img || fseek(img, 0, SEEK_END) != 0 || (img_len = ftell(img)) < 0 ||
        fseek(img, 0, SEEK_SET) != 0) {
        fail("internal", "open image", NULL, 0, 0);
        return 2;
    }
    if (img_len == 0 ||
        (unsigned long)img_len > (real_mode ? REAL_MODE_IMAGE_MAX : IMAGE_MAX)) {
        errno = EFBIG;
        fail("internal", real_mode ? "image exceeds the real-mode size limit"
                                    : "image size must be 1..0x1000 bytes",
             NULL, 0, 0);
        return 2;
    }

    if (real_mode) {
        image_gpa = ((uint64_t)real_cs << 4) + real_ip;
        if ((uint64_t)real_ip + (uint64_t)img_len > 0x10000u ||
            image_gpa + (uint64_t)img_len > GUEST_RAM_SIZE) {
            errno = EFBIG;
            fail("internal", "real-mode CS:IP image exceeds guest RAM", NULL, 0, 0);
            return 2;
        }
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
    if (real_mode || capture_final_state || result_block_requested)
        immediate_exit_supported = ioctl(kvm, KVM_CHECK_EXTENSION, KVM_CAP_IMMEDIATE_EXIT) > 0;
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

    if (fread(ram + image_gpa, 1, (size_t)img_len, img) != (size_t)img_len) {
        errno = EIO;
        fail("internal", "read image", NULL, api, 0);
        return 2;
    }
    fclose(img);

    /* Guest page tables: identity map of the first 2 MiB with one PS leaf. */
    if (!real_mode) {
        uint64_t *pml4 = (uint64_t *)(ram + PML4_GPA);
        uint64_t *pdpt = (uint64_t *)(ram + PDPT_GPA);
        uint64_t *pd = (uint64_t *)(ram + PD_GPA);
        pml4[0] = PDPT_GPA | 0x3;  /* P | RW */
        pdpt[0] = PD_GPA | 0x3;    /* P | RW */
        pd[0] = 0x0 | 0x83;        /* P | RW | PS: 0..2 MiB */
    }
    /* Guest GDT: null, 64-bit code (0x08), data (0x10). */
    if (!real_mode) {
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
                unsigned j;
                guest_cpuid_count = cpuid->nent;
                memcpy(guest_cpuid, cpuid->entries,
                       guest_cpuid_count * sizeof(guest_cpuid[0]));
                for (j = 0; j < cpuid->nent; j++) {
                    if (cpuid->entries[j].function == 1 && cpuid->entries[j].index == 0) {
                        cpuid_signature = cpuid->entries[j].eax;
                        cpuid_leaf1_found = 1;
                        break;
                    }
                }
                got = 1;
            } else if (errno == E2BIG && nent < CPUID_MAX_ENT) {
                nent = nent * 2 > CPUID_MAX_ENT ? CPUID_MAX_ENT : nent * 2;
            } else {
                fail("internal", "KVM_GET_SUPPORTED_CPUID", NULL, api, 0);
                return 2;
            }
        }
        if (cpuid_profile_neutral) {
            /* S6-14o: the CPUID table of the model target (OdX8664V1Neutral of
             * crates/od-isa/src/cpuid.rs) replaces the host-supported set: exactly the leaves
             * 0, 1, 0x80000000, 0x80000001 and 0x80000008, nothing else. */
            static const struct kvm_cpuid_entry2 neutral[] = {
                /* function, index, flags, eax, ebx, ecx, edx */
                {0, 0, 0, 1, 0x6E65704Fu /* "Open" */, 0x20706F74u /* "top " */,
                 0x6B736544u /* "Desk" */, {0, 0, 0}},
                {1, 0, 0, 0x600, 0x100, 0, 0x0781A379u, {0, 0, 0}},
                {0x80000000u, 0, 0, 0x80000008u, 0, 0, 0, {0, 0, 0}},
                {0x80000001u, 0, 0, 0, 0, 0, 0x20100800u, {0, 0, 0}},
                {0x80000008u, 0, 0, 0x3024, 0, 0, 0, {0, 0, 0}},
            };
            unsigned n = (unsigned)(sizeof(neutral) / sizeof(neutral[0]));
            memset(cpuid->entries, 0, nent * sizeof(cpuid->entries[0]));
            memcpy(cpuid->entries, neutral, sizeof(neutral));
            cpuid->nent = n;
            guest_cpuid_count = n;
            memset(guest_cpuid, 0, sizeof(guest_cpuid));
            memcpy(guest_cpuid, neutral, sizeof(neutral));
            cpuid_signature = neutral[1].eax;
            cpuid_leaf1_found = 1;
        }
        if (ioctl(vcpu, KVM_SET_CPUID2, cpuid) < 0) {
            fail("internal", "KVM_SET_CPUID2", NULL, api, 0);
            return 2;
        }
        free(cpuid);
    }
    if (real_mode && !cpuid_leaf1_found) {
        errno = ENOTSUP;
        fail("internal", "configured KVM CPUID model lacks leaf 1 signature", NULL, api, 0);
        return 2;
    }

    if (ioctl(vcpu, KVM_GET_SREGS, &sregs) < 0) {
        fail("internal", "KVM_GET_SREGS", NULL, api, 0);
        return 2;
    }
    if (real_mode) {
        configure_real_mode(&sregs, real_cs);
    } else {
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
    }
    if (ioctl(vcpu, KVM_SET_SREGS, &sregs) < 0) {
        fail("internal", "KVM_SET_SREGS", NULL, api, 0);
        return 2;
    }
    memset(&regs, 0, sizeof(regs));
    regs.rip = real_mode ? real_ip : IMAGE_GPA;
    regs.rsp = real_mode ? 0 : STACK_TOP;
    if (real_mode)
        regs.rdx = cpuid_signature; /* INIT preserves EDX processor signature. */
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
                io_pending = 0; /* Re-entry completes pending I/O before EINTR is returned. */
                intr_count++;
                continue;
            }
            run_count++;
            exit_kind = "internal";
            stop_reason = "kvm_run_ioctl_failed";
            if (real_mode || capture_final_state)
                final_error = "KVM_RUN ioctl failed; final CPU state omitted";
            break;
        }
        io_pending = 0; /* Successful KVM_RUN re-entry completes the previous I/O exit. */
        if (run->exit_reason == KVM_EXIT_INTR) {
            intr_count++;
            continue;
        }
        run_count++;
        switch (run->exit_reason) {
        case KVM_EXIT_IO:
            service_io_exit(run);
            io_pending = 1;
            continue;
        case KVM_EXIT_HLT:
            add_exit("hlt");
            if (inject_after_hlt_requested && inject_attempts == 0) {
                struct result_block_header header;
                struct kvm_interrupt interrupt_request;

                if (ioctl(vcpu, KVM_GET_SREGS, &pre_injection_state.sregs) < 0 ||
                    ioctl(vcpu, KVM_GET_REGS, &pre_injection_state.regs) < 0) {
                    exit_kind = "internal";
                    stop_reason = "pre_injection_state_failed";
                    final_error = "KVM_GET_SREGS or KVM_GET_REGS failed before injection";
                    goto done;
                }
                pre_injection_state_valid = 1;
                /* KVM_INTERRUPT is only supported here for the runner's Long64 CPL0 setup. */
                if (pre_injection_state.sregs.cs.l != 1 ||
                    pre_injection_state.sregs.cs.db != 0 ||
                    pre_injection_state.sregs.cs.dpl != 0 ||
                    (pre_injection_state.sregs.cs.selector & 3u) != 0 ||
                    (pre_injection_state.sregs.cr0 & 0x80000000u) == 0 ||
                    (pre_injection_state.sregs.efer & 0x400u) == 0) {
                    exit_kind = "internal";
                    stop_reason = "unsupported_injection_state";
                    final_error = "--inject-after-hlt requires a Long64 CPL0 state (CS.L=1, CS.DB=0, DPL/RPL=0, CR0.PG=1, EFER.LMA=1)";
                    goto done;
                }
                if (read_result_header(ram, &header) != 0 ||
                    header.completion == RESULT_BLOCK_COMPLETE) {
                    exit_kind = "internal";
                    stop_reason = "result_block_invalid_before_injection";
                    final_error = "result block header invalid or already complete before injection";
                    goto done;
                }
                inject_case_id = header.case_id;
                inject_event_count_before = header.event_count;
                inject_event_count_before_valid = 1;
                memset(&interrupt_request, 0, sizeof(interrupt_request));
                interrupt_request.irq = inject_vector;
                inject_attempts++;
                inject_ioctl_result = ioctl(vcpu, KVM_INTERRUPT, &interrupt_request);
                if (inject_ioctl_result < 0) {
                    inject_ioctl_errno = errno;
                    exit_kind = "internal";
                    stop_reason = "interrupt_injection_failed";
                    final_error = "KVM_INTERRUPT failed; userspace injection was not accepted";
                    goto done;
                }
                continue;
            }
            if (inject_after_hlt_requested && inject_attempts == 1) {
                struct result_block_header header;

                if (read_result_header(ram, &header) != 0) {
                    exit_kind = "internal";
                    stop_reason = "result_block_invalid_after_injection";
                    final_error = "result block header invalid after interrupt injection";
                    goto done;
                }
                inject_event_count_after = header.event_count;
                inject_event_count_after_valid = 1;
                if (header.case_id != inject_case_id ||
                    header.completion != RESULT_BLOCK_COMPLETE ||
                    header.event_count != inject_event_count_before + 1u) {
                    exit_kind = "internal";
                    stop_reason = "interrupt_not_delivered";
                    final_error = "injected vector did not produce exactly one completed guest event";
                    goto done;
                }
                inject_delivery_observed = 1;
            }
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
            if (real_mode || capture_final_state)
                final_error = "KVM_EXIT_FAIL_ENTRY; final CPU state omitted";
            goto done;
        case KVM_EXIT_INTERNAL_ERROR:
            add_exit("internal");
            exit_kind = "internal";
            stop_reason = "kvm_internal_error";
            if (real_mode || capture_final_state)
                final_error = "KVM_EXIT_INTERNAL_ERROR; final CPU state omitted";
            goto done;
        case KVM_EXIT_MMIO:
            add_exit(real_mode ? "mmio" : "other");
            exit_kind = "internal";
            stop_reason = real_mode ? "unsupported_mmio_exit" : "unexpected_exit_reason";
            if (real_mode || capture_final_state)
                final_error = "KVM_EXIT_MMIO is unsupported; final CPU state omitted";
            goto done;
        default:
            add_exit("other");
            exit_kind = "internal";
            stop_reason = "unexpected_exit_reason";
            if (real_mode || capture_final_state)
                final_error = "unsupported KVM exit; final CPU state omitted";
            goto done;
        }
    }
done:
    if (inject_after_hlt_requested && !inject_delivery_observed &&
        strcmp(stop_reason, "interrupt_injection_failed") != 0 &&
        strcmp(stop_reason, "interrupt_not_delivered") != 0 &&
        strcmp(stop_reason, "result_block_invalid_before_injection") != 0 &&
        strcmp(stop_reason, "pre_injection_state_failed") != 0 &&
        strcmp(stop_reason, "unsupported_injection_state") != 0) {
        exit_kind = "internal";
        stop_reason = "interrupt_not_delivered";
        if (!final_error)
            final_error = "guest stopped before one completed interrupt event was observed";
    }
    memset(&tv, 0, sizeof(tv));
    setitimer(ITIMER_REAL, &tv, NULL);
    if ((real_mode || capture_final_state || result_block_requested) && io_pending) {
        int drain_result = drain_pending_io(vcpu, run, &run_count, &intr_count,
                                            immediate_exit_supported);
        if (drain_result != 0) {
            snapshot_safe = 0;
            exit_kind = "internal";
            stop_reason = "pending_io_drain_failed";
            if (!final_error && drain_result == -1)
                final_error = "KVM_RUN failed while draining pending I/O";
            else if (!final_error && drain_result == -2)
                final_error = "guest advanced during pending-I/O drain";
            else if (!final_error)
                final_error = "KVM_CAP_IMMEDIATE_EXIT unavailable for pending-I/O drain";
        }
    }
    if (snapshot_safe && result_block_requested)
        (void)capture_result_block(ram);
    if (snapshot_safe && ((real_mode && !final_error) || capture_final_state)) {
        if (ioctl(vcpu, KVM_GET_SREGS, &final.sregs) < 0 ||
            ioctl(vcpu, KVM_GET_REGS, &final.regs) < 0) {
            exit_kind = "internal";
            stop_reason = "read_final_state_failed";
            if (!final_error)
                final_error = "KVM_GET_SREGS or KVM_GET_REGS failed";
        } else {
            final_state_valid = 1;
        }
    }
    print_json(exit_kind, final_error, &en, api, run_count, intr_count, stop_reason,
               final_state_valid ? &final : NULL, real_mode);
    return 0;
}
