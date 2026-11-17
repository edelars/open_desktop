#!/bin/sh
# S6-14 step 5 oracle: runs from the repo root; needs ssh access to the Intel Mini (BatchMode).
# Image: python3 make-images.py (in this directory). Host work dir: /tmp/od-s6-14-step5.
# Build: gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner
# The image is loaded at GPA 0x8000 and entered at 0800:0000 (the SIPI vector 0x08 entry, state
# after INIT): the trampoline of the S6-16e/f probe runs real mode -> protected mode -> IA-32e and
# stops at the HLT put at the AP's first 64-bit instruction (GPA 0xB000).
# Runs of the same image (the final state is the same in every run, the block differs):
#   oracle-<stem>.json          --result-block 0x9000:64   (GDT: null, code32, code64, data)
#   oracle-<stem>-blocks.jsonl  --result-block 0x8100:128 (AP-entered cell, GDTR, IDTR descriptors),
#                               0x8400:16 (EFER cell and jump cell), 0x10000:64 (PML4),
#                               0x11000:64 (PDPT), 0x12000:64 (PD), 0x13000:64 (PD_L),
#                               0x14000:64 (PT_L)
HOST=senya@192.168.1.39
D=docs/reports/data/stage6/s6-14-step5
W=/tmp/od-s6-14-step5
ssh -o BatchMode=yes $HOST "mkdir -p $W"
scp -o BatchMode=yes tools/kvm-runner/kvm_runner.c $D/*.bin $HOST:$W/
ssh -o BatchMode=yes $HOST "cd $W && gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner"
run() { ssh -o BatchMode=yes $HOST "cd $W && ./kvm_runner $1.bin --real-mode 0x0800:0x0000 --result-block $2"; }
for bin in $D/*.bin; do
  stem=$(basename $bin .bin)
  run $stem 0x9000:64 > $D/oracle-$stem.json
  : > $D/oracle-$stem-blocks.jsonl
  for blk in 0x8100:128 0x8400:16 0x10000:64 0x11000:64 0x12000:64 0x13000:64 0x14000:64; do
    run $stem $blk >> $D/oracle-$stem-blocks.jsonl
  done
done
