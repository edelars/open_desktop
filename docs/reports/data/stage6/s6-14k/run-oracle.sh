#!/bin/sh
# S6-14k oracle: runs from the repo root; needs ssh access to the Intel Mini (BatchMode).
# Images: python3 make-images.py (in this directory). Host work dir: /tmp/od-s6-14k.
# Build: gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner
# All images start at GPA 0 and are entered at 0000:0000. --result-block 0x30:64 captures the GDT
# pointer (0x30) and the GDT (0x38..0x6f) after the stop: the data segment loads set the accessed
# bit of a descriptor in memory (image b), so the model's memory must equal the captured bytes.
# The stop is the HLT at 0x203e (image f: 0x201f) executed in protected mode, before the RDMSR.
HOST=senya@192.168.1.39
D=docs/reports/data/stage6/s6-14k
ssh -o BatchMode=yes $HOST 'mkdir -p /tmp/od-s6-14k'
scp -o BatchMode=yes tools/kvm-runner/kvm_runner.c $D/*.bin $HOST:/tmp/od-s6-14k/
ssh -o BatchMode=yes $HOST 'cd /tmp/od-s6-14k && gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner'
run() { ssh -o BatchMode=yes $HOST "cd /tmp/od-s6-14k && ./kvm_runner $1.bin --real-mode 0x0000:0x0000 --result-block 0x30:64" > $D/oracle-$1.json; }
for stem in a-flat-data-accessed-set-cr4-06b0-0000-0000 \
    b-flat-data-accessed-clear-flags-fffffffe-0000-0000 \
    c-base-1000-avl-g-d-header-at-ds-base-0000-0000 d-byte-limit-5fff-0000-0000 \
    e-data-db0-g1-cr4-07ff-0000-0000 f-sme-flag-stop-at-rdmsr-201f-0000-0000; do
  run $stem
done
