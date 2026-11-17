#!/bin/sh
# S6-14j oracle: runs from the repo root; needs ssh access to the Intel Mini (BatchMode).
# Images: python3 make-images.py (in this directory). Host work dir: /tmp/od-s6-14j.
# Build: gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner
# All images start at GPA 0 and are entered at 0000:0000. --result-block 0x30:64 captures the GDT
# pointer (0x30) and the GDT (0x38..0x6f) after the stop: the far jump sets the accessed bit of the
# code descriptor in memory (image a), so the model's memory must equal the captured bytes.
# The stop is the HLT at CS.base + 0x2000 executed in protected mode after the far jump.
HOST=senya@192.168.1.39
D=docs/reports/data/stage6/s6-14j
ssh -o BatchMode=yes $HOST 'mkdir -p /tmp/od-s6-14j'
scp -o BatchMode=yes tools/kvm-runner/kvm_runner.c $D/*.bin $HOST:/tmp/od-s6-14j/
ssh -o BatchMode=yes $HOST 'cd /tmp/od-s6-14j && gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner'
run() { ssh -o BatchMode=yes $HOST "cd /tmp/od-s6-14j && ./kvm_runner $1.bin --real-mode 0x0000:0x0000 --result-block 0x30:64" > $D/oracle-$1.json; }
for stem in a-flat-code-accessed-clear-0000-0000 b-flat-code-accessed-set-0000-0000 \
    c-small-16bit-code-base-1000-0000-0000 d-page-granular-d1-base-1000-0000-0000 \
    e-conforming-rpl2-0000-0000 f-avl-g-d-base-8000-limit-0fffffff-0000-0000; do
  run $stem
done
