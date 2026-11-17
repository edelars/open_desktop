#!/bin/sh
# S6-14l oracle: runs from the repo root; needs ssh access to the Intel Mini (BatchMode).
# Images: python3 make-images.py (in this directory). Host work dir: /tmp/od-s6-14l.
# Build: gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner
# All images start at GPA 0 and are entered at 0000:0000. --result-block 0x30:64 captures the GDT
# pointer and the GDT after the stop. The stop is the HLT that replaces `MOV CR0, EAX` (0x2062) or
# the HLT of the read-back tail (0x2074/0x2075); a refused WRMSR is a #GP without an IDT, which
# the runner reports as triple_fault_or_shutdown.
HOST=senya@192.168.1.39
D=docs/reports/data/stage6/s6-14l
ssh -o BatchMode=yes $HOST 'mkdir -p /tmp/od-s6-14l'
scp -o BatchMode=yes tools/kvm-runner/kvm_runner.c $D/*.bin $HOST:/tmp/od-s6-14l/
ssh -o BatchMode=yes $HOST 'cd /tmp/od-s6-14l && gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner'
run() { ssh -o BatchMode=yes $HOST "cd /tmp/od-s6-14l && ./kvm_runner $1.bin --real-mode 0x0000:0x0000 --result-block 0x30:64" > $D/oracle-$1.json; }
for stem in a-efer-901-written-and-read-back-0000-0000 b-efer-0-equal-skips-wrmsr-0000-0000 \
    c-efer-lme-only-0000-0000 d-efer-high-dword-bit32-reserved-0000-0000 \
    e-efer-sce-lma-written-0000-0000 f-efer-bit1-reserved-0000-0000 g-efer-bit12-svme-0000-0000 \
    h-efer-nxe-only-0000-0000 i-efer-bit9-reserved-0000-0000 j-efer-lma-only-0000-0000 k-efer-d01-lma-dropped-0000-0000; do
  run $stem
done
