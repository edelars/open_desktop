#!/bin/sh
# S6-14o oracle: runs from the repo root; needs ssh access to the Intel Mini (BatchMode).
# Images: python3 make-images.py (in this directory). Host work dir: /tmp/od-s6-14o-r2.
# Build: gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner
# Image a is loaded at GPA 0x8000 and entered at 0800:0000 (SIPI vector 0x08 state) with the guest
# CPUID table of the model target (--cpuid-profile od-x86-64-v1-neutral). The full path of S6-14a
# (rows 1..8, including the vendor dispatcher CMP EBX,'Auth'/'Genu' + JNE of verify_cpu) runs
# real -> protected -> IA-32e and stops at the HLT at the jump target (GPA 0xB000).
# Runs of image a (the final state is the same in every run, the block differs):
#   oracle-a-...json          --result-block 0x9000:64   (GDT: null, code32, code64, data)
#   oracle-a-...-blocks.jsonl 0x8400:128 (header cells, LOCK, GDTR, IDTR descriptors),
#                             0x9F80:128 (stack), 0x10000:64 (PML4), 0x11000:64 (PDPT),
#                             0x12000:64 (PD), 0x13000:128 (PT, entries 0..15 only)
# Image b (CPUID probe, no profile and with the profile): result block 0x8200:192.
# Compatibility of the earlier modes: the same images run by the previous binary (kvm_runner_old,
# sha256 5f6a2aac... built from the S6-14n/step-5 source) and the new one give identical stdout
# (both pinned with taskset -c 0: leaf 1 EBX[31:24] of the host table is the APIC ID of the CPU the
# runner runs on). If kvm_runner_old is not on the host, put it there (the S6-14n binary) or the
# comparison is skipped with a message.
HOST=senya@192.168.1.39
D=docs/reports/data/stage6/s6-14o
W=/tmp/od-s6-14o-r2
ssh -o BatchMode=yes $HOST "mkdir -p $W"
scp -o BatchMode=yes tools/kvm-runner/kvm_runner.c $D/*.bin $HOST:$W/
scp -o BatchMode=yes docs/reports/data/stage6/s6-14n/a-2mib-secondary-startup-64-0000-0000.bin \
  docs/reports/data/stage6/s6-14-step5/a-probe-v2-normal-sipi-hlt-at-0xb000-0800-0000.bin $HOST:$W/
ssh -o BatchMode=yes $HOST "cd $W && gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner && sha256sum kvm_runner.c kvm_runner *.bin"
A=a-full-s6-14a-path-0800-0000
B=b-cpuid-probe-0800-0000
P="--cpuid-profile od-x86-64-v1-neutral"
run() { ssh -o BatchMode=yes $HOST "cd $W && ./kvm_runner $1.bin $2 --real-mode 0x0800:0x0000 --result-block $3"; }
run $A "$P" 0x9000:64 > $D/oracle-$A.json
: > $D/oracle-$A-blocks.jsonl
for blk in 0x8400:128 0x9F80:128 0x10000:64 0x11000:64 0x12000:64 0x13000:128; do
  run $A "$P" $blk >> $D/oracle-$A-blocks.jsonl
done
run $B "$P" 0x8200:192 > $D/oracle-$B-profile.json
# Diagnostic only: the host default table (no profile). Pinned to CPU 0: leaf 1 EBX[31:24] is the
# APIC ID of the CPU the runner runs on (without the pin the bytes depend on the scheduler).
ssh -o BatchMode=yes $HOST "cd $W && taskset -c 0 ./kvm_runner $B.bin --real-mode 0x0800:0x0000 --result-block 0x8200:192" > $D/oracle-$B-host-default.json

# Earlier modes: old and new binary, same image, stdout compared with cmp.
ssh -o BatchMode=yes $HOST "cd $W && printf '\\364' > hlt64.bin && if [ ! -x kvm_runner_old ]; then
  echo 'SKIP: kvm_runner_old is not on the host (the S6-14n binary, sha256 5f6a2aac...)'; exit 0; fi
  sha256sum kvm_runner_old
  chk() { name=\$1; shift; taskset -c 0 ./kvm_runner_old \"\$@\" > cmp-\$name-old.out
          taskset -c 0 ./kvm_runner \"\$@\" > cmp-\$name-new.out
          if cmp cmp-\$name-old.out cmp-\$name-new.out; then echo \"\$name identical \$(wc -c < cmp-\$name-new.out) bytes \$(sha256sum < cmp-\$name-new.out)\"; else echo \"\$name DIFFERS\"; fi; }
  chk step5 a-probe-v2-normal-sipi-hlt-at-0xb000-0800-0000.bin --real-mode 0x0800:0x0000 --result-block 0x9000:64
  chk n a-2mib-secondary-startup-64-0000-0000.bin --real-mode 0x0000:0x0000 --result-block 0x2000:64
  chk long64 hlt64.bin --capture-final-state
  chk plain hlt64.bin
  chk probe b-cpuid-probe-0800-0000.bin --real-mode 0x0800:0x0000 --result-block 0x8200:192"
