| fixture | потоков | vCPU | процессов | rounds всего | наблюдено исходов x86-TSO | запрещённых | late barrier | rounds/s |
|---|---|---|---|---|---|---|---|---|
| 2+2W | 2 | 2 | 3 | 3000000 | 3/3 | 0 | 0 | 106k |
| IRIW | 4 | 2 | 3 | не применим (нужно 4 vCPU) | - | - | - | - |
| LB | 2 | 2 | 3 | 3000000 | 3/3 | 0 | 0 | 118k |
| LOCK-CMPXCHG-MUTEX | 2 | 2 | 3 | 3000000 | 2/2 | 0 | 0 | 136k |
| LOCK-INC(N=2,K=2) | 2 | 2 | 3 | 3000000 | 1/1 | 0 | 0 | 88k |
| LOCK-XADD | 2 | 2 | 3 | 3000000 | 2/2 | 0 | 0 | 125k |
| MP | 2 | 2 | 3 | 3000000 | 3/3 | 0 | 0 | 114k |
| SB | 2 | 2 | 3 | 3000000 | 3/4 | 0 | 0 | 113k |
| SB-LOCK | 2 | 2 | 3 | 3000000 | 3/3 | 0 | 0 | 85k |
| SB-LOCK-LOCAL | 2 | 2 | 3 | 3000000 | 3/4 | 0 | 0 | 96k |
| SB-LOCK-XADD | 2 | 2 | 3 | 3000000 | 3/3 | 0 | 0 | 86k |
| SB-MFENCE | 2 | 2 | 3 | 3000000 | 3/3 | 0 | 0 | 94k |
| SB-XCHG | 2 | 2 | 3 | 3000000 | 3/3 | 0 | 0 | 94k |
| STORE-FORWARDING | 2 | 2 | 3 | 3000000 | 2/2 | 0 | 0 | 110k |
| THREE-INSN-COUNTER(N=2,K=2) | 2 | 2 | 3 | 3000000 | 3/3 | 0 | 0 | 66k |
| UNLOCKED-INC | 2 | 2 | 3 | 3000000 | 2/2 | 0 | 0 | 140k |
| UNLOCKED-INC(N=2,K=2) | 2 | 2 | 3 | 3000000 | 3/3 | 0 | 0 | 104k |
| XCHG-MUTEX | 2 | 2 | 3 | 3000000 | 2/2 | 0 | 0 | 125k |
| 2+2W | 2 | 4 | 3 | 3000000 | 3/3 | 0 | 0 | 58k |
| IRIW | 4 | 4 | 3 | 3000000 | 15/15 | 0 | 0 | 52k |
| LB | 2 | 4 | 3 | 3000000 | 3/3 | 0 | 0 | 57k |
| LOCK-CMPXCHG-MUTEX | 2 | 4 | 3 | 3000000 | 2/2 | 0 | 0 | 58k |
| LOCK-INC(N=2,K=2) | 2 | 4 | 3 | 3000000 | 1/1 | 0 | 0 | 66k |
| LOCK-INC(N=4,K=2) | 4 | 4 | 3 | 3000000 | 1/1 | 0 | 0 | 63k |
| LOCK-XADD | 2 | 4 | 3 | 3000000 | 2/2 | 0 | 0 | 59k |
| MP | 2 | 4 | 3 | 3000000 | 3/3 | 0 | 0 | 54k |
| SB | 2 | 4 | 3 | 3000000 | 3/4 | 0 | 0 | 58k |
| SB-LOCK | 2 | 4 | 3 | 3000000 | 3/3 | 0 | 0 | 48k |
| SB-LOCK-LOCAL | 2 | 4 | 3 | 3000000 | 3/4 | 0 | 0 | 56k |
| SB-LOCK-XADD | 2 | 4 | 3 | 3000000 | 3/3 | 0 | 0 | 51k |
| SB-MFENCE | 2 | 4 | 3 | 3000000 | 3/3 | 0 | 0 | 57k |
| SB-XCHG | 2 | 4 | 3 | 3000000 | 3/3 | 0 | 0 | 56k |
| STORE-FORWARDING | 2 | 4 | 3 | 3000000 | 2/2 | 0 | 0 | 63k |
| THREE-INSN-COUNTER(N=2,K=2) | 2 | 4 | 3 | 3000000 | 3/3 | 0 | 0 | 54k |
| UNLOCKED-INC | 2 | 4 | 3 | 3000000 | 2/2 | 0 | 0 | 46k |
| UNLOCKED-INC(N=2,K=2) | 2 | 4 | 3 | 3000000 | 3/3 | 0 | 0 | 65k |
| UNLOCKED-INC(N=4,K=2) | 4 | 4 | 3 | 3000000 | 7/7 | 0 | 0 | 63k |
| XCHG-MUTEX | 2 | 4 | 3 | 3000000 | 2/2 | 0 | 0 | 58k |

всего rounds: 111000000, запрещённых исходов: 0

статусы файлов: corpus-v2-p1.json:ok, corpus-v2-p2.json:ok, corpus-v2-p3.json:ok, corpus-v4-p1.json:ok, corpus-v4-p2.json:ok, corpus-v4-p3.json:ok, formula-LOCK-INC-n2k2-v2-p1.json:ok, formula-LOCK-INC-n2k2-v2-p2.json:ok, formula-LOCK-INC-n2k2-v2-p3.json:ok, formula-LOCK-INC-n2k2-v4-p1.json:ok, formula-LOCK-INC-n2k2-v4-p2.json:ok, formula-LOCK-INC-n2k2-v4-p3.json:ok, formula-LOCK-INC-n4k2-v4-p1.json:ok, formula-LOCK-INC-n4k2-v4-p2.json:ok, formula-LOCK-INC-n4k2-v4-p3.json:ok, formula-THREE-INSN-COUNTER-n2k2-v2-p1.json:ok, formula-THREE-INSN-COUNTER-n2k2-v2-p2.json:ok, formula-THREE-INSN-COUNTER-n2k2-v2-p3.json:ok, formula-THREE-INSN-COUNTER-n2k2-v4-p1.json:ok, formula-THREE-INSN-COUNTER-n2k2-v4-p2.json:ok, formula-THREE-INSN-COUNTER-n2k2-v4-p3.json:ok, formula-UNLOCKED-INC-n2k2-v2-p1.json:ok, formula-UNLOCKED-INC-n2k2-v2-p2.json:ok, formula-UNLOCKED-INC-n2k2-v2-p3.json:ok, formula-UNLOCKED-INC-n2k2-v4-p1.json:ok, formula-UNLOCKED-INC-n2k2-v4-p2.json:ok, formula-UNLOCKED-INC-n2k2-v4-p3.json:ok, formula-UNLOCKED-INC-n4k2-v4-p1.json:ok, formula-UNLOCKED-INC-n4k2-v4-p2.json:ok, formula-UNLOCKED-INC-n4k2-v4-p3.json:ok
