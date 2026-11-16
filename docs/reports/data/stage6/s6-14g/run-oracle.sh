#!/bin/sh
# S6-14g oracle: runs from the repo root; needs ssh access to the Intel Mini (BatchMode).
# Host work dir: /tmp/od-s6-14g. Build: gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner
# --result-block GPA:SIZE captures the stack words written by the image (hex in the JSON).
# CPUID values of the host (GenuineIntel, i5-3210M) are NEVER compared with the model: image e
# executes CPUID and the comparison skips EAX/EBX/ECX/EDX there.
HOST=senya@192.168.1.39
D=docs/reports/data/stage6/s6-14g
ssh -o BatchMode=yes $HOST 'mkdir -p /tmp/od-s6-14g'
scp -o BatchMode=yes tools/kvm-runner/kvm_runner.c $D/*.bin $HOST:/tmp/od-s6-14g/
ssh -o BatchMode=yes $HOST 'cd /tmp/od-s6-14g && gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner'
# run <stem> <CS:IP> <result-block GPA:SIZE>
run() { ssh -o BatchMode=yes $HOST "cd /tmp/od-s6-14g && ./kvm_runner $1.bin --real-mode $2 --result-block $3" > $D/oracle-$1.json; }
run a-verify-cpu-id-probe-0000-1000 0x0000:0x1000 0x5fe0:64
run b-alu-flags-matrix-0000-1000 0x0000:0x1000 0x5fa0:128
run c-jcc-forms-0000-1000 0x0000:0x1000 0x5fa0:128
run d-dword-stack-sp-wrap-0000-1000 0x0000:0x1000 0xfff0:16
run e-cpuid-flags-and-ip-only-0000-1000 0x0000:0x1000 0x5fe0:64
run f-test-jne-eax1-0000-1021 0x0000:0x1021 0x6000:16
run g-test-jne-eax0-0000-1021 0x0000:0x1021 0x6000:16
