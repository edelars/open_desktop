#!/bin/sh
# S6-14f oracle: runs from the repo root; needs ssh access to the Intel Mini (BatchMode).
# Host work dir: /tmp/od-s6-14f. Build: gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner
# --result-block GPA:16 captures the stack words 0x6010..0x601f of SS (hex in the JSON).
HOST=senya@192.168.1.39
D=docs/reports/data/stage6/s6-14f
ssh -o BatchMode=yes $HOST 'mkdir -p /tmp/od-s6-14f'
scp -o BatchMode=yes tools/kvm-runner/kvm_runner.c $D/*.bin $HOST:/tmp/od-s6-14f/
ssh -o BatchMode=yes $HOST 'cd /tmp/od-s6-14f && gcc -O2 -Wall -Wextra -Werror -std=c11 kvm_runner.c -o kvm_runner'
# run <stem> <CS:IP> <result-block GPA:SIZE>
run() { ssh -o BatchMode=yes $HOST "cd /tmp/od-s6-14f && ./kvm_runner $1.bin --real-mode $2 --result-block $3" > $D/oracle-$1.json; }
run a-call-success-tail-0000-1000 0x0000:0x1000 0x6010:16        # SS base 0, stack 0x6010..0x601f
run b-call-failure-tail-cf1-9f00-1000 0x9f00:0x1000 0xa5010:16   # SS = 0x9f00, 0x9f000 + 0x6010
run c-nested-call-push-imm8-popf-0000-1000 0x0000:0x1000 0x6010:16
run d-sp-wrap-call-ret-0000-1000 0x0000:0x1000 0xfff8:8          # SP = 0 wraps to 0xfffe
run e-sp-wrap-stop-in-callee-0000-1000 0x0000:0x1000 0xfff8:8
