# od-isa

Архитектурное состояние x86, собственный декодер и эталонный интерпретатор.

Не зависит от JIT, машины, host API и дискового формата. Данные CPU и семантика инструкций — здесь; исполнение ARM64 — в od-jit.

Статус на 22 октября 2026 года: реализованы decoder и reference interpreter для ограниченного набора S0-09 из `docs/specs/CPU_CONTRACT.md` (`MOV r64, imm64`, register-only `ADD/SUB/CMP`, `JZ/JNZ`). Есть проверки branch boundaries, flags и instruction budget. Это не полная x86-64 реализация; ARM64 JIT и независимое сравнение ещё не готовы. Выполнять `cargo test -p od-isa --offline`.
