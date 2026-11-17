#!/bin/sh
# S6-14m oracle: runs from the repo root; needs ssh access to the Intel Mini (BatchMode).
# Images: python3 make-images.py (in this directory). Host work dir: /tmp/od-s6-14m.
# Build: gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner
# All images start at GPA 0 and are entered at 0000:0000. Per image five runs of the same image:
#   oracle-<stem>.json          --result-block 0x30:64     (GDT pointer and GDT) + the final state
#   oracle-<stem>-tables.jsonl  four lines, --result-block 0x4000:64, 0x7000:64, 0x8000:64, 0x9000:64
#                               (first 8 entries of PML4, PDPT, PD, PT after the stop)
# The stop is the HLT at 0x20f0 (after the far jump) or a fault with an empty IDT, which the
# runner reports as triple_fault_or_shutdown.
HOST=senya@192.168.1.39
D=docs/reports/data/stage6/s6-14m
ssh -o BatchMode=yes $HOST 'mkdir -p /tmp/od-s6-14m'
scp -o BatchMode=yes tools/kvm-runner/kvm_runner.c $D/*.bin $HOST:/tmp/od-s6-14m/
ssh -o BatchMode=yes $HOST 'cd /tmp/od-s6-14m && gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner'
run() { ssh -o BatchMode=yes $HOST "cd /tmp/od-s6-14m && ./kvm_runner $1.bin --real-mode 0x0000:0x0000 --result-block $2"; }
for bin in $D/*.bin; do
  stem=$(basename $bin -0000-0000.bin)-0000-0000
  run $stem 0x30:64 > $D/oracle-$stem.json
  : > $D/oracle-$stem-tables.jsonl
  for gpa in 0x4000 0x7000 0x8000 0x9000; do
    run $stem $gpa:64 >> $D/oracle-$stem-tables.jsonl
  done
done
