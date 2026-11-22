# ADR-0014 — DAZ и MXCSR_MASK цели

**Дата:** 2026-11-21

**Задача:** расхождение DAZ из [S2-39n](../plans/STAGE_2.md#s2-39n-narrow-sse-forms-movssmovsd-and-ldmxcsrstmxcsr-of-the-cpl0-set) (срез [S2-39p](../plans/STAGE_2.md#s2-39p-daz-in-the-target-mxcsr_mask)).

**Статус:** принято как архитектурное решение по поручению координатора, без ожидания владельца. Ревью проводит координатор (Claude Opus / `high`).

**Статус (обновлено): superseded по приоритету main (merge 56c60c9).** Решение владельца «приоритет main» при слиянии main в stage8 (merge-коммит 56c60c9): маска `TARGET_FXSAVE_MXCSR_MASK` возвращена к `0x0000FFBF` (S7-31i, DAZ не поддерживается), `LDMXCSR` с памятью исполняется только S7-31i `Operation::LdmxcsrMemory` (`#GP(0)` на DAZ, CR0.EM/CR4.OSFXSR = 0 — `Unsupported`, без префиксов REX/66/67/сегмент). Расхождение с оракулом (Intel Core i5-3210M принимает DAZ, S2-39p) записано открытым вопросом владельцу: [ISSUE-663](../KNOWN_ISSUES.md).

## Вопрос

Поддерживает ли цель бит DAZ (MXCSR бит 6)? До этого решения модель сообщала в образе FXSAVE MXCSR_MASK `0x0000FFBF`, а LDMXCSR и FXRSTOR64 давали `#GP(0)` на бит 6. Физический процессор oracle загружает DAZ. Из-за этого oracle S2-39n показал 40 расхождений политики.

## Факты

Источник — Intel SDM, редакция 325462-093US (тот же файл, что в S2-37/S2-55).

1. **Физическая машина** (Mac mini, Intel Core i5-3210M, Ubuntu, Linux 7.0.0-42, gcc 15.2.0; программы `daz_oracle.c` и `daz_addss.c` в ignored-каталоге `work/stage8/s2_39p_daz_oracle/`; сохранён один прогон `daz_oracle`/`daz_addss`, второго сохранённого прогона нет; для 40 случаев S2-39n два прогона подтверждены совпадением sha256 выгрузок `oracle_out` и `oracle_out2`). FXSAVE64 записывает MXCSR_MASK `0x0000FFFF`, и после загрузки DAZ тоже. LDMXCSR со значениями `0x40`, `0x1FC0`, `0x9FC0`, `0xFFFF` и `0xFFBF` исполняется, STMXCSR возвращает то же значение. `0x00010000` и `0x80001F80` дают SIGSEGV (`#GP`). FXRSTOR64 образа с MXCSR `0x1FC0` исполняется. Все 40 случаев DAZ в oracle S2-39n (`~/s2_39n_sse_set_oracle/` на mini: значения `0x40`, `0x1FC0`, `0x8040`, `0x1F40`, `0xFFFF` в 8 формах адресации) исполняются с `mx` = загруженному значению. Регистры не меняются, и в обоих прогонах сторона железа одинакова.
2. **Модель до решения:** `TARGET_FXSAVE_MXCSR_MASK = 0x0000_FFBF` («conservative DAZ-clear» профиль [CPU_CONTRACT.md](../specs/CPU_CONTRACT.md)). В образе FXSAVE было `0xFFBF`, LDMXCSR и FXRSTOR64 давали `#GP(0)` на бит 6.
3. **SDM.** Vol. 1 §10.2.3.4 (с. 10-5): DAZ появился в Pentium 4 и Xeon с SSE2. Бит зарезервирован только «in earlier IA-32 processors and in some models of the Pentium 4 processor». Попытка установить его там, где DAZ нет, даёт `#GP`. §11.6.3 (с. 11-20): DAZ есть «in most of the Pentium 4 processors and in the Intel Xeon processor, with the exception of some early steppings». Поддержка определяется только битом 6 поля MXCSR_MASK образа FXSAVE; отдельного бита CPUID нет. §11.6.6 (с. 11-21): нулевое поле означает `0x0000FFBF` (DAZ нет). Иначе ОС и программы должны умножать (AND) записываемое значение на MXCSR_MASK. Vol. 2A: FXRSTOR `#GP` «For an attempt to set reserved bits in MXCSR» (с. 3-410), поле MXCSR_MASK в FXSAVE (с. 3-414), LDMXCSR (с. 3-539).
4. **Целевой CPU** — Intel Celeron N5105 ([ADR-0013](ADR-0013-windows-cpu-profile.md), решение 2). Физически не проверен: машины нет. Это процессор 2021 года с SSE4.2. Исключения SDM относятся только к ранним Pentium 4, поэтому мы считаем, что DAZ на нём есть. Это вывод из SDM, а не измерение.
5. **Windows.** Источник Microsoft о DAZ и MXCSR_MASK не найден: M1, M2 и M20 из ADR-0013 их не называют. По §11.6.6 ОС, которая записывает MXCSR из контекста программы, обрезает значение маской из FXSAVE. Ядро Linux так и делает (`mxcsr_feature_mask` из FXSAVE, `arch/x86/kernel/fpu/init.c`; [CPU_CONTRACT.md](../specs/CPU_CONTRACT.md), разбор S2-39 образа ядра). Для гостя важна согласованность: маска в FXSAVE и проверка в LDMXCSR и FXRSTOR должны совпадать. Поведение Windows наблюдать на S8-24.
6. **Семантика DAZ (S2-37) уже есть.** `sse_fp` (`daz32`/`daz64`, подавление DE) сверен с железом, где DAZ загружался LDMXCSR. Пока маска была `0xFFBF`, гость не мог установить DAZ ни LDMXCSR, ни FXRSTOR64, и эта ветка оставалась недостижимой.

## Варианты

- **A. Маска `0x0000FFFF` везде, где модель её сообщает или проверяет:** образ FXSAVE, LDMXCSR, FXRSTOR64. Совпадает с физической машиной и с SDM для целевого CPU. Включает уже проверенную семантику DAZ.
- **B. Оставить `0x0000FFBF` и `#GP(0)` с явной записью.** Это согласованно по §11.6.6, но расходится с i5-3210M и, по SDM, с N5105. Программа, которая ставит DAZ без проверки MXCSR_MASK, получит `#GP`, которого на целевом железе нет.
- **C. Принимать DAZ, но сообщать `0xFFBF`.** Отвергнут: нарушает контракт §11.6.6 (бит, помеченный зарезервированным, принимается).

## Решение

**Вариант A.** `od_isa::TARGET_FXSAVE_MXCSR_MASK = 0x0000_FFFF`. FXSAVE и FXSAVE64 записывают `0x0000FFFF` в байты 28–31. LDMXCSR и FXRSTOR64 дают `#GP(0)` только при битах 31:16. Валидатор результата testkit (S2-39h) ожидает `0xFFFF`.

## Последствия и границы

- 40 расхождений политики из S2-39n закрыты по выгрузке oracle (на железе `ok`, `mx` = значение; в модели те же пять значений в семи формах адресации теста (`[rax+1]`, `[r8+1]`, `[rax+rcx*2+1]`, `[rbp+0x101]`, `[eax+1]` с префиксом `0x67`, `ds:[rax+1]`, `[RIP+disp32]`; у oracle форм 8) теперь исполняются, тест `s2_39_cpl0_sse_set`). Сравнитель S2-39n (`src/main.rs`) лежал в ignored-каталоге удалённого worktree и заново не запускался.
- Не меняются: маски допуска узких профилей schema-v2 (`mxcsr & !0xffbf`, «DAZ/FTZ clear») — это локальные ограничения профиля, вне которых профиль отказывает (`Unsupported`), а не `#GP`. Не меняется и метаданное фикстур `diagnostic_mxcsr_mask = 0x0000ffbf`: по CPU_CONTRACT оно не определяет возможности цели.
- Биты CPUID не объявляются (S2-57); у DAZ бита CPUID нет.
- XSAVE не реализован (ADR-0013, решение 4). Когда он появится, MXCSR_MASK в legacy-области XSAVE должен браться из той же константы.
- Не проверено: целевой N5105 физически, Windows, native-пути. В `od-jit` и `od-host` нет обработки LDMXCSR и FXSAVE/FXRSTOR (поиск по именам), поэтому JIT решение не затрагивает.
