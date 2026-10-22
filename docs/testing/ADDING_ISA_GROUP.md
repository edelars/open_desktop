# Подключение новой группы ISA к независимой проверке

Использовать этот checklist до оптимизации новой группы. Проверка сначала проходит на интерпретаторе и физическом oracle; только после совпадения можно оценивать JIT.

## Checklist

1. **Нормативный источник:** записать точные документ/версию и страницы/таблицы Intel SDM или AMD APM. Отметить различия допустимого поведения до появления fixtures.
2. **CPU feature gate:** выписать обязательные features и проверить их на каждом физическом oracle host. Один Intel host не подтверждает AMD или другую microarchitecture.
3. **Байтовые формы:** перечислить opcode, prefix, ModR/M, SIB, displacement и immediate bytes. Для каждого поля указать разрешённые значения и запрещённые соседние кодировки.
4. **Архитектурные эффекты:** определить читаемые/записываемые регистры, flags и их definedness, RIP, memory effects и fault state. Не сравнивать неопределённые биты.
5. **Fixtures:** вручную составить независимые byte cases на типичные значения, boundaries и отрицательные forms; записать происхождение и expected exit. Generated cases дополняют ручные, но не заменяют их.
6. **Decode/fetch boundaries:** проверить каждый усечённый prefix/opcode/ModR/M/SIB/displacement/immediate, неверные branch/address targets и доступность байтов около guard page.
7. **Reference execution:** добавить семантику в `od-isa`, затем проверить regression corpus и boundary cases до изменения JIT.
8. **Physical profile:** добавить формы в allowlist только с точным feature requirement; проверять input до native execution. Timeout/signal/harness errors не являются успешной instruction result.
9. **Independent comparison:** запустить `stage1_harness.py --suite oracle` на неизменных fixtures и повторить с фиксированным seed. Проверить fingerprint, digest, schema, defined state и повторяемость.
10. **Оптимизация:** менять lowering только после совпадающих reference/native результатов. Сохранять fixtures и сравнения как регрессионные данные.

## Проверенный пример: ADD/SUB/CMP S0-09

Существующий пример задаёт формы `REX.W + 03 /r`, `REX.W + 2B /r` и `REX.W + 3B /r`, разрешает только регистровый ModR/M для RAX/RCX/RDX/RBX и сравнивает CF/PF/AF/ZF/SF/OF. Вручную составленные fixtures проверяют overflow, borrow, signed compare boundaries, а generated corpus покрывает фиксированные seed и boundary values. Source-of-truth семантики — [CPU contract v0.1](../specs/CPU_CONTRACT.md), Intel SDM 093; физическая проверка выполнена на одном Intel Core i5-3210M.

Физическое подтверждение ограничено текущим register-only профилем. Оно не доказывает AMD equivalence, memory addressing, privileged режим или полную ISA.
