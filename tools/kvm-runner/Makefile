# S4-34a: builds only on a Linux host with <linux/kvm.h> (the Mac mini).
CC ?= gcc
CFLAGS ?= -O2 -Wall -Wextra -Werror -std=c11

kvm_runner: kvm_runner.c
	$(CC) $(CFLAGS) -o $@ $<

clean:
	rm -f kvm_runner

.PHONY: clean
