#!/bin/sh
# S6-14e oracle: runs from the repo root; needs ssh access to the Intel Mini (BatchMode).
# Host work dir: /tmp/od-s6-14e. Build: gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner
# --result-block GPA:4 captures the lock/data word after the run (hex in the JSON).
HOST=senya@192.168.1.39
D=docs/reports/data/stage6/s6-14e
ssh -o BatchMode=yes $HOST 'mkdir -p /tmp/od-s6-14e'
scp -o BatchMode=yes tools/kvm-runner/kvm_runner.c $D/*.bin $HOST:/tmp/od-s6-14e/
ssh -o BatchMode=yes $HOST 'cd /tmp/od-s6-14e && gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner'
# run <stem> <CS:IP> <result-block GPA:SIZE>
run() { ssh -o BatchMode=yes $HOST "cd /tmp/od-s6-14e && ./kvm_runner $1.bin --real-mode $2 --result-block $3" > $D/oracle-$1.json; }
run a-linked-0000-1000 0x0000:0x1000 0x5018:4      # lock word at DS:0x5018, DS = 0
run b-reloc-9f00-1000 0x9f00:0x1000 0xa4018:4      # DS = 0x9f00, lock word at 0x9f000 + 0x5018
run c-bts-twice-0000-1000 0x0000:0x1000 0x5018:4   # second BTS sees CF = 1, JAE not taken
run d-bts-bit31-0000-1000 0x0000:0x1000 0x1020:4   # bit 31 of a data word initialised in the image
