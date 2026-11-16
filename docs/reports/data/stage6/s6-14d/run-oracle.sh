#!/bin/sh
# S6-14d oracle: runs from the repo root; needs ssh access to the Intel Mini (BatchMode).
# Host work dir: /tmp/od-s6-14d. Build: gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner
HOST=senya@192.168.1.39
D=docs/reports/data/stage6/s6-14d
ssh -o BatchMode=yes $HOST 'mkdir -p /tmp/od-s6-14d'
scp -o BatchMode=yes tools/kvm-runner/kvm_runner.c $D/*.bin $HOST:/tmp/od-s6-14d/
ssh -o BatchMode=yes $HOST 'cd /tmp/od-s6-14d && gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner'
run() { ssh -o BatchMode=yes $HOST "cd /tmp/od-s6-14d && ./kvm_runner $1.bin --real-mode $2" > $D/oracle-$1.json; }
run a-linked-0000-1000 0x0000:0x1000
run b-reloc-0100-1000 0x0100:0x1000
run c-farjmp-0100-1000 0x0100:0x1000
run d-sipi9f-9f00-0000 0x9f00:0x0000
