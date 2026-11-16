#!/bin/sh
# S6-14i oracle: runs from the repo root; needs ssh access to the Intel Mini (BatchMode).
# Host work dir: /tmp/od-s6-14i. Build: gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner
# All images start at GPA 0 and are entered at 0000:0000 (the runner places the image at the entry).
# --result-block GPA:SIZE captures guest memory after the stop (the descriptor-table area): LGDT/LIDT
# only read it, so the model's memory must equal the captured bytes.
# The stop is HLT: images a..e put HLT right after MOV CR0,EAX, so with PE = 1 the HLT executes in
# protected mode (CS cache unchanged); stop_reason must be guest_hlt in every image.
HOST=senya@192.168.1.39
D=docs/reports/data/stage6/s6-14i
ssh -o BatchMode=yes $HOST 'mkdir -p /tmp/od-s6-14i'
scp -o BatchMode=yes tools/kvm-runner/kvm_runner.c $D/*.bin $HOST:/tmp/od-s6-14i/
ssh -o BatchMode=yes $HOST 'cd /tmp/od-s6-14i && gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner'
# run <stem> <CS:IP> <result-block GPA:SIZE>
run() { ssh -o BatchMode=yes $HOST "cd /tmp/od-s6-14i && ./kvm_runner $1.bin --real-mode $2 --result-block $3" > $D/oracle-$1.json; }
run a-block4-verbatim-0000-0000 0x0000:0x0000 0x30:64
run b-ds-base-high-base-byte-0000-0000 0x0000:0x0000 0x2030:64
run c-cr0-no-pe-et-forced-0000-0000 0x0000:0x0000 0x0:16
run d-cr0-pe-extra-bits-0000-0000 0x0000:0x0000 0x0:16
run e-cr0-cd-nw-no-pe-0000-0000 0x0000:0x0000 0x0:16
