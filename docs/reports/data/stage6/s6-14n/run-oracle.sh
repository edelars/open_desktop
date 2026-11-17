#!/bin/sh
# S6-14n oracle: runs from the repo root; needs ssh access to the Intel Mini (BatchMode).
# Images: python3 make-images.py (in this directory). Host work dir: /tmp/od-s6-14n.
# Build: gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner
# All images start at GPA 0 and are entered at 0000:0000. Per image ten runs of the same image:
#   oracle-<stem>.json          --result-block 0x30:64     (GDT pointer and GDT) + the final state
#   oracle-<stem>-tables.jsonl  five lines, --result-block 0x4000:64, 0x7000:64, 0x8000:64,
#                               0x9000:64 (first 8 entries of PML4, PDPT, PD, PT after the stop) and
#                               0x5000:64 (the trampoline header with the jump cell)
#   oracle-<stem>-high.jsonl    four lines, informational, not compared by the model (it never walks
#                               the target): 0x4ff8:8 (PML4[511]), 0xaff0:16 (PDPT2[510..511]),
#                               0xb048:8 (PD2[9]), 0xcce0:8 (PT2[0x19c]) - the entries of the oracle's
#                               fetch walk to 0xffffffff8139c090
# The stop is the HLT at the jump target (guest_hlt) or a fault with an empty IDT, which the runner
# reports as triple_fault_or_shutdown.
HOST=senya@192.168.1.39
D=docs/reports/data/stage6/s6-14n
ssh -o BatchMode=yes $HOST 'mkdir -p /tmp/od-s6-14n'
scp -o BatchMode=yes tools/kvm-runner/kvm_runner.c $D/*.bin $HOST:/tmp/od-s6-14n/
ssh -o BatchMode=yes $HOST 'cd /tmp/od-s6-14n && gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner'
run() { ssh -o BatchMode=yes $HOST "cd /tmp/od-s6-14n && ./kvm_runner $1.bin --real-mode 0x0000:0x0000 --result-block $2"; }
for bin in $D/*.bin; do
  stem=$(basename $bin -0000-0000.bin)-0000-0000
  run $stem 0x30:64 > $D/oracle-$stem.json
  : > $D/oracle-$stem-tables.jsonl
  for blk in 0x4000:64 0x7000:64 0x8000:64 0x9000:64 0x5000:64; do
    run $stem $blk >> $D/oracle-$stem-tables.jsonl
  done
  : > $D/oracle-$stem-high.jsonl
  for blk in 0x4ff8:8 0xaff0:16 0xb048:8 0xcce0:8; do
    run $stem $blk >> $D/oracle-$stem-high.jsonl
  done
done
