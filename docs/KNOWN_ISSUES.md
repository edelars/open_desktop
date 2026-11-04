# Известные недоделки и проблемы

Реестр проблем, обнаруженных в ходе работы и не устранённых к завершению задачи. Правила ведения — в [AGENTS.md](../AGENTS.md#документирование-результатов-и-открытых-проблем). Планируемые возможности сами по себе не являются найденными проблемами и описываются в [ROADMAP.md](ROADMAP.md) и [планах этапов](plans/).

Для каждой записи указывать:

- Идентификатор, дату обнаружения и затронутый компонент.
- Суть недоделки или проблемы и её последствия.
- Условия воспроизведения или основание обнаружения, ссылки на исходники и результаты проверок при наличии.
- Статус: открыта, в работе или решена; необходимый следующий шаг.
- При решении — результат проверки и ссылку на обновлённую документацию реализации.

## Записи

<a id="issue-034-s2-42e-execute-only-code-not-admitted-by-native-v2"></a>

### ISSUE-034 — S2-42e execute-only code native acceptance is pending

- **Дата обнаружения:** 2026-11-04.
- **Компонент:** Linux x86_64 native-v2 mapping and S2-42e/S2-42q MOV acceptance.
- **Статус:** открыто; mapping compatibility fix implemented, full native acceptance rerun pending.
- **Суть и последствия:** The 1,080-case selection was rejected before execution because every fixture uses immutable logical execute-only code while the old `MappingPlan` required readable regions. The runner now projects that mapping to host RX for trace snapshots and retains logical permissions for pre-step admission. This is not hardware execute-only enforcement and does not claim guest `#PF`; native acceptance remains unverified until the unchanged suite passes after transfer.
- **Основание:** The 2026-11-04 Linux x86_64 run at source fingerprint `d740ae2b8b341138ce1149514ecf594df67138973bea4aa8fff78bf3e288a118` returned `unsupported` for all 1,080 first-repeat native results; no guest instruction executed. Evidence: [S2-42e summary](../work/stage2-42e-oracle/s2-42e-summary.json) and run records `work/stage2-42e-oracle/stage2-20261104-102111-498cc626/`. The execute-only contract is pinned in [S2-42e](testing/S2_42E_MOV_MEMORY_CORPUS.md#s2-42q-handwritten-boundary-controls).
- **Следующий шаг:** transfer the mapping fix, rerun the complete 1,080-case suite twice on Linux x86_64, and close this issue only if saved-run verification reports a pass. Otherwise retain the failed evidence and keep native acceptance open.

<a id="issue-033-s2-37cq-comiss-mixed-event-handling-audit"></a>

### ISSUE-033 — S2-37cq COMISS mixed-event handling needs audit

- **Дата обнаружения:** 2026-11-02.
- **Компонент:** `od-isa` COMISS comparison and the bounded S2-37cq/S2-37v profiles.
- **Статус:** открыто; расхождение с SDM или физическим x86 не установлено.
- **Суть и последствия:** S2-37cq и S2-37v задают локальную обработку mixed NaN/denormal operands. Нужен отдельный аудит raw classification, mask-dependent control flow и независимых oracle, чтобы подтвердить согласованность локальных профилей с применимыми правилами Intel SDM. S2-38u и S2-38v намеренно остаются invalid-only и отказывают после чтения при любом subnormal; этот узкий результат не разрешает аудит прежних профилей.
- **Основание:** Intel SDM Rev. 093 Vol. 2A перечисляет для COMISS invalid и denormal conditions; Vol. 1 §§4.9.2, 11.5.2.1–2 описывают precedence и SIMD exception behavior ([official PDF, Vol. 1](https://cdrdv2-public.intel.com/929351/253665-093-sdm-vol-1.pdf#page=112), [Vol. 2A](https://cdrdv2-public.intel.com/929353/253666-093-sdm-vol-2a.pdf#page=319)). Это основание для аудита, а не подтверждение дефекта. Физические проверки не выполнялись. См. [S2-37cq](specs/CPU_CONTRACT.md#s2-37cq--bounded-long64-comiss-xmm0dword-ptr-rax-m32-memory-profile), [S2-37v](specs/CPU_CONTRACT.md#s2-37v--canonical-long64-comiss-and-ucomiss-xmm-register-pairs), [S2-38u](specs/CPU_CONTRACT.md#s2-38u--exact-comiss-memory-invalid-only-profile) и [S2-38v](specs/CPU_CONTRACT.md#s2-38v--exact-ucomiss-memory-invalid-only-profile).
- **Следующий шаг:** выполнить профильный аудит production paths и независимых schema/testkit oracles S2-37cq/S2-37v на NaN, subnormal и mixed operands; при доступности физического x86 добавить контрольные MXCSR cases. Регистрировать mismatch только если его подтвердят код, нормативный анализ или измерение.

### ISSUE-032 — S2-37dj lacks physical x86 acceptance evidence

- **Дата обнаружения:** 2026-11-02.
- **Компонент:** локальный `Long64` interpreter profile `s2-long-addsd-memory-rax-v1` в od-isa.
- **Статус:** открытое ограничение приёмочных свидетельств; подтверждённый дефект не установлен.
- **Суть и последствия:** masked-only `ADDSD` m64 [RAX] проходит raw-bit ISA и независимый fixture oracle, но физическое исполнение на x86 не сравнивалось. Локальные проверки не подтверждают аппаратную fault ordering, unaligned memory behavior или `MXCSR` status.
- **Основание:** реализация и границы описаны в [standalone report](reports/S2_37DJ_ADDSD_MEMORY.md) и [shared report](reports/STAGE_2_REPORT.md#s2-37dj--addsd-xmm0m64-rax); все доступные проверки остаются локальными.
- **Следующий шаг:** включить точные encoding/profile, AC, span faults и `MXCSR` scenarios в общий физический x86 acceptance substage; зафиксировать CPU/microcode, режим и результаты. До этого не заявлять physical acceptance.

### ISSUE-031 — S2-37di lacks physical x86 acceptance evidence

- **Дата обнаружения:** 2026-11-01.
- **Компонент:** локальный Long64 interpreter profile `s2-long-addss-memory-rax-v1` в `od-isa`.
- **Статус:** открытое ограничение приёмочных свидетельств; подтверждённый дефект не установлен.
- **Суть и последствия:** exact `ADDSS XMM0,m32 [RAX]` согласуется с существующим ADDSS raw helper и независимым integer fixture oracle, но физическое исполнение на x86 не сравнивалось. Локальные тесты поэтому не подтверждают аппаратный fault ordering, unaligned memory behavior или MXCSR status на целевом процессоре.
- **Основание:** S2-37di добавляет interpreter-only профиль и фиксирует отсутствие физической проверки в [standalone report](reports/S2_37DI_ADDSS_MEMORY.md) и [shared report](reports/STAGE_2_REPORT.md#s2-37di--addss-xmm0m32-rax); локальные suites подтверждают только выбранную семантику и oracle.
- **Следующий шаг:** включить точные encoding/profile, AC, boundary faults и MXCSR scenarios в общий физический x86 acceptance substage; сохранить CPU/microcode, режим и результаты. До этого не заявлять physical acceptance.

### ISSUE-030 — S2-37dh lacks physical x86 acceptance evidence

- **Дата обнаружения:** 2026-11-01.
- **Компонент:** локальный Long64 interpreter profile `s2-long-minss-memory-rax-v1` в `od-isa`.
- **Статус:** открытое ограничение приёмочных свидетельств; подтверждённый дефект не установлен.
- **Суть и последствия:** точное исполнение `MINSS XMM0,m32 [RAX]` проверяется локальными raw-bit ISA и fixture oracle, но на физическом x86 процессоре не сравнивалось. Поэтому локальные проверки подтверждают согласованность реализации с выбранным контрактом и oracle, но не аппаратное поведение по AC, memory faults и MXCSR.
- **Основание:** S2-37dh добавляет только interpreter-only профиль и фиксирует отсутствие physical x86 acceptance в [standalone report](reports/S2_37DH_MINSS_MEMORY.md) и [shared report](reports/STAGE_2_REPORT.md#s2-37dh--minss-xmm0m32-rax). Локальные focused/full suites подтверждают fixture и semantic matrix.
- **Следующий шаг:** включить этот точный encoding/profile в общий физический x86 acceptance substage; сравнить результат и fault ordering на целевой аппаратуре и записать конфигурацию и наблюдения. До этого не заявлять physical acceptance.

### ISSUE-029 — Linux x86 runner target did not compile all tests

- **Дата обнаружения:** 2026-11-01; исправлено 2026-11-03.
- **Компонент:** `tools/x86-runner`, Linux x86_64-only `native_v2` backend.
- **Статус:** compile blocker решён 2026-11-03. Linux runtime/native admission execution, ptrace oracle execution и hardware acceptance остаются `not_run` на Darwin arm64 host.
- **Основание и последствия:** ранее `cargo check -p od-x86-runner --all-targets --target x86_64-unknown-linux-gnu --offline` обнаруживал два неполных Linux-only initializer-а в `native_v2.rs`: `Snapshot` и тестовый `Fixture`. Каждый не задавал 142 текущих optional control fields schema-v2. Все поля теперь явно заданы как `None`; наблюдаемое состояние, native admission policy и семантика не менялись. `serde_json` 1.0.151 уже присутствует в зависимостях runner и успешно компилируется для Linux target; отдельная dependency правка не потребовалась.
- **Воспроизведение и результат:** указанная cross-target команда после исправления завершается успешно для `--all-targets --offline`. Это подтверждает успешную type-check проверку Linux x86_64 targets, но не линкует final runner binary и не подтверждает Linux runtime, ptrace, native admission execution или hardware behavior.
- **Следующий шаг:** выполнить Linux native admission regression tests и S2-39k ptrace run на подходящей Linux x86_64 системе; hardware acceptance требует отдельно описанной физической среды и evidence. Не трактовать успешный cross-target type-check как выполнение этих проверок.
- **S2-30cl:** `D9 F8` добавлен в существующий native x87-deny regression matrix. После устранения compile blocker тест проходит Linux target type-check; runtime execution остаётся `not_run`. См. [S2-30cl report](reports/S2_30CL_X87_FPREM.md).
- **S2-30cm:** `D9 F5` добавлен в тот же Linux-only native x87-deny matrix. После устранения compile blocker тест проходит Linux target type-check; runtime execution остаётся `not_run`. См. [S2-30cm report](reports/S2_30CM_X87_FPREM1.md).
- **S2-37dj:** прежнее cross-target compile препятствие устранено. Тест denial для `ADDSD` m64 проходит Linux target type-check, но на Darwin arm64 host не запускался. См. [S2-37dj report](reports/S2_37DJ_ADDSD_MEMORY.md).

### ISSUE-028 — Неподтверждённый внешний report о CVTPI2PS memory x87 transition

- **Дата обнаружения:** 2026-10-31.
- **Компонент:** `od-isa` S2-37bg, bounded Long64 `CVTPI2PS XMM0,[RAX]` profile `s2-long-cvtpi2ps-memory-rax-v1`.
- **Статус:** открытое непроверенное сообщение о возможном расхождении; подтверждённый дефект не установлен.
- **Суть и последствия:** Intel SDM Rev. 093 Vol. 2A p. 3-224 предписывает для `CVTPI2PS` безусловно установить x87 TOP в 0 и все теги в valid после выполнения инструкции; bounded profile следует этому требованию и при memory source. Внешний QEMU issue [#4394](https://gitlab.com/qemu-project/qemu/-/work_items/4394), открытый Simon Scherer 1 сентября 2026 года, содержит standalone C/asm reproducer и сообщает о несовпадающих TOP/FTW наблюдениях при native и QEMU execution. Автор не указал CPU или hypervisor provenance; наше независимое physical reproduction не выполнялось. Поэтому аппаратное поведение и применимость наблюдения остаются непроверенными, а подтверждённый дефект не установлен.
- **Воспроизведение/основание:** сравнить правило Intel SDM с C/asm reproducer и заявленными native/QEMU TOP/FTW результатами в QEMU #4394. Локальные тесты проверяют spec-derived переход TOP=0/tag_word=0; они не заменяют physical CPU/hypervisor reproduction.
- **Следующий шаг:** при физической acceptance выполнить минимальную проверку memory-source CVTPI2PS на документированном Intel CPU и сохранить CPU/microcode, hypervisor/bare-metal, режим, точные байты и исходный/итоговый TOP/tag state. До получения evidence сохранять SDM-derived локальную семантику и не заявлять аппаратное подтверждение.

### ISSUE-022 — S2-19j address-size-32 REP LODS использовал RCX вместо ECX

- **Дата обнаружения:** 2026-10-27.
- **Компонент:** `od-isa` bounded Long64 `REP LODS` с address-size 32 (S2-19j).
- **Статус:** решена 27 октября 2026 года.
- **Суть и последствия:** цикл использовал полный RCX для проверки count и decrement. При `67` архитектурный count register — ECX: высокие 32 бита не должны запускать повторение; zero ECX выполняет ноль итераций и сохраняет RCX, а decrement после успешной итерации записывает zero-extended ECX в RCX. Старое поведение могло читать память при ECX=0, ошибочно продолжать цикл из-за высоких битов и оставлять неверный остаток после итерации.
- **Основание:** Intel SDM REP pseudocode выбирает ECX для address-size 32 и исполняет decrement только внутри `WHILE CountReg != 0`; локальные unit и literal schema-v2 cases проверяют zero count, успешный decrement, resume и no-commit memory fault. См. [CPU contract](specs/CPU_CONTRACT.md#local-s2-19j-bounded-rep-lods-address-size-32-contract), [plan](plans/STAGE_2.md#s2-19j-bounded-rep-lods-с-address-size-32), [report](reports/STAGE_2_REPORT.md#s2-19j--bounded-rep-lods-address-size-32) и [fixture notes](../tests/fixtures/x86/stage2/README.md#bounded-rep-lods-address-size-32-fixtures-s2-19j).
- **Результат:** исправлен только `Operation::RepLods`; address-size-64 и legacy REP LODS семантика не менялась. Целевые и workspace проверки, offline all-target check, formatting, fixture freshness, diff check и Graphify update подтверждены в S2-19j correction report. Hardware, native/JIT, Mac mini и SSH проверки не выполнялись.
- **Следующий шаг:** отсутствует для локальной семантической ошибки; физическая/native acceptance остаётся в общем финальном substage и не является доказательством, полученным этой правкой.

### ISSUE-018 — Специальные значения и исключения indexed FCOMI-family вне bounded профиля

- **Дата обнаружения:** 2026-10-24.
- **Компонент:** `od-isa` S2-30an, indexed DB/DF FCOMI/FUCOMI/FCOMIP/FUCOMIP.
- **Статус:** открыта; текущий bounded profile поддерживает только canonical-Normal ordered operands и не моделирует x87 exception/CR0 delivery или physical native x87 behavior.
- **Суть и последствия:** расширенные формы `i=0..7` декодируются и исполняются в явно ограниченном профиле. QNaN/SNaN comparison/arithmetic behavior, denormal/infinity arithmetic or other use outside the canonical and pseudo-denormal `FLD m80fp` DB 28 load slices S2-31aa/ad, other unsupported encodings, exception masks/sticky status, traps и hardware-visible native x87 state не подтверждены; поведение вне profile остаётся Unsupported и не должно трактоваться как полная архитектурная реализация.
- **Основание:** interpreter matrices, exact schema coverage, six literal CLI fixtures, workspace tests/checks и Linux-target compile-only check для S2-30an; см. [контракт](specs/CPU_CONTRACT.md#local-s2-30an-indexed-bounded-fcomi-family-register-forms), [план](plans/STAGE_2.md#s2-30an-bare-indexed-fcomi-family-register-forms), [отчёт](reports/STAGE_2_REPORT.md#s2-30an--indexed-fcomi-family-register-forms) и [fixtures](../tests/fixtures/x86/stage2/README.md).
- **Следующий шаг:** при расширении x87 профиля сверить unordered/special-value и exception delivery semantics по Intel SDM revision 093, добавить отдельные state/exception vectors и физическую/native acceptance; до этого сохранять narrow canonical-Normal contract и native deny.

### ISSUE-017 — Нет полного x86 LOCK ordering для общего memory backend

- **Дата обнаружения:** 2026-10-23.
- **Компонент:** `od-isa::GuestMemory`, S2-21f `LOCK XADD qword [memory], register`.
- **Статус:** открыта; S2-21f ограничен private synchronous RAM при последовательном исполнении. Shared/asynchronous memory, DMA, MMIO и SMP use остаются вне контракта.
- **Суть и последствия:** `atomic_fetch_add_integer` задаёт один backend вызов, wrapping update и no-mutation-on-error, но не задаёт полного x86 LOCK ordering, alignment/split-lock policy и не проверяет capability backend. Это достаточно только при отсутствии внешних конкурентных читателей/писателей и отложенных эффектов; generic `GuestMemory` не может динамически гарантировать это precondition. Unaligned local fixture проверяет backend bytes, но не hardware `#AC`/split-lock behavior.
- **Уточнение S2-21g:** обязательный `GuestMemory::mfence` completion call покрывает только явный MFENCE для opt-in private synchronous RAM. Он не служит ordering boundary для LOCK XADD и не закрывает эту проблему; S2-21f остаётся под тем же integration precondition.
- **Уточнение S2-22g:** valid A=0 descriptor для локального `MOV FS, AX` профиля обновляется одним `GuestMemory::write` длиной 1 по access byte после чтения/проверок. Это семантический byte-sized backend access, а не утверждение о физической ширине bus transaction. Он допускается только для private synchronous RAM при serial interpreter execution без внешних наблюдателей/писателей, DMA, shared aliases, delayed writes или MMIO. Срез полагается на общий контракт no-mutation-on-error и сам по себе не задаёт полного x86 LOCK ordering или SMP semantics.
- **Уточнение S2-22h/i:** тот же узкий A=0 byte write добавлен только для bare `MOV GS, AX` на CPL3 в отдельном local-only GDT profile; S2-22i добавил один local-only последовательный data read через загруженную GS.base. Оба сохраняют private synchronous RAM и serial-execution preconditions и не закрывают общий LOCK ordering или SMP semantics.
- **Основание:** архитектурный анализ S2-21f сопоставил [memory trait](../crates/od-isa/src/memory.rs), [архитектурный контракт памяти](ARCHITECTURE.md) и Intel SDM revision 093 LOCK/XADD semantics. `FlatMemory` тестирует local split-region behavior, но не подтверждает межпоточную, DMA, MMIO или hardware атомарность.
- **Воспроизведение/условие:** использовать S2-21f `F0 48 0F C1 /r` через `GuestMemory`, который имеет concurrent readers/writers, shared aliases, DMA, MMIO или отложенные writes; interpreter не проверит backend capability и не обеспечивает требуемый ordering.
- **Следующий шаг:** до подключения такого backend определить memory ownership/capability и complete x86 LOCK ordering contract, затем добавить concurrent/ordering tests и отдельную native/physical acceptance. До этого сохранять S2-21f и S2-22g/h/i memory ownership preconditions; не расширять LOCK формы или descriptor-write profile на shared, asynchronous, MMIO или SMP memory.

### ISSUE-015 — Разногласие SDM по NT для POPF word на CPL3/IOPL0

- **Дата обнаружения:** 2026-10-23.
- **Компонент:** `od-isa` S2-17r, long-mode word `POPF` (`66 9D`) в CPL3/IOPL0.
- **Статус:** открыта; входное значение NT=1 возвращает `Unsupported`, архитектурное поведение не заявлено.
- **Суть и последствия:** Intel SDM revision 093 Vol.2B Table 4-15 и Vol.1 §7.3.13.2 p.7-22 расходятся относительно загрузки NT bit для 16-bit POPF при CPL3/IOPL0. Чтобы не закреплять семантику на основании противоречащих нормативных разделов, реализация отклоняет до commit как source NT=1, так и начальное NT=1; NT=0 поддерживается. Это сужает принятый профиль и исключает физическое утверждение для NT=1.
- **Основание:** сверка Intel SDM order number 253667-093US, revision 093, Vol.2B Table 4-15 с Vol.1 §7.3.13.2 p.7-22 при реализации S2-17r. Literal fixture `s2_17r_popfw_updates_allowed_flags` использует TF=0 и NT=0 и не разрешает разногласие.
- **Следующий шаг:** найти авторитетную errata/уточнение Intel либо надёжное дополнительное нормативное свидетельство. До этого сохранять `Unsupported` для NT=1 и не расширять fixture/native profile.

### ISSUE-016 — Не установлен restart state MOVS после destination fault

- **Дата обнаружения:** 2026-10-23.
- **Компонент:** `od-isa` S2-18a/S2-18b/S2-18c/S2-18d, long-mode non-REP `MOVSB` (`A4`), `MOVSW` (`66 A5`), `MOVSD` (`A5`) и `MOVSQ` (`REX.W A5`) with implicit `[RSI]`/`[RDI]`; S2-18o adds non-REP address-size-32 `MOVSB/W/D/Q` (`67 A4`, `67 66 A5`, `67 A5`, `67 48 A5`). Also S2-18e bare STOSB and S2-18l address-size-32 STOSB/W/D/Q.
- **Статус:** открыта только для non-REP destination-fault snapshots; это непроверенная архитектурная граница, а не подтверждённый дефект runtime.
- **Основание и последствия:** Intel SDM revision 093, Vol.2B MOVS задаёт `DEST ← SRC`, RSI/RDI и шаг по DF (1/2/4/8), но не устанавливает полный snapshot non-REP destination `#PF` после успешного source read. S2-18o локально читает полный source operand, затем выполняет одну width-sized destination write, коммитит оба index32 только после успеха; destination fault оставляет source read в trace, но не коммитит CPU state. При этом для данного bounded `GuestMemory` write failure не меняет RAM. Это local memory-model policy under ISSUE-016, not hardware restart, MMIO rollback, or LOCK/SMP atomicity claim. STOS similarly commits its index only after write success. REP suspension is separately specified in the SDM between completed iterations and is tracked by S2-19a. Fault-only fixtures remain local and are rejected by native admission.
- **Воспроизведение/условие:** preserve existing bare MOVS `*_write_fault` fixtures from S2-18a–d and `s2_18e_stosb_write_fault`; S2-18l is covered by `stos_address32_write_errors_leave_full_state_and_memory_uncommitted`. S2-18o exercises source/destination outcomes in `movs_address32_source_and_destination_errors_commit_no_cpu_or_ram_state`, all-or-error range failure in `movs_address32_partial_destination_fault_does_not_mutate_a_prefix`, and 73 local fixture cases including contiguous cross-4-GiB writes. These host results are not physical expectations.
- **Следующий шаг:** find an unambiguous normative/errata source or obtain a reliable post-fault snapshot during the shared final Mac mini acceptance substage. Until then, keep the local commit policy and do not claim the hardware RSI/RDI/RIP state after a string destination `#PF`.

### ISSUE-014 — Не установлено состояние RSP после fault у memory-destination POP

- **Дата обнаружения:** 2026-10-23.
- **Компонент:** `od-isa` S2-17j/S2-17l, long-mode qword `POP r/m64` and word `POP r/m16` (`8F /0`) с ordinary memory destination.
- **Статус:** открыта; нормативный post-fault CPU state не подтверждён, не установленный дефект runtime.
- **Основание и последствия:** Intel SDM order number 253667-093US, revision 093, Vol.2B, `POP` pp. 4-398–4-402 описывает чтение значения со старого stack top, увеличение RSP на размер операнда до записи в memory destination и вычисление адреса destination после увеличения, если RSP используется как base. Раздел перечисляет `#PF` для memory destination, но не устанавливает, обязано ли увеличенное RSP сохраняться при fault destination store. Executor вычисляет EA на staged state после `RSP += 8` для qword и `RSP += 2` для word, публикуя staged state только после успешной записи; это локальная fault policy, не подтверждённое архитектурное postcondition. Store-`#PF` regressions намеренно проверяют exception и неизменившуюся RAM, но не сравнивают GPR/RIP; native admission отвергает fault-only fixtures.
- **Воспроизведение/условие:** `8F /0` или `66 8F /0` с читаемым значением на старом `[RSP]` и memory destination, который вызывает store `#PF`. Вызвать `step` с инъекцией architectural write fault в `FlatMemory`; проверить #PF и неизменившуюся память. CPU post-state не сравнивать до уточнения нормативного поведения.
- **Следующий шаг:** найти однозначное Intel/AMD normative или errata evidence либо получить надёжный физический x86 snapshot в общем финальном Mac mini substage. До проверки не закреплять архитектурное ожидание для RSP/RIP/GPR при destination-store fault. Успешный `[RSP]` путь использует post-increment EA согласно SDM; эта успешная семантика не затронута проблемой. Контекст реализации: [S2-17j report](reports/STAGE_2_REPORT.md#s2-17j--qword-memory-destination-pop-local), [S2-17l report](reports/STAGE_2_REPORT.md#s2-17l--16-bit-memory-destination-pop-local), [CPU contract](specs/CPU_CONTRACT.md) и [plan](plans/STAGE_2.md).

### ISSUE-013 — Не подтверждён приоритет target `#GP` и stack fault для near CALL

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `od-isa` S2-17c/d/e, near relative `CALL rel32` (`E8`) и near indirect `CALL r/m64` (`FF /2`) с register или ordinary memory source, включая порядок source/target/stack fault checks.
- **Статус:** открыта; непроверенный приоритет combined fault, не подтверждённый дефект.
- **Основание и последствия:** executor проверяет canonicality target перед вызовом stack helper для `E8 rel32` и `FF /2`; memory `FF /2` перед этим читает qword source. Отдельные source-read fault, noncanonical-target `#GP` и stack-fault cases проходят. Для случая, когда near CALL получил noncanonical target и одновременно invalid/noncanonical guest stack, архитектурный победивший fault не установлен. Intel SDM revision 093, Vol.2A, `CALL` p.3-125 описывает operation pseudocode через временный RIP, stack capacity, push и обновление RIP, а `#GP` для target перечисляет отдельно; эта подача сама по себе не подтверждает приоритет combined fault для direct, register-indirect или memory-indirect формы. Independent Sol-medium reviews для S2-17d/e чистые; S2-17e локально проверяет отдельные source/target/stack faults без commit; combined dual-fault fixture и expected physical snapshot пока не созданы.
- **Воспроизведение/условие:** выполнить `E8 rel32`, register-only `FF /2` или memory `FF /2` (читаемый source содержит noncanonical qword) при одновременно invalid/noncanonical guest stack. Для memory CALL текущий executor сначала читает source, затем проверяет target canonicality и stack helper; наблюдаемый порядок реализации не устанавливает нормативный expected winner.
- **Следующий шаг:** найти нормативное или errata evidence; иначе проверить возможность надёжно захватить combined-fault result для direct/register/memory indirect CALL в harness, создать dual-fault fixture с expected state и включить её в общий финальный physical Mac mini acceptance evidence. До подтверждения не менять runtime по догадке. Контекст — [S2-17e report](reports/STAGE_2_REPORT.md#s2-17e--memory-near-indirect-call-local), [CPU contract](specs/CPU_CONTRACT.md) и [plan](plans/STAGE_2.md).

### ISSUE-012 — Не подтверждена семантика operand range через верх u64

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `od-isa` S2-08 full-width address validation диагностического long-mode профиля; S2-34.5 legacy `MOVUPS [RAX],XMM` также сохраняет ту же явную overflow policy.
- **Статус:** открыта; непроверенная semantic boundary, не подтверждённый hardware bug.
- **Основание и последствия:** проверка Intel SDM093 Vol.3A, §§4.5.1 и 6.15, не дала однозначного нормативного ответа для многобайтного доступа, у которого endpoint переполняет u64. Поэтому реализация возвращает `MemoryError::AddressOverflow` до backend и без commit; она не выдаёт неподтверждённый #GP. Это harness/memory-boundary error, не установленная x86 exception semantics.
- **Воспроизведение:** в диагностическом long-mode профиле задать `RAX = 0xfffffffffffffff8` и исполнить legacy `MOVUPS [RAX],XMM0` (`0f 11 00`); старт canonical, endpoint 16-byte operand непредставим u64. Проверить `AddressOverflow`, сохранённый CPU state и отсутствие обращения к backend. Qword пример `RAX = 0xffffffffffffffff`, `48 8b 08` остаётся исходным воспроизведением для S2-08.
- **Следующий шаг:** найти точный нормативный источник или независимое подтверждение на физическом x86 для этого диапазона, затем закрепить architectural outcome и regression. Закрыть до полной приёмки этапа 2; локальная проверка выбранного boundary policy не заменяет hardware confirmation. Контекст — [CPU contract](specs/CPU_CONTRACT.md#исполнение-mov--s2-08) и [S2-34.5](specs/CPU_CONTRACT.md#local-profile-s2-345-s2-long-movups-mem-store-v1).

### ISSUE-011 — AMD APM 4.09 не прочитан через web fetch

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** нормативные источники CPU, S2-00; проверка AMD semantics.
- **Статус:** открыта; ограничение доступа к источнику в текущей среде.
- **Основание и последствия:** повторный `web.run.open` вернул `Failed to fetch … (404) Not Found` для [страницы 40332 revision 4.09](https://docs.amd.com/v/u/en-US/40332_4.09_APM_PUB), [официального PDF](https://www.amd.com/content/dam/amd/en/documents/processor-tech-docs/programmer-references/40332.pdf) и [indexed content API](https://docs.amd.com/api/khub/documents/68GKiN0gMEd6bMddsmhPwg/content). Поиск показывает metadata/выдержки, но тело manual через этот инструмент не прочитано; они не считаются проверкой нормативной семантики. Наблюдение не доказывает общую недоступность сайта или отсутствие документа.
- **Повторная ограниченная проверка:** 22 октября `web.run.open` снова вернул 404 для landing 4.09, khub content, отдельного официального Volume 3 `24594.pdf` и Volume 1 khub. Другой HTTP fetch получил для landing только SPA shell «Loading application…»; после загрузки в Chrome официальный портал показал «Content not found» с сообщением о перемещённом/удалённом документе именно по этому URL. Поиск портала `40332` результата в наблюдаемой странице не дал. Отдельный `urllib.request.urlopen` GET официального [24594.pdf](https://www.amd.com/content/dam/amd/en/documents/processor-tech-docs/programmer-references/24594.pdf), предназначенный для обработки bytes в памяти и `pdftotext - -`, завершился `TimeoutError: The read operation timed out` с лимитом 25 s до получения PDF. PDF bytes/hash и текст разделов не получены; search snippets не использованы вместо manual. Это разные наблюдаемые границы инструментов/URL, не вывод об универсальной недоступности AMD.
- **Следующий шаг:** получить официальный AMD PDF revision 4.09 через доступный официальный канал, проверить титул/version и необходимые разделы, сохранить provenance/hash и обновить [CPU contract](specs/CPU_CONTRACT.md) и [отчёт](reports/STAGE_2_REPORT.md). До этого новые vendor-specific решения считать непроверенными по AMD; уже реализованный GPR contract подтверждён Intel SDM.

### ISSUE-010 — graphify не извлекает символы публичного C ABI header

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** производный граф `graphify-out/`, C extractor инструмента graphify.
- **Статус:** открыта; ограничение инструмента навигации, не подтверждённый дефект C header.
- **Основание и последствия:** `graphify update .` завершился успешно, но сообщил syntax error на строке 68 `include/open_desktop/host_abi.h` и отсутствие извлечённых символов этого файла. Поэтому граф недостаточен для полного анализа связей публичного ABI. Успешный workspace/VMHost build не даёт оснований считать эту диагностику ошибкой компиляции исходника.
- **Воспроизведение:** выполнить `graphify update .` на текущем checkout. Граф после ревью содержит 1184 nodes и 2363 edges; предупреждение о header сохранилось.
- **Следующий шаг:** проверить поддержку используемых C macros/атрибутов в extractor и исправить или обновить инструмент. До этого для ABI-ревью дополнительно читать нормативный header и проверять штатным компилятором. Не менять ABI только ради устранения предупреждения graphify.

### ISSUE-009 — Дефекты валидации, сохранения результатов и reducer этапа 1

- **Дата обнаружения:** 2026-10-22.
- **Компоненты:** `od-testkit::{case,result,minimize}`, `tests/cpu/stage1_oracle.py`, `stage1_harness.py`.
- **Статус:** решена 22 октября 2026 года.
- **Суть и последствия:** oracle использовал непроверенные ID как пути и не отклонял конфликтующие ID/basename; batch терял вывод при native failure, а входы не фиксировались перед запуском. Generated corpus заменял ручные cases. Tagged JSON enums игнорировали лишние поля. Reducer принимал разовый flaky mismatch, не переносил ненулевой entry RIP и превышал лимит callback при значениях 0/1.
- **Воспроизведение и исправления:** семь находок и отдельные regression tests описаны в [ревью этапа 1](reports/STAGE_1_REVIEW_2026-10-22.md). Первые четыре Rust regressions воспроизведены до исправления. Точные входы и process outputs теперь сохраняются; валидация выполняется до использования ID в путях; reducer подтверждает каждого кандидата дважды и переносит entry.
- **Результат проверки:** 31 testkit test на обоих hosts, 6 Python regressions, local/workspace/fixture checks и VMHost smoke прошли. Объединённый физический прогон: 1010 уникальных cases × 2 повтора, 2020 native snapshots, ноль расхождений; [evidence](reports/data/stage1/review-2026-10-22.json).
- **Следующий шаг:** отсутствует для перечисленных дефектов; ограничения register-only профиля сохранены в [CPU_ORACLE](testing/CPU_ORACLE.md).

### ISSUE-008 — Нет Rust toolchain и checkout на физическом x86 host

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `tools/x86-runner`, физическая проверка S1-05/S1-06 и подтверждение Linux-сборки S1-04.
- **Статус:** решена 22 октября 2026 года.
- **Суть и последствия:** исходное отсутствие toolchain и checkout мешало проверить Linux `setrlimit`/watchdog и запускать нативные fixtures. Владелец разрешил установить toolchain и скопировать checkout в отдельный каталог.
- **Основание:** Rust stable 1.99.1 установлен в пользовательский каталог; checkout скопирован в `~/projects/open_desktop-stage1`. На Mac mini прошли Linux workspace check, тесты целевых crates, Linux `setrlimit` regression test и сборка `od-x86-runner`. Затем S1-06 автоматически исполнил 10 cases × 2 повтора на физическом Intel CPU; подробности и доказательства находятся в [CPU_ORACLE](testing/CPU_ORACLE.md), [STAND](testing/STAND.md) и [плане этапа 1](plans/STAGE_1.md).
- **Следующий шаг:** отсутствует; устранённое условие закрыто. Ответственный: инженер runner.

### ISSUE-005 — RUN_ACK опережает готовность VMHost к STOP

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `apps/vmhost`, запуск `FragmentWorker` и C ABI lifecycle.
- **Статус:** решена 22 октября 2026 года.
- **Суть и последствия:** успешный RUN_ACK отправляется сразу после постановки worker в DispatchQueue. C ABI теперь сохраняет STOP, пришедший до начала исполнения, и сбрасывает флаг после завершения run, поэтому остановка не зависит от планирования worker.
- **Воспроизведение:** штатный launcher с `--hex 75fe --budget 18446744073709551615 --stop-after-ms 0`; на M3 Max в 5 из 100 запусков получен `VMHost returned status 6`. Подробности и сборка — в [ревью S0](reports/STAGE_0_REVIEW_2026-10-22.md).
- **Результат:** `od_fragment_request_stop` сохраняет ранний запрос для следующего run; [control-race smoke](../tests/system/vmhost_control_race_smoke.py) прошёл 100/100 запусков немедленного STOP с reason 2. Отключение канала после RUN_ACK также проверено: VMHost остановил worker и завершил процесс.

### ISSUE-006 — Неполный control frame блокирует готовый RESULT

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `apps/vmhost/Sources/main.swift`, `ControlFrame.read`.
- **Статус:** решена 22 октября 2026 года.
- **Суть и последствия:** POLLIN означает наличие хотя бы части кадра; блокирующий read всего кадра переставал обслуживать completion pipe. VMHost теперь неблокирующе накапливает данные и декодирует только полные кадры, продолжая опрашивать completion.
- **Воспроизведение:** после RUN_ACK передать один байт `O` следующего header и держать socket открытым. Для конечного budget=2000000 RESULT не пришёл за 2 s, тогда как контроль без частичного кадра завершился за 0.021 s со стартом процесса. Полные входы — в [ревью S0](reports/STAGE_0_REVIEW_2026-10-22.md).
- **Результат:** [control-race smoke](../tests/system/vmhost_control_race_smoke.py) подтверждает доставку RESULT при незавершённых header и payload; проверка отключения канала также проходит.

### ISSUE-007 — Launcher теряет результат при завершении до отложенного STOP

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `apps/macos/Sources/launcher.swift`, ожидание `--stop-after-ms`.
- **Статус:** решена 22 октября 2026 года.
- **Суть и последствия:** launcher раньше спал до таймера STOP и затем безусловно отправлял запрос, теряя RESULT, если helper естественно завершился первым. Launcher теперь ожидает socket и срок STOP одновременно, затем принимает RESULT без ACK или STOP_ACK перед RESULT.
- **Воспроизведение:** `--hex 48b80100000000000000 --stop-after-ms 25` возвращает exit=1 и `control frame write failed`. Подтверждено на свежих release smoke-бинарниках; [ревью S0](reports/STAGE_0_REVIEW_2026-10-22.md).
- **Результат:** точное воспроизведение с задержкой 25 ms теперь завершается reason 0; 20 запусков с нулевой задержкой завершаются корректно с reason 0 либо reason 2. Допустимые порядки описаны в [CONTROL_PROTOCOL_V0.md](../protocols/control/CONTROL_PROTOCOL_V0.md).

### ISSUE-001 — Корневые пользовательские каталоги и Finder metadata

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** структура репозитория, [карта проекта](PROJECT_LAYOUT.md).
- **Статус:** решена 22 октября 2026 года.
- **Проблема и последствия:** проверка структуры обнаруживает не описанные в нормативной карте объекты корня: файлы `.DS_Store` и каталоги `.codex/`, `.opencode/`, `graphify-out/`, `iso/`. Из-за этого `check_layout.py` завершается ошибкой, хотя часть объектов может быть локальной конфигурацией или временными данными.
- **Воспроизведение:** из корня проекта запустить `python3 tools/check_layout.py`. На 22 октября 2026 года получены `ERROR: Unmapped root file: .DS_Store` и `ERROR: Unmapped root directory` для `.codex`, `.opencode`, `graphify-out`, `iso`.
- **Результат:** в [карте проекта](PROJECT_LAYOUT.md) перечислены допустимые необязательные каталоги `.codex/`, `.opencode/`, `graphify-out/` и `iso/`; layout checker учитывает их как локальные/производные данные и пропускает `.DS_Store`. Файлы и каталоги не перемещались и не удалялись. `python3 tools/check_layout.py` после изменения прошёл.

### ISSUE-002 — Задержка VMHost RUN→RESULT из-за polling worker completion

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `apps/vmhost` control loop и VMHost completion signaling.
- **Статус:** решена 22 октября 2026 года для короткого VMHost RUN→RESULT path; длительная нагрузка и отдельный STOP latency требуют дальнейшей проверки.
- **Проблема и последствия:** первоначальный loop ожидал worker completion через `poll()` control socket с timeout 50 ms. Один короткий `MOV RAX,1` integration run дал медианный RUN→RESULT 51.33 ms [51.16–51.61] на 20 fresh VMHost processes.
- **Основание:** [S0-13 CPU baseline report](reports/STAGE_0_CPU_BASELINE_2026-10-22.md), раздел VMHost integration pilot; raw samples до/после в локальном `work/stage-0/2026-10-22-e04-vmhost-abi-ipc-resource/`. Все 40 процессов вернули ожидаемые `RAX=1`, `RIP=10`, exit reason 0 и instruction count 1. Источник timeout и реализация completion signal: `apps/vmhost/Sources/main.swift`, `serviceStopRequests()` и `FragmentWorker`.
- **Результат:** VMHost теперь одновременно ждёт control socket и completion pipe без периодического timeout. В повторе из 20 запусков медиана RUN→RESULT составила 138.1 μs [114.5–479.7], примерно в 372 раза меньше исходного пилота. Повторные launcher и isolation smoke прошли. Изменение и ограничения измерения описаны в [S0-13 baseline report](reports/STAGE_0_CPU_BASELINE_2026-10-22.md).
- **Ограничения и следующий шаг:** сравнение было одним коротким сеансом при неконтролируемых условиях и не является изолированной мерой IPC или результатом длительной нагрузки. После исправления отдельный busy-helper pilot измерил STOP_ACK и STOP→RESULT; значения и ограничения описаны в S0-13 report. Для финального решения повторить с контролируемым фоном и длительной нагрузкой.

### ISSUE-003 — Временный SSH-доступ к M1 Pro для JIT smoke

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** внешний контрольный Mac M1 Pro, проверка S0-08/E01.
- **Статус:** решена 2026-10-22 для разрешённого smoke-прогона.
- **Суть и последствия:** исходный SSH-вход без ключа завершился отказом аутентификации, поэтому запуск на M1 Pro зависел от настройки временного ключа.
- **Основание:** по запросу владельца SSH-хост достигнут 22 октября; после явного согласия владельца на передачу трёх smoke-файлов использован временный Ed25519 ключ. Приватный ключ не попадал в репозиторий.
- **Результат:** на M1 Pro (macOS 14.6.1, MacBookPro18,3) прошли arithmetic JIT, cooperative STOP и process-isolation smoke. SHA-256 и ограничения опыта записаны в [STAND.md](testing/STAND.md); статус S0-08 — в [плане](plans/STAGE_0.md) и [отчёте](reports/STAGE_0_REPORT.md).
- **Очистка доступа:** после проверок приватный ключ на основном Mac и каталог с тремя переданными smoke-файлами на M1 удалены. Владельцу остаётся удалить строку с комментарием `open-desktop-stage0-m1-temp` из `~/.ssh/authorized_keys` на M1.

### ISSUE-004 — S0-08: Developer ID distribution и проверка конечного bundle

- **Дата обнаружения:** 2026-10-22.
- **Компонент:** `od-host`, VMHost supervisor и путь поставки, S0-08/E01.
- **Статус:** открыта; локальные fault-diagnostics закрыты, но этап 0 не принят до проверки пути выпуска.
- **Суть и последствия:** unit test подтверждает fault в generated code внутри дочернего процесса, а protocol-level test helper подтверждает `SIGSEGV` диагностику launcher-а. Временная ad-hoc `.app` smoke-упаковка валидна и исполняет JIT, но не подтверждает Developer ID distribution или notarization. Владелец сообщает о наличии Developer ID certificate, но соответствующая signing identity (сертификат с доступным private key) не найдена в keychains проверенных M3 Max и M1 Pro. Членство Apple Developer Program не активно; Apple notary service недоступен до его возобновления.
- **Основание:** проверки `generated_code_fault_is_confined_to_child_process`, `vmhost_generated_fault_smoke.py` и `vmhost_app_bundle_smoke.sh` прошли на M3 Max. Инвентаризация подписи и пределы теста записаны в [STAND.md](testing/STAND.md) и [отчёте этапа 0](reports/STAGE_0_REPORT.md).
- **Следующий шаг:** установить на M3 Max действующую Developer ID Application identity вместе с соответствующим private key и проверить подписание конечного Open Desktop bundle. Если требуется проверить notarization, владелец должен возобновить членство Apple Developer Program и предоставить авторизацию для notary service; учетные данные не передавать через чат. Затем повторить S0-14 review. До появления доступной identity Developer ID signing не проверено, а без активного членства notarization недоступна. Ответственный: владелец конфигурации подписи совместно с инженером macOS.

### ISSUE-018 — S2-22 покрывает только ограниченные selector/descriptor slices

- **Дата обнаружения:** 2026-10-23.
- **Компонент:** `od-isa` long-mode сегментная модель и S2-22.
- **Статус:** открыта; это зарегистрированный остаток этапа, не подтверждённый дефект локальных S2-22 slices.
- **Суть и последствия:** S2-22a–k задают локальные профили для отдельных FS/GS base/selector операций, CPL3 VERW, ограниченных `MOV FS, AX` / `MOV GS, AX` загрузок из GDT с A-bit update только в private synchronous RAM, точных FS/GS.base composition cases и bare null `MOV DS/ES, AX` при Long64/CPL0 с AX=0. S2-22k моделирует только видимый selector и unusable state, без descriptor lookup. Эти срезы не реализуют LDT, другие null-segment загрузки, общие accessed-bit updates, остальные GS/SS/DS/ES loads, stack checks, другие CPL/modes или применение protected-mode limit/type checks. Поэтому Stage 2 не может объявить полную сегментную архитектуру или завершить S2-22 по результатам этих профилей.
- **Основание:** ограничения зафиксированы в [плане S2-22](plans/STAGE_2.md), [CPU contract](specs/CPU_CONTRACT.md), [interpreter guide](testing/INTERPRETER.md), [fixture notes](../tests/fixtures/x86/stage2/README.md) и [отчёте этапа 2](reports/STAGE_2_REPORT.md). Локальные fixture/unit checks подтверждают только перечисленные формы.
- **Следующий шаг:** после утверждения обязательного guest profile в S2-00 выбирать отдельные bounded slices с собственным fault/state matrix и review. Не расширять текущие профили автоматически на остальные формы.

### ISSUE-019 — Семантика арифметики и инструкций x87 остаётся открытой

- **S2-29r unequal-exponent helper — bounded gap closed:** exact finite addition
  now accepts two normalized nonzero binary80 operands whose exponents differ
  by at most 62, forms an exact signed `i128` coefficient at the lower exponent,
  and rounds once through the existing packer. The unequal-only helper rejects
  equal exponents, zero operands, and gaps above 62; the composed API still
  accepts same-exponent values and treats one zero as neutral. Two zeros,
  cancellation, and finite overflow retain bounded `None` outcomes. This adds
  no FADD/FSUB instruction semantics or guest exception behavior. Review
  was clean; targeted tests, all 1,682 `od-isa` tests and workspace check passed.
  The isolated worktree layout check was unavailable for its `.git` pointer and
  absent local oracle manifests, with no layout changes. ISSUE-019 remains open
  for architectural x87 semantics and physical acceptance. See the [plan](plans/STAGE_2.md#s2-29r-exact-unequal-exponent-finite-binary80-addition-bounded-helper),
  [contract](specs/CPU_CONTRACT.md#local-s2-29r-exact-unequal-exponent-finite-binary80-addition),
  and [report](reports/S2_29R_X87_UNEQUAL_EXPONENT.md).

- **S2-29u p64-aware large-gap addition bridge — bounded helper complete:**
  `add_finite_to_binary80` now handles normalized nonzero binary80 operands with
  exponent gaps above 64 by classifying the tail below/at/above half at p=64;
  gaps through 64 keep the exact alignment path. The binary32/binary64 adapter
  retains its earlier results and diagnostics. Focused large-gap, compatibility,
  zero/cancellation and immutability tests passed; full `od-isa` passed 1,699,
  workspace all-target check, format, diff check, and Graphify update passed.
  Independent Sol/high review was clean. This is helper evidence only: it adds
  no guest x87 instruction semantics or physical x86 acceptance. ISSUE-019 stays
  open. See the [plan](plans/STAGE_2.md#s2-29u-p64-aware-large-gap-binary80-addition-bridge-bounded-helper),
  [CPU contract](specs/CPU_CONTRACT.md#local-s2-29u-p64-aware-binary80-large-gap-addition),
  and [report](reports/STAGE_2_REPORT.md#s2-29u--p64-aware-large-gap-binary80-addition-bridge).

- **S2-29v finite square-root helper — bounded helper complete:**
  `sqrt_finite_to_binary80` computes positive nonzero normalized finite binary80
  square roots with integer arithmetic and one 64-bit significand rounding.
  The exact remainder remains `u128`; literal ten-byte vectors cover all four
  rounding modes, the nearest threshold and `u64::MAX` carry, exponent
  boundaries, pseudo-denormal equivalence, and rejected zero/negative inputs.
  The relocated shared root/decomposition helpers preserve binary32/binary64
  paths, whose existing literal sqrt tests pass. Full `od-isa` tests (1706),
  component all-target check, formatting and Graphify update passed; independent
  Sol/high review was clean. This remains helper-only: no FSQRT execution, FCW,
  x87 state/status/exception semantics, or physical x86 acceptance is claimed.
  ISSUE-019 remains open for broader x87 behavior and physical acceptance. See
  the [plan](plans/STAGE_2.md#s2-29v-pure-finite-x87-square-root-to-binary80-bounded-helper),
  [CPU contract](specs/CPU_CONTRACT.md#local-s2-29v-pure-finite-x87-square-root-helper),
  and [report](reports/STAGE_2_REPORT.md#s2-29v--pure-finite-x87-square-root-helper).

- **S2-29w finite scaling helper — bounded helper complete:**
  `scale_finite_to_binary80` scales normalized nonzero finite values by an exact
  power of two and rounds once through the existing coefficient packer. Literal
  coverage checks signs and all mathematical modes, underflow and signed zero,
  subnormal/normal carry, pseudo-denormal canonicalization, exponent-add
  overflow, finite-range rejection, invalid inputs, and immutability. All 1,712
  `od-isa` tests, workspace all-target check, formatting, and independent
  Sol/high review passed. No FSCALE/FCW/FSW or guest exception behavior and no
  physical x86 acceptance are claimed. ISSUE-019 remains open; this helper did
  not establish a new issue. See the [plan](plans/STAGE_2.md#s2-29w-exact-finite-binary80-scaling-to-one-rounded-result-bounded-helper),
  [CPU contract](specs/CPU_CONTRACT.md#local-s2-29w-exact-finite-binary80-scaling-to-one-rounded-result),
  and [report](reports/STAGE_2_REPORT.md#s2-29w--exact-finite-binary80-scaling-to-one-rounded-result).

- **S2-29x exact truncated finite remainder — bounded helper complete:**
  `remainder_finite_trunc` returns an exact coefficient decomposition of
  `lhs - trunc(lhs / rhs) * rhs` for normalized nonzero finite operands and
  exponent gaps through 64. A smaller left operand is returned unchanged;
  larger gaps and malformed inputs are rejected. Literal coverage includes
  signed operands, exact zero, exponent-gap boundaries, normal/subnormal and
  pseudo-denormal inputs, `i32` exponent endpoints, and literal binary80
  composition. Focused tests passed 3/3, full `od-isa` passed 1,717 tests, and
  component/workspace checks, formatting, Graphify update, and independent
  Sol/high review passed. This helper does not implement FPREM/FPREM1, partial
  reduction/C2, quotient bits, FCW/FSW, guest exceptions, architectural
  signed-zero behavior, or physical x86 acceptance. ISSUE-019 remains open for
  architectural x87 behavior and physical acceptance. See the [plan](plans/STAGE_2.md#s2-29x-exact-truncated-finite-binary80-remainder-decomposition-bounded-helper),
  [CPU contract](specs/CPU_CONTRACT.md#local-s2-29x-exact-truncated-finite-binary80-remainder-decomposition),
  and [report](reports/STAGE_2_REPORT.md#s2-29x--exact-truncated-finite-binary80-remainder-decomposition).

- **S2-29y exact nearest-even finite remainder — bounded helper complete:**
  `remainder_finite_nearest_even` computes the exact nearest-integer-ties-even
  remainder coefficient for normalized nonzero Normal, Subnormal, and
  PseudoDenormal inputs, with a special d=-1 half-way case and `u128`
  quotient/remainder arithmetic for gaps through 64. Focused tests passed 4/4,
  all 1,724 `od-isa` tests passed, and formatting, component/workspace checks,
  and independent Sol/high review passed. This adds no FPREM1 or other ISA
  behavior, quotient condition flags, FCW/FSW effects, guest exceptions,
  architectural signed-zero choice, or physical x86 acceptance. ISSUE-019
  remains open. See the [plan](plans/STAGE_2.md#s2-29y-exact-finite-nearest-even-binary80-remainder-decomposition-bounded-helper),
  [CPU contract](specs/CPU_CONTRACT.md#local-s2-29y-exact-finite-nearest-even-binary80-remainder),
  and [report](reports/STAGE_2_REPORT.md#s2-29y--exact-finite-nearest-even-binary80-remainder-helper).

- **S2-29z finite remainder quotient metadata — bounded helper complete:** the
  truncating and nearest-even metadata APIs return the existing exact remainder
  plus unsigned `u128` quotient magnitude and separate sign; zero quotient is
  positive, and nearest-even reports the ties-to-even rounded integer quotient.
  Nearest-even preserves all negative gaps: `d <= -2` passes lhs through with
  quotient zero; `d == -1` returns zero for `A <= B` and one for `A > B`. The
  legacy helpers project these results and preserve their acceptance and
  rejection domains. Literal vectors cover values above `u64`, signed operands,
  zero quotient, exact multiples, odd/even ties, gaps through the accepted
  boundary, malformed/provenance rejects, `i32` exponent endpoints, and wrapper
  projection equivalence. The 13 focused remainder tests, all 1,741 `od-isa`
  tests, format check, component/workspace checks, and Graphify update passed.
  Layout check remains unavailable in this worktree for the pre-existing
  ISSUE-023 `.git` pointer and missing ignored oracle manifests. The initial
  Sol/high review's P2 negative-gap wording mismatch was corrected in the
  contract/report/helper docs; follow-up review is requested separately. This
  adds no FPREM/FPREM1, quotient flags,
  FCW/FSW or guest exception behavior, decoder/execution/native/JIT/C ABI
  admission, physical x86 acceptance, or performance claim. ISSUE-019 remains
  open. See [plan](plans/STAGE_2.md#s2-29z-exact-quotient-metadata-for-bounded-finite-binary80-remainders),
  [CPU contract](specs/CPU_CONTRACT.md#local-s2-29z-exact-quotient-metadata-for-bounded-finite-remainders),
  and [report](reports/STAGE_2_REPORT.md#s2-29z--exact-quotient-metadata-for-bounded-finite-binary80-remainders).

- **S2-29aa direct square-root precision rounding — bounded helper complete:**
  `sqrt_finite_to_binary80_precision` rounds positive normalized finite
  binary80 square roots directly to p=24/53/64 across the binary80 exponent
  range. The legacy helper remains the p64 projection with unchanged input
  acceptance/rejection; lower precision uses exact `(E,q,R)` decomposition,
  target-bit/discarded-tail classification, and the existing normal packer.
  Independent literal bytes cover all modes and precisions, p24 even/odd exact
  midpoint ties, adjacent p24/p53 boundaries without an exact p53 midpoint
  claim, exponent carry, minimum-subnormal/max-finite/negative-exponent inputs,
  pseudo-denormal equivalence, wrapper/domain equivalence, invalid precisions,
  immutability, and a direct-rounding versus double-rounding difference.
  Local validation and review status are recorded in the [standalone report](reports/S2_29AA_X87_SQRT_PRECISION.md).
  No FSQRT execution, FCW/x87 architectural state or exceptions, native/JIT,
  physical x86 acceptance, or performance claim is established. ISSUE-019 stays
  open for broader x87 semantics and physical acceptance.

- **S2-29ab finite square-root precision diagnostics — bounded helper complete:**
  `sqrt_finite_to_binary80_precision_with_diagnostics` returns the raw binary80
  value plus mathematical inexactness and target-precision increment metadata
  at p=24/53/64. Inexactness uses `R != 0` at p64 and `L != 0 || R != 0` at
  lower precision, so exact p24 midpoint roots with `R=0` remain inexact.
  `incremented` is the pre-carry increase of the truncated positive
  significand, not architectural C1; neither field represents x87 PE/C1.
  Existing bit-only APIs remain projections with their old results and `None`
  domains. The independent literals and validation/review record are in the
  [standalone report](reports/S2_29AB_X87_SQRT_DIAGNOSTICS.md). The helper adds
  no FCW/FSW, exception or instruction behavior, native/JIT/C ABI admission,
  physical x86 claim, or generic FP diagnostics framework. ISSUE-019 remains
  open for broader x87 behavior and physical acceptance.

- **S2-29ac finite division precision diagnostics — bounded helper complete:**
  direct target-precision finite division returns mathematical inexact and
  pre-carry magnitude-increment metadata for p24/53/64. It accepts only normal
  finite binary80 results, preserves the old helper's broader subnormal domain,
  and adds no guest instruction/status behavior. Independent literal tests,
  full `od-isa`, workspace check, formatting, Graphify and clean Sol/high review
  are recorded in [S2-29ac report](reports/S2_29AC_X87_DIVISION_PRECISION.md).
  ISSUE-019 remains open.

- **S2-29ad finite multiplication precision diagnostics — bounded helper complete:**
  direct full-product rounding returns mathematical inexact and pre-carry
  magnitude-increment metadata at p24/53/64 for both signs. The new API accepts
  only normal finite results; old `multiply_finite_to_binary80` and its
  subnormal-result domain remain unchanged. Independent literal tests, full
  `od-isa`, workspace check, formatting, Graphify and clean Sol/high review are
  recorded in [S2-29ad report](reports/S2_29AD_X87_MULTIPLICATION_PRECISION.md).
  ISSUE-019 remains open.

- **S2-29ae finite addition precision diagnostics — bounded helper complete:**
  exact finite binary80 addition rounds directly to p24/53/64 for gaps through
  64, with mathematical inexact and pre-carry magnitude-increment metadata.
  Exact cancellation and non-Normal outputs return `None`; the old addition
  helper retains its zero-neutral, subnormal and larger-gap domain. Full
  `od-isa`, workspace checks and independent Sol/high review passed. A first
  subnormal-output reject vector was corrected after review to use normalized
  Normal inputs; both the new helper rejection and old-helper min-subnormal
  result now pass. See [S2-29ae report](reports/S2_29AE_X87_ADDITION_PRECISION.md).
  The initial gap≤64 bound was extended in S2-29af; this entry records the
  original slice and checks. ISSUE-019 remains open.

- **S2-29af large-gap addition precision diagnostics — bounded helper complete:**
  the precision-aware finite addition API now handles gaps above 64 with the
  existing large-gap bridge. The exact-u128 path for gaps≤64 retains its old
  bytes and diagnostics. Large-gap accepted results are inexact; zero,
  cancellation, out-of-range exact exponents, rounded overflow and malformed
  inputs remain `None`. The legacy add helper's broader outputs are unchanged.
  Full `od-isa`, workspace build, formatting, Graphify and independent Sol/high
  review pass. See [S2-29af report](reports/S2_29AF_X87_LARGE_GAP_ADDITION_PRECISION.md).
  ISSUE-019 remains open.

- **FCHS/FABS stale rejection assertions — resolved:** `fabs_rejects_missing_or_invalid_source_and_boundaries_atomically` and `fchs_rejects_invalid_source_and_execution_boundaries_atomically` now use standalone SF (`FSW=0x0040`) in the rejection vectors; IE|SF (`0x0041`) remains accepted as specified by S2-30bi. The existing tests continue to assert atomic state preservation, and the forbidden-status matrix passes. Validation: focused FABS/FCHS tests and the S2-30bi forbidden-status test pass; full `cargo test -p od-isa --offline` passes 739/739, `cargo test -p od-testkit --offline` 189/189, and `cargo test -p od-jit --offline` 5/5. No CPU semantics changed. Reference: [S2-30bi CPU contract](specs/CPU_CONTRACT.md#local-s2-30bi-historical-stack-fault-status-through-bounded-fchs-fabs); [S2-30bi report](reports/STAGE_2_REPORT.md#s2-30bi--historical-sf-only-with-ie-through-bounded-fchs-fabs).
- **S2-30ci historical FCMOVNU extent:** original exact bare `DB D9` coverage is superseded by indexed S2-30cj. The indexed local Long64 interpreter admits `DB D8..DF`; it checks TOP and `(TOP+i)&7` before PF for `i>0`, checks only TOP for self-index, and copies raw bytes/tag only for PF=0. Under FCW `0x037f` and `FSW & 0x82ff == 0`, used participants must be Valid canonical Normal or Zero exact signed zero. Local evidence: 363,520 success snapshots (358,400 indexed plus 5,120 self), 161,280 invalid class/tag snapshots (150,528 indexed plus 10,752 self), 18-stream schema, literal rows, complete producer/complement snapshots and JIT denial. Native/JIT execution, x87 exception and unordered/Special semantics, and physical x86 validation remain open; no Mac mini or SSH run was made. Next step remains the shared final batched host/physical acceptance. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30cj-bounded-indexed-fcmovnu-st0-sti-db-d8df), [plan](plans/STAGE_2.md#s2-30cj-indexed-bare-fcmovnu-st0-sti-db-d8df), [report](reports/STAGE_2_REPORT.md#s2-30cj--indexed-bare-fcmovnu-st0-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30cj-indexed-bare-fcmovnu-st0-sti-db-d8df).

- **S2-30bu bounded FCMOVB (historical ST(1) extent):** exact bare `DA C1` locally preflighted both physical operands as Valid canonical Normal or Zero exact signed zero before checking CF, copied raw bytes/tag only when taken, and preserved FSW under the local `C1=0` gate. Its 51,200 success and 21,504 invalid class/tag matrices and five-stream allowlist describe the earlier ST(1)-only extent, superseded for indexed register sources by S2-30bv. Undefined-C1 behavior, x87 exception delivery, JIT/native execution and physical acceptance remain unverified. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bu-bounded-fcmovb-st0-st1-da-c1), [plan](plans/STAGE_2.md#s2-30bu-bare-fcmovb-st0-st1), [report](reports/STAGE_2_REPORT.md#s2-30bu--bounded-fcmovb-st0-st1) and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bu-bounded-fcmovb-st0-st1).
- **S2-30bv indexed FCMOVB:** exact bare `DA C0..C7` now locally preflights physical TOP plus `(TOP+i)&7` for `i>0`; self-index `i=0` validates only TOP and leaves its exact bytes/tag unchanged. A taken indexed move copies the exact source bytes/tag; gates remain FCW `0x037f`, `FSW & 0x82ff == 0`, and Valid canonical Normal or Zero exact signed zero. The 363,520 success and 161,280 invalid class/tag interpreter matrices, per-index environment/boundary checks, 19-stream schema and eight CLI fixtures are local evidence only. Undefined-C1 behavior, x87 exception delivery, JIT/native execution and physical acceptance remain unverified. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bv-bounded-indexed-fcmovb-sti-da-c0c7), [plan](plans/STAGE_2.md#s2-30bv-indexed-bare-fcmovb-st0-sti), [report](reports/STAGE_2_REPORT.md#s2-30bv--bounded-indexed-fcmovb-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bv-bounded-indexed-fcmovb-sti).
- **S2-30ca historical FCMOVNE extent:** exact bare DB C9 ST(1)-only coverage has been superseded by S2-30cb indexed DB C8..CF. Existing C9 fixtures and producer sequences remain applicable; indexed acceptance adds all source indices under the same strict FCW/FSW and canonical Normal/Zero gates. The C9-only history is not evidence for the newly indexed forms.
- **S2-30cb indexed FCMOVNE evidence:** exact bare `DB C8..CF` selects physical `(TOP+i)&7` and preflights participants before integer ZF; self-index validates TOP only. ZF=0 copies raw binary80 bytes/tag and ZF=1 preserves them. Local coverage includes 363,520 successes, 161,280 invalid class/tag rejects, per-index environment/status/boundary checks, 19 exact schema streams, eight indexed CLI fixtures, repeat and FCOMI producer cases, and FCMOVE complement checks. JIT/native execution, exception delivery and physical x86 acceptance remain open under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-30cb-bounded-indexed-bare-fcmovne-st0-sti-db-c8cf), [plan](plans/STAGE_2.md#s2-30cb-indexed-bare-fcmovne-st0-sti-db-c8cf), [report](reports/STAGE_2_REPORT.md#s2-30cb--indexed-bare-fcmovne-st0-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30cb-indexed-bare-fcmovne-st0-sti).
- **S2-30cc historical FCMOVBE extent:** the exact bare Long64 `DA D1` result was local evidence for ST(1) only; the limitation is superseded by S2-30cd. No native/JIT or physical behavior is established by that historical slice.
- **S2-30cd indexed FCMOVBE evidence:** exact bare Long64 `DA D0..D7` carries the source index in `Operation`, preflights TOP and `(TOP+i)&7` before integer CF/ZF for i>0, and preflights only TOP for i=0. Under FCW `0x037f` and `FSW & 0x82ff == 0`, each used participant must be Valid canonical Normal or Zero exact signed zero; the raw ten-byte/tag copy occurs exactly when CF or ZF is set. The 363,520 success and 645,120 invalid participant snapshots, exact twenty-stream schema, full-state indexed fixtures, self-condition rows, wrap/tag/environment checks and JIT denial are local evidence only. C1=1, x87 exception delivery, native/JIT execution and physical x86 acceptance remain open. See [contract](specs/CPU_CONTRACT.md#local-s2-30cd-bounded-indexed-fcmovbe-st0-sti-da-d0d7), [plan](plans/STAGE_2.md#s2-30cd-indexed-bare-fcmovbe-st0-sti-da-d0d7), [report](reports/STAGE_2_REPORT.md#s2-30cd--indexed-bare-fcmovbe-st0-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30cd-indexed-bare-fcmovbe-st0-sti-da-d0d7).

- **Историческая запись S2-30cc о FABS/FCHS test expectations — resolved:** At that point, the full `od-isa` suite had 718 passes and two failures because FABS/FCHS reject vectors seeded IE|SF. Later S2-30bi explicitly admitted seeded IE|SF history, so the two vectors were changed to standalone SF (`FSW=0x0040`), which remains forbidden without IE. State-preservation assertions remain. The related FST/FSTP subnormal and FCMOV/schema neighbor expectations mentioned in the original entry were corrected in their respective follow-ups. Current validation is recorded in the resolved [FCHS/FABS entry above](#issue-019--семантика-арифметики-и-инструкций-x87-остаётся-открытой); no test-maintenance follow-up remains for this record.

- **S2-30bs bounded FNOP evidence:** exact bare Long64 `D9 D0` locally preserves all x87 state except FIP and retires once under FCW `0x037f` and clean `FSW & 0x80ff`; it performs no payload inspection or data access. The 4,608-snapshot matrix, nine FSW-bit rejects, invalid FCW/missing-state and atomic boundary checks, exact four-stream schema, six CLI fixtures, and JIT denial are local evidence only. C0..C3 preservation follows local policy; physical x86/native behavior and general x87 exception behavior remain unresolved. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bs-bounded-bare-fnop-d9-d0), [plan](plans/STAGE_2.md#s2-30bs-bare-fnop-with-a-bounded-environment-gate), [report](reports/STAGE_2_REPORT.md#s2-30bs--bounded-bare-fnop), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bs-bounded-fnop).

- **S2-30bq/br bounded FFREE evidence:** exact bare `DD C0..C7` marks only physical `(TOP+i)&7` Empty under FCW `0x037f` and `FSW & 0x80ff == 0`, without reading the prior tag/payload; TOP, all payloads, other tags, complete status/control words and other state are retained. The 24,576-case opaque-state matrix, retained 3,072-case ST(0) coverage, idempotence, environment/decode/boundary rejects, exact 19-stream schema allowlist and indexed composition fixtures are local evidence only. Stack exceptions are not modeled, and no JIT/native/physical x86 behavior is established. ISSUE-019 remains open. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30br-bounded-bare-ffree-sti-dd-c0c7), [plan](plans/STAGE_2.md#s2-30br-generalize-bounded-ffree-to-all-sti-indices), [report](reports/STAGE_2_REPORT.md#s2-30br--bounded-ffree-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30br-bounded-ffree-sti).

- **S2-30bn bounded FLD1/FLDZ evidence:** bare `D9 E8`/`D9 EE` now push exact binary80 one/zero only to an Empty physical destination under FCW `0x037f` and clean `FSW & 0x80ff == 0`; any occupied destination rejects atomically. The 1,536-case interpreter matrix, fill-eight/ninth-reject and FLD1→FLDZ→FXCH→FSTP composition plus four exact schema-v2 fixtures are local evidence only. Empty-source operations, Special arithmetic, architectural stack exceptions and native/physical acceptance remain unresolved. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bn-bounded-fld1-and-fldz-constant-pushes), [plan](plans/STAGE_2.md#s2-30bn-bare-fld1-and-fldz-bounded-local-constant-push-slice), [report](reports/STAGE_2_REPORT.md#s2-30bn--bounded-fld1-and-fldz), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bn-fld1-and-fldz-constant-pushes).
- **S2-30bo bounded FLD ST(0) evidence:** exact bare `D9 C0` copies the old TOP's canonical Normal or signed Zero raw bytes/tag into the Empty physical push slot after atomic source/destination preflight. The 5,120-state interpreter matrix, rejection classes, seven-copy/eighth-reject, FLD1/FLDZ compositions, exact four-stream schema and literal fixtures are local evidence only. Empty/Special stack behavior, stack exceptions, physical acceptance and native/JIT execution remain open; do not treat these results as broader x87 support. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bo-bounded-bare-fld-st0-d9-c0), [plan](plans/STAGE_2.md#s2-30bo-bare-fld-st0-bounded-raw-copy-push-slice), [report](reports/STAGE_2_REPORT.md#s2-30bo--bounded-fld-st0), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bo-bounded-fld-st0).
- **S2-30bp bounded FLD ST(i) evidence:** exact bare `D9 C0..C6` selects physical source `(old_TOP+i)&7`, then copies its canonical Normal/signed Zero payload and tag into Empty `(old_TOP+7)&7`, atomically preflighting both. Local coverage has 35,840 matrix successes plus source/tag/environment/destination/boundary rejection and six new indexed success fixtures. C7, Empty/Special/malformed sources, stack exceptions, and JIT/native/physical behavior remain outside the slice; the shared final physical substage is still needed before any hardware claim. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bp-bounded-bare-fld-sti-d9-c0c6), [plan](plans/STAGE_2.md), [report](reports/STAGE_2_REPORT.md#s2-30bp--bounded-fld-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bp-bounded-fld-sti).

- **S2-30bl bounded FXCH evidence:** the exact bare `D9 C8+i` profile now accepts either Valid-tagged canonical-Normal values or Zero-tagged exact signed zero. Nonzero indices atomically exchange all ten raw bytes and physical two-bit tags; `i=0` validates TOP only and retains its payload/tag. Local coverage retains 15,360 Normal-only states and adds 32,256 mixed indexed plus 256 self-Zero states, malformed Zero/wrong-tag/environment rejection, full-state/tag assertions, schema checks, and literal fixtures. C0/C2/C3 preservation remains a deterministic local policy without a hardware-equality claim. Empty/Special participants, stack faults, physical acceptance, and broader x87 behavior remain open under ISSUE-019; JIT/native admission remains denied. See [contract](specs/CPU_CONTRACT.md#local-s2-30bl-canonical-signed-zero-through-bounded-fxch-sti), [plan](plans/STAGE_2.md#s2-30bl-admit-canonical-signed-zero-to-bounded-fxch), [report](reports/STAGE_2_REPORT.md#s2-30bl--canonical-signed-zero-through-bounded-fxch), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bl-canonical-signed-zero-fxch).
- **S2-30ao bounded FCHS evidence:** bare `D9 E0` admits only canonical Zero-tagged signed zero through the narrow sign-operation gate; the Normal-only gates for FTST/FST/FSTP remained unchanged at that historical point. S2-30ap extends that same narrow gate to FABS; S2-30aq/ar/as later supersede those FTST/FST/FSTP limits respectively. Local interpreter and project-fixture evidence does not establish native or physical behavior. ISSUE-019 remains open for final host/physical acceptance. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30ao-signed-zero-fchs-admission-d9-e0), [plan](plans/STAGE_2.md#s2-30ao-admit-canonical-signed-zero-to-bounded-fchs-d9-e0) and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30ao-signed-zero-fchs).
- **S2-30ap bounded FABS evidence (historical scope):** bare `D9 E1` admits canonical Zero-tagged signed zero, clears only its sign bit and C1, preserves the Zero tag/TOP/payload and other state, records FIP and retires once. The old statements that FST and FSTP ST(i) remained Normal-only were superseded by S2-30ar and S2-30as respectively. See [contract](specs/CPU_CONTRACT.md#local-s2-30ap-signed-zero-fabs-admission-d9-e1), [plan](plans/STAGE_2.md#s2-30ap-admit-canonical-signed-zero-to-bounded-fabs-d9-e1), [report](reports/STAGE_2_REPORT.md#s2-30ap--canonical-signed-zero-through-bounded-fabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30ap-signed-zero-fabs).
- **S2-30aq bounded FTST evidence (historical scope):** bare `D9 E4` additionally accepts exact Zero-tagged signed zero, compares either sign equal, and sets C3 only. The old statements that FST and FSTP ST(i) remained Normal-only were superseded by S2-30ar and S2-30as respectively. See [contract](specs/CPU_CONTRACT.md#local-s2-30aq-canonical-signed-zero-ftst-d9-e4), [plan](plans/STAGE_2.md#s2-30aq-admit-canonical-signed-zero-to-bounded-ftst-d9-e4), [report](reports/STAGE_2_REPORT.md#s2-30aq--canonical-signed-zero-through-bounded-ftst), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30aq-signed-zero-ftst).
- **S2-30at bounded FTST evidence:** bare `D9 E4` additionally accepts only Special-tagged canonical binary80 ±infinity (E=`0x7fff`, J=1, fraction=0), comparing negative as Less/C0 and positive as Greater/no condition codes; C1 is cleared and TOP/raw bytes/tags are preserved. The interpreter covers both signs × all TOP × all 16 prior condition-code states with repeat idempotence, malformed source/tag/environment rejection and execution-boundary atomicity. Two exact-sequence `project_fixture` compositions load binary32 ±infinity and observe `0x3900`/`0x3800` through FNSTSW; schema shape is unchanged. JIT/native and physical x86 behavior remain unverified, so ISSUE-019 stays open. See [contract](specs/CPU_CONTRACT.md#local-s2-30at-canonical-signed-infinity-ftst-d9-e4), [plan](plans/STAGE_2.md#s2-30at-admit-canonical-signed-infinity-to-bounded-ftst-d9-e4), [report](reports/STAGE_2_REPORT.md#s2-30at-canonical-signed-infinity-through-bounded-ftst), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30at-canonical-signed-infinity-ftst).
- **S2-30ar bare FST ST(i) signed-zero evidence (historical):** exact Zero-tagged ±0 passes the existing `0x037f` FCW and `FSW & 0x80ff == 0` gates. FST copies captured bytes and the Zero tag to `(old_TOP+i)&7` with no pop/rotation; Normal FST behavior is unchanged. Its then-current statement that FSTP remained Normal-only was superseded by S2-30as. See [contract](specs/CPU_CONTRACT.md#local-s2-30ar-canonical-signed-zero-fst-sti-dd-d0i), [plan](plans/STAGE_2.md#s2-30ar-admit-canonical-signed-zero-to-bare-fst-sti-dd-d0i), [report](reports/STAGE_2_REPORT.md#s2-30ar--canonical-signed-zero-through-fst-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30ar-signed-zero-fst-sti).
- **S2-30au bounded FST/FSTP ST(i) infinity evidence:** the shared source gate now additionally admits only Special-tagged canonical signed binary80 infinity. FST copies the ten bytes/tag without TOP change; FSTP copies then empties old TOP and advances TOP. Unit and exact-sequence project-fixture coverage passed; schema, JIT and native admission were not widened. Local evidence does not establish physical x86 behavior. ISSUE-019 remains open for the combined Mac mini/physical acceptance. See [contract](specs/CPU_CONTRACT.md#local-s2-30au-canonical-signed-infinity-through-fstfstp-sti), [plan](plans/STAGE_2.md#s2-30au-admit-canonical-signed-infinity-to-bounded-fstfstp-sti), [report](reports/STAGE_2_REPORT.md#s2-30au--canonical-signed-infinity-through-fstfstp-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30au-canonical-signed-infinity-fstfstp-sti).
- **S2-30bb bounded FST/FSTP ST(i) subnormal evidence:** only `(Special, Subnormal)` canonical binary80 E=0/J=0/nonzero-fraction sources now pass the shared raw-copy gate. The 16,384-case opcode/sign/TOP/index/condition/fraction matrix, malformed/tag/control/boundary rejects, raw FLD m80 composition, four seeded CLI fixtures and JIT denial are covered. Physical x86/runtime acceptance is deferred to final combined S2-31 validation; this local result does not close ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-30bb-canonical-subnormal-fst-fstp-sti), [plan](plans/STAGE_2.md#s2-30bb-admit-canonical-binary80-subnormal-to-bounded-fstfstp-sti), [report](reports/STAGE_2_REPORT.md#s2-30bb-canonical-binary80-subnormal-through-bounded-fstfstp-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bb-canonical-binary80-subnormal-fst-fstp-sti).
- **S2-30az bounded FST/FSTP ST(i) QuietNaN evidence:** the shared register-store source gate additionally admits only `(Special, QuietNaN)` canonical binary80 values with E=`0x7fff`, J=1 and Q=1, either sign and any low-62-bit payload. FST copies bytes/tag without changing TOP or FSW; FSTP copies before emptying old TOP and advancing TOP. The 16,384-case full-state interpreter matrix, malformed/tag/environment/boundary rejects, four exact-sequence m32 QNaN fixtures, and m32/m64 direct compositions pass. Existing Normal/zero/infinity cases and the local C1 policy remain; schema, classifier and JIT/native admission are unchanged. No physical x86 behavior is established, so ISSUE-019 remains open for combined host/physical acceptance. See [contract](specs/CPU_CONTRACT.md#local-s2-30az-canonical-qnan-fst-fstp-sti), [plan](plans/STAGE_2.md#s2-30az-admit-canonical-binary80-quietnan-to-bounded-fstfstp-sti), [report](reports/STAGE_2_REPORT.md#s2-30az--canonical-binary80-quietnan-through-bounded-fstfstp-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30az-canonical-qnan-fst-fstp-sti).
- **S2-30av bounded FCHS/FABS evidence:** the shared narrow sign-operation gate additionally admits only `(Special, Infinity)` for canonical signed binary80 infinity in bare `D9 E0`/`D9 E1`; sign transforms, C1 clear, TOP/tag and remaining state preserve the existing local policy. Full-state coverage includes both opcodes × both signs × all TOP × all 16 prior condition-code states, repeated-operation checks, neighboring-class/tag/environment and execution-boundary rejection, plus m32/m64 load→operation→store→FNSTSW compositions. Two exact-sequence project fixtures and one wrong-tag CLI fixture pass; the schema and JIT/native gates remain unchanged. Local evidence is not physical x86 evidence; ISSUE-019 remains open for combined host/physical acceptance. See [contract](specs/CPU_CONTRACT.md#local-s2-30av-canonical-signed-infinity-fchs-fabs), [plan](plans/STAGE_2.md#s2-30av-admit-canonical-signed-infinity-to-bounded-fchsfabs), [report](reports/STAGE_2_REPORT.md#s2-30av--canonical-signed-infinity-through-bounded-fchsfabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30av-canonical-signed-infinity-fchsfabs).
- **S2-30aw bounded FCHS/FABS evidence:** the shared gate additionally admits only `(Special, QuietNaN)` for canonical binary80 E=`0x7fff`, J=1, Q=1, either sign, and all low-62 payload values. FCHS toggles only the sign bit; FABS clears it; Special tag, TOP, payload and other state remain preserved, C1 clears, and no exception flags are added. A 2,048-case full-state interpreter matrix covers the two operations, signs, all TOP and prior condition-code states with four payload boundaries, repeated FABS/double FCHS, malformed neighbors, wrong tags, environment and execution boundaries. Four existing-schema m32 QNaN load→operation→store→FNSTSW CLI fixtures cover both signs/operations, and direct interpreter compositions cover m32/m64; the wrong-tag CLI rejection fixture passes. JIT remains denied and schema/classifier are unchanged. Physical/native behavior is unverified; ISSUE-019 stays open. See [contract](specs/CPU_CONTRACT.md#local-s2-30aw-canonical-qnan-fchs-fabs), [plan](plans/STAGE_2.md#s2-30aw-admit-canonical-binary80-quietnan-to-bounded-fchsfabs), [report](reports/STAGE_2_REPORT.md#s2-30aw--canonical-binary80-quietnan-through-bounded-fchsfabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30aw-canonical-binary80-quietnan-fchsfabs).
- **S2-30ax bounded FCHS/FABS evidence:** the shared gate additionally admits only canonical `(Special, SignalingNaN)` binary80 sources with E=`0x7fff`, J=1, Q=0 and a nonzero low-62-bit payload. Both signs are accepted; FCHS toggles the sign bit, FABS clears it, and both preserve Q=0, payload, tag/TOP and other state, clear C1, and add no IE/PE. The 2,048-case full-state matrix checks both operations/signs/all TOP/all prior condition codes/four payloads, double-FCHS and repeated-FABS, malformed and wrong-tag encodings, environment, and fetch/truncation/STOP/budget/fault boundaries. A raw FLD m80fp→operation→FSTP m80fp→FNSTSW AX unit composition inspects bytes after load/operation/store; four seeded bare schema-v2 fixtures check both signs/operations with budget one, and a wrong-tag CLI case rejects atomically. JIT remains denied. No m32/m64 conversion, classifier/schema change or JIT/native admission was added. Physical x86 behavior remains unverified; ISSUE-019 stays open. See [contract](specs/CPU_CONTRACT.md#local-s2-30ax-canonical-snan-fchs-fabs), [plan](plans/STAGE_2.md#s2-30ax-admit-canonical-binary80-signalingnan-to-bounded-fchsfabs), [report](reports/STAGE_2_REPORT.md#s2-30ax--canonical-binary80-signalingnan-through-bounded-fchsfabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30ax-canonical-binary80-signalingnan-fchsfabs).
- **S2-30ay bounded FCHS/FABS evidence:** the shared gate additionally admits only `(Special, Subnormal)` canonical binary80 values with E=0, J=0 and nonzero 63-bit fraction. Both signs are accepted; FCHS toggles only the sign bit, FABS clears it, and both preserve fraction, tag/TOP and other state, clear C1, and add no DE/UE/PE/IE. A 2,048-case full-state matrix covers both operations/signs/all TOP/prior condition codes/four fractions, double-FCHS and repeated-FABS, malformed neighboring classes, wrong tags, invalid/missing state and execution boundaries. A raw FLD m80fp→operation→FSTP m80fp→FNSTSW AX composition inspects bytes after load/operation/store and checks no DE; four raw-seeded budget-one schema-v2 fixtures check both signs/operations and JIT denial, while a wrong-tag CLI case rejects atomically. The classifier/schema and all other gates remain unchanged. Physical x86 behavior remains unverified; ISSUE-019 stays open for final combined acceptance. See [contract](specs/CPU_CONTRACT.md#local-s2-30ay-canonical-subnormal-fchs-fabs), [plan](plans/STAGE_2.md#s2-30ay-admit-canonical-binary80-subnormal-to-bounded-fchsfabs), [report](reports/STAGE_2_REPORT.md#s2-30ay--canonical-binary80-subnormal-through-bounded-fchsfabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30ay-canonical-binary80-subnormal-fchsfabs).

- **Нормативная неопределённость: #D при integer-store из denormal/pseudo-denormal.** Intel SDM rev.093 Vol.1 §8.2.2/Table 8-3 говорит, что pseudo-denormal при использовании как operand трактуется как denormal и вызывает DE; §8.4.1/Table 8-9 классифицирует FIST/FISTP как arithmetic instructions; §8.5.2 предписывает #D при арифметической операции с denormal operand. При этом специализированная Vol.2A FIST/FISTP entry (pp. 3-347–348) перечисляет #IS/#IA/#P, но не #D. Текущие S2-31cq/cr/cs ожидают PE без нового DE при canonical binary80 subnormal; это пока неоднозначность нормативного текста, а не подтверждённый дефект поведения.
- **Последствия:** ожидаемое появление нового DE для integer-store из canonical subnormal и pseudo-denormal не установлено. Не расширять FISTP на pseudo-denormal с PE-only предположением до разрешения противоречия; не считать текущие PE-only фикстуры аппаратным доказательством.
- **Воспроизведение/основание:** сравнить официальный Intel SDM rev.093 Vol.1 §§8.2.2, 8.4.1, 8.5.2 с Vol.2A FIST/FISTP pp. 3-347–348 и существующими S2-31cq/cr/cs fixtures. Исходники: [Vol.1](https://cdrdv2-public.intel.com/v1/dl/getContent/671436), [Vol.2A](https://cdrdv2-public.intel.com/v1/dl/getContent/671199).
- **Статус:** открыто; фактическое поведение на x86 не проверено.
- **Следующий шаг:** в едином финальном Mac mini/physical acceptance добавить отдельные inputs с DE=0: canonical binary80 subnormal и pseudo-denormal через m16/m32 FIST/FISTP и m64 FISTP, для всех допустимых widths и RC; сравнить сохранение/установку DE, PE, результат, TOP и destination. После evidence согласовать instruction-specific contract, fixtures и status S2-31cq/cr/cs. До этой проверки native evidence отсутствует.

- **S2-31dq bounded FRNDINT evidence:** Special-tagged unnormals (biased exponent 1..`0x7ffe`, J=0) now have a local masked-invalid path producing fixed negative binary80 indefinite and sticky IE, with C1 cleared. The literal seeded fixture observes IE with FNSTSW and clears it with FNCLEX. Evidence is interpreter/project-fixture only; FLD, JIT/native and physical acceptance stay unchanged. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-31dq-special-unnormal-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dq-special-tagged-binary80-unnormal-through-bounded-frndint), [report](reports/STAGE_2_REPORT.md#s2-31dq--special-tagged-binary80-unnormal-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-31dq-frndint-unnormal-fixture). This does not resolve or change the separate FIST/FISTP #D ambiguity above.
- **S2-31dr bounded FRNDINT evidence:** exact Special-tagged binary80 pseudo-infinities (E=`0x7fff`, J=0, zero fraction, either sign) use the S2-31dq masked-invalid indefinite path in bare D9 FC. The 12,288-case interpreter matrix covers all FCW/TOP/legal historical IE/SF/sticky-flag/prior-C1 combinations, and a seeded `project_fixture` observes IE through FNSTSW and clears it with FNCLEX. E=0 zero/tag mismatch remains rejected; pseudo-NaNs were rejected at this substage and are subsequently covered by S2-31ds. FLD and JIT/native admission are unchanged. Physical x86/Mac mini acceptance remains unperformed; this evidence does not close ISSUE-019. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-31dr-special-pseudo-infinity-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dr-special-tagged-binary80-pseudo-infinity-through-bounded-frndint), [report](reports/STAGE_2_REPORT.md#s2-31dr--special-tagged-binary80-pseudo-infinity-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-31dr-frndint-pseudo-infinity-fixture).
- **S2-31ds bounded FRNDINT evidence:** bare D9 FC accepts Special-tagged binary80 pseudo-NaNs (E=`0x7fff`, J=0, nonzero fraction) for both signs and Q=bit62 values 0/1 through the same fixed negative indefinite/sticky IE path as S2-31dq/dr; C1 clears, no current DE/PE/SF/ZE/OE/UE is added, and admitted historical flags survive. The independent 49,152-case full-state interpreter matrix and seeded Q=0/Q=1 `D9 FC DF E0 DB E2` project fixtures are recorded. Canonical J=1 SNaN quieting, FLD/classifier scope and JIT/native denial remain unchanged; the fixtures are not native evidence. The normative basis is Intel SDM revision 093 Vol.2A FRNDINT p.3-372 and Vol.1 §8.2.2 Tables 8-3/8-10 and Table 4-3. Physical x86 and general x87 behavior remain open under ISSUE-019; this work does not resolve the separate FIST/FISTP #D ambiguity. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-31ds-special-pseudo-nan-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31ds-special-tagged-binary80-pseudo-nan-through-bounded-frndint), [report](reports/STAGE_2_REPORT.md#s2-31ds--special-tagged-binary80-pseudo-nans-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-31ds-frndint-pseudo-nan-fixtures).
- **S2-31du bounded FLD OE evidence:** only exact bare D900/DD00 widened their local FSW mask from `0x80cc` to `0x80c4`, admitting historical OE while preserving IE/DE/OE/UE/PE histories. The 131,072-case full-state matrix includes both prior-C1 values and all 32 sticky combinations; seeded OE-only FNSTSW fixtures and same-width FST-overflow→FLD-infinity→FNSTSW/FNCLEX compositions remain local interpreter evidence. OE producers are covered only under nearest and directed-away rounding. ZE/SF/ES/B continue to reject; S2-31ak rejection fixtures now use ZE, and the schema regression verifies legal ZE-based Normal Unsupported fixtures for D900/DD00 are rejected after changing only FSW to OE-only. Native runtime, physical x86, Mac mini and general exception delivery remain unverified; this task does not resolve ISSUE-019. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-31du-historical-sticky-oe-for-bounded-m32m64-fld), [plan](plans/STAGE_2.md#s2-31du-admit-historical-sticky-oe-through-exact-binary32binary64-fld), [report](reports/STAGE_2_REPORT.md#s2-31du--historical-sticky-oe-for-bounded-m32m64-fld), and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-30f evidence:** destination-index literal vectors cover varied TOP/wrap, alias with unrelated Empty tags, source preservation, bad used tags, specials, unequal exponents and inexact coefficients. DC decode is exhaustive; testkit schema accepts all eight bare encodings and native admission remains fail-closed. Full x87 architectural behavior remains unverified.
- **S2-30g evidence:** all 64 TOP/destination pairs verify physical result placement, pop/tag/TOP updates and payload preservation; i=0 Empty-tag behavior, i=1 new ST(0), wrap, exact carry, i=0 overflow rejection, unsupported/fetch/stop/budget atomicity, exhaustive DE decode, fixture-schema acceptance and native fail-closed behavior are covered. Full x87 architectural behavior remains unverified.
- **S2-30h evidence:** literal subtraction vectors cover all eight TOP values and i=1..7, source payload sentinels, sign combinations, ±2^-63, minimum Normal and maximum-coefficient carry. Full-state rejects include every i=0/TOP alias, odd 65-bit coefficient, exact subnormal output, overflow and unsupported input classes. D8 ModR/M/prefix/WAIT/truncation and neighbor forms are checked; schema and runner cover i=1..7 success/i=0 Unsupported, while native admission remains closed. Full x87 architectural behavior remains unverified.
- **S2-30i evidence:** literal destination-form vectors cover sign/order asymmetry, ±2^-63, maximum even 65-bit carry, minimum Normal, and exact placement for every TOP×i=1..7 pair. Full-state atomic rejects cover alias, missing state, FCW/FSW, tags, unequal exponents, all raw non-Normal classes, cancellation, odd 65-bit coefficient, exact subnormal output and overflow. DC ModR/M/prefix/fetch boundaries, fixture schema and runner matrix are covered; native admission remains closed. Full x87 architectural behavior remains unverified.
- **S2-30j evidence:** DE E8+i uses the shared exact FSUB result gate, writes to old physical `(TOP+i)&7`, empties old TOP, increments TOP, clears C1, records FIP and retires once. Literal vectors cover sign/order cases, ±2^-63, maximum even carry and minimum Normal; every TOP×i=1..7 checks physical placement, TOP wrap/pop, preserved payloads and state. Alias for every TOP and numeric/environment/tag/domain rejects compare full state; fetch, stop and zero-budget boundaries, exhaustive DE ModR/M/prefix decode, schema acceptance and all-eight native denial are covered. Full x87 architectural behavior remains unverified.
- **S2-30k evidence:** D8 E8+i computes ST(i)−ST(0) into old physical TOP with no pop. Literal direction/sign and exact boundary vectors, all TOP×i placements, alias/domain full-state rejection, fetch/stop/budget, exhaustive D8 decode/prefix, fixture schema/runner and native fail-closed behavior are covered. Full x87 architectural behavior remains unverified.
- **S2-30l evidence:** DC E0+i computes ST(0)−ST(i) into old physical (TOP+i)&7 without pop. Local vectors cover all TOP×i destinations, exact boundary values, alias rejection and fetch/stop/budget atomicity; decoder/schema/native-deny coverage is documented with the implementation. Full x87 architectural behavior remains unverified.
- **S2-30m evidence:** DE E0+i computes ST(0)−ST(i) into old physical (TOP+i)&7, then empties old TOP and increments TOP. Literal vectors cover every TOP×i pair, signs/order, ±2^-63, maximum accepted even carry and minimum Normal; alias and numeric/environment/tag/domain rejects preserve complete state. Fetch/truncation/STOP/budget boundaries, exhaustive DE ModR/M and prefix/REX/WAIT decoding, schema alias representation, all-index fixture-runner results and eight-code native denial are covered. Full x87 architectural behavior remains unverified.
- **S2-30n evidence:** D8 C8+i computes the exact product into old physical TOP without pop. Local vectors cover every TOP×i pair (including the valid i=0 square), sign XOR, differing exponents, inexact/overflow/subnormal rejection, raw class/environment/tag rejection, and fetch/stop/budget atomicity. D8 decoding, fixture schema, runner state checks and native fail-closed admission cover the encoding group. Full x87 architectural behavior remains unverified.
- **S2-30o evidence:** DC C8+i computes the exact product into old physical `(TOP+i)&7` without pop. Local vectors cover every TOP×i pair (including the valid i=0 square), payload sentinels, full-width-significand multiplication, and inexact/overflow/subnormal plus fetch/stop/budget atomicity. DC decoding preserves adjacent FADD/FSUBR/FSUB groups; fixture schema and native fail-closed coverage include all eight encodings. Full x87 architectural behavior remains unverified. The native admission test defect for DE E0..E7 was corrected by adding `FsubrpStSt0` to the explicit x87 deny set.
- **S2-30p evidence:** DE C8+i computes exact old `ST(i)*ST(0)` into old physical `(TOP+i)&7`, then empties old TOP and increments TOP. Literal coverage checks all TOP×i pairs, including i=0 square write-before-empty, all sign combinations, differing exponents, a full-width significand, i=0 inexact/overflow rejection, full-state inexact/range/raw-class/environment/tag rejection, and fetch/stop/budget atomicity. Exhaustive DE decoding preserves FADDP, FSUBRP and FSUBP neighbors; the fixture schema and runner cover DE C8..CF, including a no-stop/budget-one inexact DE C8 rejection, and native admission remains closed. Full x87 architectural behavior remains unverified.
- **S2-30q evidence:** D8 F0+i divides old ST(0) by ST(i) exactly and writes old physical TOP without popping. Literal coverage includes all TOP×i pairs, 15/3=5, 1/3 rejection, all signs, unequal exponents, lhs-significand<rhs normalization, full-width significand divided by one, i=0 ±minimum/±maximum Normal self-division to +1, and exact overflow/subnormal range rejection. Environment/tag/raw-class/zero/inexact rejects and missing state compare complete state; fetch/stop/budget preserve atomicity. Exhaustive D8 decode, schema acceptance of all eight encodings with an actual no-stop D8 F1 inexact case, runner coverage and explicit native denial are recorded in the S2-30q report. Full x87 architectural behavior remains unverified.
- **S2-30r evidence:** DE F8+i divides old ST(i) by old ST(0), writes to old physical `(TOP+i)&7`, empties old TOP and increments TOP. Literal vectors cover every TOP×i pair, ordered 15/3=5, the i=0 exact +1 payload before pop, sign cases, TOP wrap, and full-state atomic inexact 1/3 and 2/3, overflow, subnormal, environment, tag, raw-class and absent-state rejection. Fetch, truncation, stop and zero-budget boundaries are included. Exhaustive DE decoding preserves neighboring groups and leaves DE F0..F7 unsupported; schema and runner cover all eight successful encodings and no-stop/budget-one inexact DE F9 rejection. Native admission stays closed. Full x87 architectural behavior remains unverified.
- **S2-30s evidence:** DE F0+i divides old ST(0) by old ST(i), writes to old physical `(TOP+i)&7`, empties old TOP and increments TOP. Literal tests cover all TOP×i placements, ordered 15/3=5, all sign combinations, exponent differences, a full-width significand divided by one, positive/negative i=0 self-division producing +1 before pop, and full-state atomic inexact 1/3 and 2/3, overflow, subnormal, environment, tag, raw-class and missing-state rejection. Fetch/truncation/stop/budget boundaries, exhaustive DE decoding with neighboring groups, schema admission of all eight encodings with an inexact DE F1 no-stop/budget-one fixture, runner matrix and native denial are covered. Full x87 architectural behavior remains unverified.
- **S2-30t evidence:** D8 F8+i divides old ST(i) by old ST(0), overwrites old physical TOP without pop, clears C1, records FIP and retires once. Literal vectors cover all TOP×i placements, ordered 15/3=5, i=0 exact +1 with tags/TOP unchanged, signs, unequal exponents, full-width significand and atomic rejection of actual inexact 1/3 plus environment/tag/class/missing-state failures. D8 exhaustive ModR/M/prefix handling, schema acceptance of all eight forms with a no-stop/budget-one D8 F9 inexact case, testkit runner and native fail-closed admission are covered. Full x87 architectural behavior remains unverified.
- **S2-30v evidence:** DC F8+i divides old ST(i) by old ST(0) into old physical `(TOP+i)&7` without pop. Literal vectors cover all 64 TOP×i placements, ordered 15/3=5, the i=0 exact +1 alias, signs, unequal exponents and Normal edges, plus full-state atomic rejection of valid-tag inexact 1/3, invalid environment/tags/classes and out-of-range outputs. Exhaustive DC ModR/M/prefix checks preserve DC F0..F7 FDIVR; schema and runner cover all eight encodings plus no-stop/budget-one DC F9 rejection, and native admission denies all eight. Full x87 architectural behavior remains unverified.
- **S2-30w evidence:** DD D8+i copies old ST(0) as ten untouched raw bytes to old physical `(TOP+i)&7`, marks the destination Valid, then empties old TOP and advances TOP; the register array is not rotated. Literal vectors cover all 64 TOP×i pairs, each prior destination tag for i!=0, signed/minimum/maximum Normal values, untouched sentinels, i=0 Empty result tag and i=1 new ST(0). Invalid source tag/class, FCW/FSW and missing-state inputs reject atomically; fetch/truncation/STOP/budget boundaries preserve state. DD ModR/M/prefix tests, schema admission, all-index CLI runner checks, a no-stop/budget-one source-tag rejection fixture and all-eight native denial are covered. Full x87 architectural behavior remains unverified. See [S2-30w contract](specs/CPU_CONTRACT.md#local-s2-30w-bounded-fstp-sti-dd-d8i), [plan](plans/STAGE_2.md#s2-30w-bare-bounded-fstp-sti-local-raw-copy-slice), [report](reports/STAGE_2_REPORT.md#s2-30w--bounded-fstp-sti-local-raw-copy-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30x evidence:** DD D0+i copies ten raw bytes from old ST(0) to old physical `(TOP+i)&7`, marks the destination Valid, and preserves TOP plus the complete FSW. Literal vectors cover all 64 TOP×i pairs, every old destination tag for i!=0, signed/minimum/maximum Normal payloads, i=0 alias, and complete-state source/environment/fetch/truncation/STOP/budget rejection. Exhaustive DD ModR/M checks preserve the existing D8..DF FSTP decode; schema accepts all eight encodings and the DD D1 source-tag rejection fixture, CLI tests cover all destinations, and native admission denies all eight forms. Full x87 architectural behavior remains unverified. See [S2-30x contract](specs/CPU_CONTRACT.md#local-s2-30x-bounded-fst-sti-dd-d0i), [plan](plans/STAGE_2.md#s2-30x-bare-bounded-fst-sti-local-raw-copy-slice), [report](reports/STAGE_2_REPORT.md#s2-30x--bounded-fst-sti-local-raw-copy-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30y evidence:** Bare `D9 E0` flips only ST(0)'s raw sign bit for an existing x87 state with FCW `0x037f`, clear exception/B bits, Valid physical TOP tag, and canonical Normal source; it clears C1, records FIP, and retires once. Literal vectors cover every TOP, both signs and minimum/maximum/full-significand Normal values, exact one-bit changes, C1 clear/set, double-FCHS payload restoration, full-state environment/tag/class/missing-state rejection, and fetch/truncation/STOP/budget boundaries. Exhaustive D9 second-byte decode accepts only E0; schema and CLI cover success plus a no-stop/budget-one Empty-tag rejection, and native admission explicitly denies D9 E0. Full x87 architectural behavior remains unverified. See [S2-30y contract](specs/CPU_CONTRACT.md#local-s2-30y-bounded-fchs-d9-e0), [plan](plans/STAGE_2.md#s2-30y-bare-bounded-fchs-sign-bit-local-slice), [report](reports/STAGE_2_REPORT.md#s2-30y--bounded-fchs-sign-bit-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30z evidence:** Bare `D9 E1` clears only ST(0)'s raw sign bit for an existing x87 state with FCW `0x037f`, clear exception/B bits, Valid physical TOP tag, and canonical Normal source; it clears C1, records FIP and retires once. Literal vectors cover every TOP, both signs and minimum/maximum/full-significand Normal values, exact sign-bit-only mutation, C1 clear/set, C0/C2/C3 retention, FABS idempotence, full-state missing/environment/tag/class rejection and fetch/truncation/STOP/budget boundaries. Exhaustive D9 decode accepts only E0→FCHS and E1→FABS; schema/CLI cover success and a no-stop/budget-one Empty-tag rejection, while native admission explicitly denies FABS and D9 E1. Full x87 architectural behavior remains unverified. See [S2-30z contract](specs/CPU_CONTRACT.md#local-s2-30z-bounded-fabs-d9-e1), [plan](plans/STAGE_2.md#s2-30z-bare-bounded-fabs-sign-bit-clear-local-slice), [report](reports/STAGE_2_REPORT.md#s2-30z--bounded-fabs-sign-clear-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30aa evidence:** Bare `D9 E4` compares canonical-Normal ST(0) with zero under the bounded source gate, replacing all C0/C1/C2/C3 bits from sign, preserving TOP/tags/raw payloads and other state, recording FIP and retiring once. Literal vectors cover all TOPs, both signs, minimum/maximum/full-significand Normals, all initial condition-code combinations including all-set, FTST→FNSTSW AX composition, full-state invalid environment/tag/class/missing-state rejection and fetch/truncation/STOP/budget boundaries. Exhaustive D9 decoding accepts exactly E0→FCHS, E1→FABS and E4→FTST; two success fixtures plus a no-stop/budget-one Empty-tag rejection pass schema/CLI coverage. Native admission explicitly denies FTST. Full x87 architectural behavior remains unverified. See [S2-30aa contract](specs/CPU_CONTRACT.md#local-s2-30aa-bounded-ftst-d9-e4), [plan](plans/STAGE_2.md#s2-30aa-bare-bounded-ftst-canonical-normal-comparison-slice), [report](reports/STAGE_2_REPORT.md#s2-30aa--bounded-ftst-canonical-normal-comparison-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30ab/S2-30ac evidence:** The original bare `D8 D1` bounded FCOM ST(1) slice is generalized to exactly bare `D8 D0+i` for all eight source indices. It compares old physical TOP with `(TOP+i)&7` by exact sign/exponent/64-bit-significand ordering, replaces all condition bits, preserves complete register/tag/environment payload state, records FIP and retires once; i=0 validates the shared Valid Normal alias once and returns equality. Vectors cover all 64 TOP×i placements, wrap/alias, all initial condition-code combinations, relations/sign orders, low-significand distinctions, Normal endpoints, independent tag/class/environment rejects, alias failure, sentinels, and fetch/truncation/STOP/budget atomicity. Existing S2-30ab literal fixtures remain; S2-30ac adds a literal D8 D7 success and a no-stop/budget-one invalid-ST(7)-tag fixture. Schema and CLI cover all eight indices and exact FCOM→FNSTSW AX sequences; unsupported D8 D8..DF remain rejected. Native admission now explicitly denies all eight D8 D0..D7 encodings. Full x87 architectural behavior remains unverified. See [S2-30ac contract](specs/CPU_CONTRACT.md#local-s2-30ac-bounded-fcom-sti-d8-d0i), [plan](plans/STAGE_2.md), [report](reports/STAGE_2_REPORT.md#s2-30ac--bounded-fcom-sti-exact-normal-compare), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30ad evidence:** Bare `D8 D9` compares old physical ST(0)/ST(1) through the existing exact canonical-Normal FCOM helper, replaces the FSW condition field, empties the old physical TOP tag, advances TOP with wrap, preserves all register payloads and unrelated state, records FIP and retires once. Literal and interpreter vectors cover all TOP values and initial condition fields, relation/sign/exponent/low-significand ordering, independent lhs/rhs tag/class and environment rejects, sentinels, and fetch/truncation/STOP/budget atomicity. Schema and CLI admit D8 D9 plus exact D8 D9 DF E0 observation; the invalid rhs-tag fixture is no-stop/budget-one. Native admission explicitly denies D8 D9. Full x87 architectural behavior remains unverified. See [S2-30ad contract](specs/CPU_CONTRACT.md#local-s2-30ad-bounded-fcomp-st1-alias-d8-d9), [plan](plans/STAGE_2.md), [report](reports/STAGE_2_REPORT.md#s2-30ad--bounded-fcomp-st1-compare-and-physical-top-pop), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30ae evidence:** Generalized the FCOMP operation to `FcompSt { source }` and accepts exactly bare `D8 D8..DF`. It compares old physical TOP with old `(TOP+i)&7` through the exact canonical-Normal helper, then replaces C0/C1/C2/C3, empties old TOP, advances TOP, preserves physical payload bytes, records FIP and retires once. `i=0` validates the shared source before equality and still pops once. Interpreter vectors cover all 64 TOP×i pairs, all initial condition fields and comparison relations, independent operand tag/class rejects, environment and execution-boundary atomicity. Eight literal success fixtures cover every i; schema/CLI admit all encodings and exact `D8 D8+i DF E0` observation with the corresponding one/two-instruction budget. Native admission explicitly denies all eight FCOMP encodings. Full x87 architectural behavior remains unverified. See [S2-30ae contract](specs/CPU_CONTRACT.md#local-s2-30ae-bounded-fcomp-sti-d8-d8i), [plan](plans/STAGE_2.md), [report](reports/STAGE_2_REPORT.md#s2-30ae--bounded-fcomp-sti-compare-and-physical-top-pop), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30af evidence:** Bare `DE D9` compares old physical ST(0)/ST(1) using the bounded canonical-Normal helper, then replaces the condition field, empties both old physical tags and advances TOP by two with wrap. Raw physical payloads and unrelated state stay unchanged; FIP/RIP and retirement update once. Interpreter vectors cover all TOP values, all three relations and all 16 initial condition fields, including wrap to 0/1 and an unused Empty ST(2); bad lhs/rhs and environment/fetch/truncation/STOP/budget inputs reject atomically. Schema/CLI cover exact `ded9`, `ded9dfe0`, and actual invalid-rhs no-stop/budget-one rejection. Exhaustive DE decoding retains existing arithmetic and leaves DE D8/DA..DF unsupported; native admission explicitly denies FCOMPP. Full x87 architectural behavior remains unverified. See [S2-30af contract](specs/CPU_CONTRACT.md#local-s2-30af-bounded-fcompp-de-d9), [plan](plans/STAGE_2.md), [report](reports/STAGE_2_REPORT.md#s2-30af--bounded-fcompp-compare-and-two-entry-physical-top-pop), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30ag evidence:** Bare `DD E1` FUCOM compares old physical TOP/TOP+1 through the exact integer canonical-Normal helper, replaces condition bits with greater/less/equal, clears C1 by explicit local policy, preserves TOP/tags/raw payloads, records FIP and retires once. All TOP×relation×16 incoming condition combinations, signed/exponent/low-significand ordering, independent bad lhs/rhs tags and classes, environment and fetch/truncation/STOP/budget atomicity are covered. Exhaustive DD decode retains FST/FSTP neighbors; schema and literal runner fixtures cover `dde1`, `dde1dfe0`, and an actual invalid-rhs-tag no-stop/budget-one rejection. Native admission denies FUCOM/DD E1. FUCOM unique QNaN behavior is excluded by the canonical-Normal-only domain. Full x87 architectural behavior remains unverified. See [S2-30ag contract](specs/CPU_CONTRACT.md#local-s2-30ag-bounded-fucom-st0-st1-dd-e1), [plan](plans/STAGE_2.md#s2-30ag-bare-bounded-fucom-st0-st1-local-canonical-normal-slice), [report](reports/STAGE_2_REPORT.md#s2-30ag--bounded-fucom-st0-st1-compare-without-pop), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30ah evidence:** Bare `DD E9` FUCOMP reuses the bounded canonical-Normal ST(0)/ST(1) compare helper, replaces condition codes, clears C1 by local policy, empties only old physical TOP, advances TOP with wrap, preserves raw register payloads and old ST(1) tag, records FIP and retires once. Interpreter vectors cover all TOP×relation×16 condition combinations, payload sentinels, independent invalid operand tags/classes and environment, plus fetch/truncation/STOP/budget atomicity. Schema and literal CLI fixtures cover `dde9`, `dde9dfe0`, and actual invalid-old-ST(1)-tag no-stop/budget-one rejection; native admission denies FUCOMP/DD E9. QNaN/special-value semantics and full x87 behavior remain unverified. See [S2-30ah contract](specs/CPU_CONTRACT.md#local-s2-30ah-bounded-fucomp-st0-st1-dd-e9), [plan](plans/STAGE_2.md#s2-30ah-bare-bounded-fucomp-st0-st1), [report](reports/STAGE_2_REPORT.md#s2-30ah--bounded-fucomp-st0-st1-compare-and-one-entry-pop), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30ai evidence:** Bare `DA E9` FUCOMPP preflights both old physical canonical-Normal operands through `bounded_x87_fcom_result(state, 1)`, replaces condition bits with greater/less/equal, clears C1 by local policy, empties both old physical tags, and advances TOP by two with wrap. Payload bytes remain untouched; FIP/RIP and retirement update once. Interpreter tests cover all TOP×relation×16 condition fields, wrap, an unused Empty ST(2) with arbitrary payload, independent operand tag/class failures, environment, fetch/truncation/STOP/budget atomicity. DA decode accepts only E9; schema/CLI cover exact `dae9`, `dae9dfe0`, and actual invalid-old-physical-ST(1)-tag no-stop/budget-one rejection. Native admission explicitly denies DA E9. Intel SDM revision 093 Vol.2A pp.3-402–3-404 opcode table/prose says two pops while p.3-403 pseudocode shows one; this slice follows the explicit table/prose. General special-value, stack-fault, exception, `#NM` and native x87 semantics remain unverified. See [S2-30ai contract](specs/CPU_CONTRACT.md#local-s2-30ai-bounded-fucompp-da-e9), [plan](plans/STAGE_2.md#s2-30ai-bare-bounded-fucompp), [report](reports/STAGE_2_REPORT.md#s2-30ai--bounded-fucompp-compare-and-two-entry-physical-top-pop), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30aj evidence:** Bare `DB F1` FCOMI uses the bounded canonical-Normal compare helper after checking x87 state, FCW/FSW and both physical tags. It maps greater/less/equal to cleared relation bits/CF/ZF, defines all `0x8d5` RFLAGS bits, clears only FSW.C1, and preserves other x87 state. Coverage includes all TOP×relation×16 initial FSW condition combinations, all 64 initial affected-flag masks, invalid operand tags/classes, environment and fetch/truncation/STOP/budget atomicity, exhaustive DB decode/prefixes, exact `dbf1`/`dbf1dfe0` fixture schema/CLI vectors with actual bad old ST(1) tag, and explicit native denial. General x87 exception/special-value behavior and native execution remain unverified. See [S2-30aj contract](specs/CPU_CONTRACT.md#local-s2-30aj-bounded-fcomi-st0-st1-db-f1), [plan](plans/STAGE_2.md#s2-30aj-bare-bounded-fcomi-st0-st1), [report](reports/STAGE_2_REPORT.md#s2-30aj--bounded-fcomi-st0-st1-to-rflags), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30ak evidence:** Bare `DF F1` FCOMIP reuses the bounded canonical-Normal compare preflight, maps greater/less/equal to cleared relation bits/CF/ZF, defines all `0x8d5` RFLAGS bits, clears only FSW.C1, empties the actual old physical TOP tag and advances TOP with wrap. It preserves old physical ST(1)'s Valid tag, all register payloads and unrelated state. Coverage includes all TOP×relation×16 initial FSW condition combinations, all 64 initial affected-flag masks with definedness, bad used tags/classes, environment and fetch/truncation/STOP/budget atomicity, exhaustive DF decoding/prefix rejection, exact `dff1`/`dff1dfe0` fixture schema/CLI vectors with actual bad old ST(1) tag, and explicit native denial of `Operation::Fcomip`/DF F1. General x87 exception/special-value behavior and native execution remain unverified. See [S2-30ak contract](specs/CPU_CONTRACT.md#local-s2-30ak-bounded-fcomip-st0-st1-df-f1), [plan](plans/STAGE_2.md#s2-30ak-bare-bounded-fcomip-st0-st1), [report](reports/STAGE_2_REPORT.md#s2-30ak--bounded-fcomip-st0-st1-to-rflags-and-pop), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-30am evidence:** Bare DF E9 FUCOMIP adds the bounded canonical-Normal compare/pop slice, with TOP×relation×16-condition and 64-RFLAGS-mask matrices, actual physical TOP=5/RHS=6 invalid-tag fixture, exact schema/CLI boundary vectors, exhaustive DF allowlist/prefix checks and explicit native denial. These checks do not establish unordered values, x87 exception delivery, physical x86 execution, or full x87 semantics; see [S2-30am report](reports/STAGE_2_REPORT.md#s2-30am--bounded-fucomip-st0-st1-to-rflags-and-pop) and [CPU contract](specs/CPU_CONTRACT.md#local-s2-30am-bounded-fucomip-st0-st1-df-e9).
- **S2-30al evidence:** Bare `DB E9` FUCOMI preflights canonical-Normal old ST(0)/ST(1) through the bounded integer comparator, maps ordered relations to cleared relation bits/CF/ZF, defines RFLAGS `0x8d5`, clears only FSW.C1 and preserves all other state. Coverage spans TOP×relation×FSW condition combinations, 64 initial flag masks with definedness, invalid operand tags/classes/environment, execution boundaries, exhaustive DB/prefix decoding, exact `dbe9`/`dbe9dfe0` schema/CLI fixtures with the real old ST(1) Empty-tag rejection, `dbf8dfe0` schema rejection (DB F8 remains outside the indexed group) and native denial. QNaN unordered behavior, exception delivery and native x87 execution remain outside this slice. See [S2-30al contract](specs/CPU_CONTRACT.md#local-s2-30al-bounded-fucomi-st0-st1-db-e9), [plan](plans/STAGE_2.md#s2-30al-bare-bounded-fucomi-st0-st1), [report](reports/STAGE_2_REPORT.md#s2-30al--bounded-fucomi-st0-st1-to-rflags), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **Дата обнаружения:** 2026-10-23.
- **Компонент:** `od-isa::x87`, S2-29–S2-39.
- **Статус:** открыта. S2-29a–s наращивают bounded raw/finite helpers: representation/classification, finite normalization, exact alignment/sum/product/quotient, mathematical rounding, normal/subnormal packing и композиции arithmetic→one-round result. S2-29u adds the p64-aware large-gap binary80 addition bridge, S2-29v adds pure finite square root, S2-29w adds exact power-of-two finite scaling, S2-29x adds exact truncated finite remainder, and S2-29y adds exact nearest-even finite remainder; these remain bounded mathematical helpers and leave architectural x87 instruction behavior open. S2-30ba additionally copies canonical Special-tagged binary80 SNaNs and S2-30bb extends that same raw-copy gate to canonical Special-tagged binary80 subnormals; physical/native x87 acceptance remains open. S2-30a–c — ограниченные environment/status instructions; S2-30d–g — exact-only FADD/FADDP; S2-30h–m — exact-only FSUB/FSUBP/FSUBR/FSUBRP; S2-30n–p — exact-only register FMUL/FMULP; S2-30q–v — exact-only FDIV/FDIVR/FDIVP/FDIVRP directions with canonical Normal results; S2-30w adds bare DD D8+i raw-copy FSTP with a canonical-Normal Valid source and explicit physical TOP pop; S2-30x adds bare DD D0+i raw-copy FST without changing TOP; S2-30y adds bare D9 E0 FCHS with a canonical-Normal Valid ST(0) source and sign-bit-only mutation; S2-30z adds bare D9 E1 FABS with the same source gate and sign-bit clear; S2-30aa adds bare D9 E4 FTST with that source gate and sign-derived replacement of all four condition-code bits. S2-30ag adds bare DD E1 FUCOM ST(0),ST(1) for canonical Normals with status-only commit and no pop. FADD/FSUB bounded forms require equal exponents, while FMUL and FDIV permit differing exponents; all require existing x87 state and canonical Normal used operands. S2-30u adds bare `DC F0+i` as `ST(0)/ST(i)` into physical ST(i), without pop. Эти срезы не закрывают общую x87 instruction semantics.
- **Суть и последствия:** `divide_finite_to_binary80`, `multiply_finite_to_binary80`, `round_pack_exact_magnitude`, `round_pack_exact_coefficient` и addition helpers завершают только bounded математическое округление и кодирование конечных binary80 результатов; это не реализует guest FMUL или остальные общие x87 instruction semantics. Division/product reject zero inputs; coefficient/add/magnitude helpers reject exact zero, while add also rejects two zero operands, unequal exponents in the equal-exponent path, and finite-range overflow. Signed zero is preserved for nonzero tiny rounded division/product results. No helper selects an architectural zero sign. Helpers do not implement x87 RC/PC/control-word semantics, architectural underflow/precision effects, special values, general guest FDIV/FADD/FSUB/FMUL semantics (except existing exact-only bounded instruction slices), flags/exceptions. FNINIT, FNSTSW AX and FNCLEX remain bounded local forms: CR0.EM/TS and #NM are not modeled; FNINIT does not model preceding WAIT FINIT or pending-exception delivery. FNSTSW AX copies raw FSW, while C0/C1/C2/C3 are architecturally undefined; deterministic snapshot retention is only local model policy. FNCLEX clears FSW[0:7] and FSW[15]; pending-exception check and delivery are not modeled. MMX/SSE shared state, native-state observation, and complete save/restore remain open. Helpers do not imply general x87 execution or FPU feature availability; x87 snapshot remains a bounded local profile.
- **Основание:** локальные literal vectors покрывают exact finite intermediates, mathematical rounding, normal/subnormal bytes и композиции finite division/product/coefficient/addition с one-round pack. Для S2-29s проверяются even-magnitude equivalence, odd product tails below/at/above ties во всех режимах/знаках, normal carry, minimum-normal/subnormal promotion, minimum/half-minimum subnormal, signed underflow zero, exponent/finite rejection, zero product и input immutability. Это не проверяет FMUL instruction semantics. Для S2-29q проверяются ±1+±1, signed normal ties в четырёх режимах, минимум/максимум subnormal границы, zero-neutral для normal/subnormal/pseudo-denormal в обоих порядках, max-finite plus zero, overflow rejection, cancellation, все zero-sign pairs, unequal exponents и неизменность input/provenance. Сумма двух significands ограничена `2^65-2`, поэтому synthetic carry-to-four недостижим; exact subnormal sums кратны minimum subnormal, поэтому inexact subnormal tails и округление в underflow zero недостижимы. S2-30d literal vectors покрывают исходный exact-only FADD commit/reject профиль. S2-30e добавляет независимые byte vectors для всех i=0..7 и разных TOP, alias case i=0 и wrap cases i=1/TOP=7, i=7/TOP=7, empty unrelated tags при i=0, per-index full-state reject, exhaustive decode/prefix/truncation, schema acceptance и native fail-closed coverage. Full x87 architectural behavior не проверялся. См. [отчёт S2-29s](reports/S2_29S_X87_PRODUCT_PACK.md), [отчёт этапа 2 S2-29q](reports/STAGE_2_REPORT.md#s2-29q--compose-same-exponent-finite-addition-with-one-round-binary80-packing-partial), [CPU contract S2-29q](specs/CPU_CONTRACT.md#local-s2-29q-same-exponent-finite-addition-to-one-rounded-binary80-result), [план S2-29q](plans/STAGE_2.md#s2-29q-compose-same-exponent-finite-addition-with-one-round-binary80-packing-partial), предыдущие [S2-29p](reports/STAGE_2_REPORT.md#s2-29p--round-exact-coefficient-to-one-binary80-result-partial) и [S2-29o](reports/STAGE_2_REPORT.md#s2-29o--compose-finite-division-with-single-round-binary80-packing-partial), и [исходник x87.rs](../crates/od-isa/src/x87.rs).
- **S2-31a evidence:** bare `D9 FC` preflights only a Valid old physical TOP with canonical-Normal `e=0..62`, one of four PC64/all-masked RC control words, and `FSW & 0x80df == 0`. Integer-only quotient/remainder rounding and binary80 packing cover all TOP×RC×sign×exact/tie/adjacent-boundary vectors and both signed `2^63` outputs; PE sticky, C1 increment, local C0/C2/C3 retention, complete unrelated-state preservation, invalid source/environment and execution-boundary atomicity are checked. The schema admits `D9 FC` and exact `D9 FC DF E0` (FNSTSW AX); literal CLI fixtures cover tie-to-even, status composition and an invalid physical TOP tag. Native admission explicitly rejects FRNDINT. This does not complete S2-31 or establish general x87 rounding/exceptions. See [S2-31a contract](specs/CPU_CONTRACT.md#local-s2-31a-bounded-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31a-bare-bounded-frndint-d9-fc-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31a--bounded-frndint-partial-slice), and [fixtures](../tests/fixtures/x86/stage2/README.md).
- **S2-31dc evidence:** signed-zero support requires the Zero tag and exact raw ±0; success preserves ten bytes, Zero/TOP, PE, C0/C2/C3 and all unrelated state, clears C1, records FIP, advances RIP and retires once. The 256-case sign×FCW×TOP×PE×C1 matrix, malformed tag/payload rejects, unchanged Normal regression, and literal negative-zero FNSTSW fixture are covered. Existing FNCLEX/decode and native/JIT denial remain in force. Physical acceptance is deferred to the combined final S2-31 substage. See [S2-31dc contract](specs/CPU_CONTRACT.md#local-s2-31dc-canonical-signed-zero-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dc-admit-signed-zero-to-bounded-frndint-d9-fc), [report](reports/STAGE_2_REPORT.md#s2-31dc--canonical-signed-zero-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31dd evidence:** bounded `D9 FC` accepts Valid canonical-Normal inputs with unbiased exponent 63..16383 and returns the original bytes before shift/round/pack; it clears only C1 among status bits, preserves sticky PE/C0/C2/C3 and all raw/tag/stack state, updates FIP/RIP, and retires once. Matrices cover the requested exponent/significand/sign/TOP/FCW/PE/C1 combinations; boundary tests distinguish fractional e=62 rounding from exact e=63 identity. A literal FNSTSW CLI fixture covers composition. Native/JIT remain closed. Physical x87 acceptance and general exception behavior remain deferred to combined S2-31 acceptance. See [S2-31dd contract](specs/CPU_CONTRACT.md#local-s2-31dd-exact-integer-normal-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dd-admit-exact-integer-normal-values-to-bounded-frndint-d9-fc), [report](reports/STAGE_2_REPORT.md#s2-31dd--exact-integer-normal-values-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31de evidence:** bare `D9 FC` now admits only Special-tagged canonical signed binary80 infinity (J=1, exponent `0x7fff`, zero fraction), preserving all ten bytes/TOP/tag and PE/C0/C2/C3 while clearing C1, updating FIP/RIP and retiring once. The 256-case sign/FCW/TOP/PE/C1 matrix, malformed Special/tag/environment and boundary rejects, literal `FLD m80fp`→`FRNDINT`→`FNSTSW AX` CLI fixture, JIT denial and existing native x87 deny are covered. Physical x86 acceptance and general x87 exception behavior remain open under ISSUE-019. See [S2-31de contract](specs/CPU_CONTRACT.md#local-s2-31de-canonical-signed-infinity-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31de-admit-canonical-signed-infinity-to-bounded-frndint-d9-fc), [report](reports/STAGE_2_REPORT.md#s2-31de--canonical-signed-infinity-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31df evidence:** bare `D9 FC` admits only Special-tagged canonical binary80 QNaNs with exponent `0x7fff`, J=1 and bit 62=1; sign and lower 62 payload bits are preserved exactly. The 1,024-case significand/sign/FCW/TOP/PE/C1 matrix, SNaN/pseudo-NaN/tag/control/status rejects, execution-boundary atomicity, literal FLD→FRNDINT→FNSTSW fixture, JIT denial and existing native x87 deny are covered. Physical x86 acceptance and general x87 exception behavior remain open under ISSUE-019. See [S2-31df contract](specs/CPU_CONTRACT.md#local-s2-31df-canonical-quiet-nan-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31df-admit-canonical-quiet-nan-to-bounded-frndint-d9-fc), [report](reports/STAGE_2_REPORT.md#s2-31df--canonical-quiet-nans-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31dg evidence:** bare `D9 FC` admits only Special-tagged canonical binary80 SNaNs (E=`0x7fff`, J=1, Q=0, lower payload nonzero), sets only Q, adds sticky IE without PE, clears C1 and preserves PE/C0/C2/C3 and unrelated state. The 1,024-case matrix, zero-payload/QNaN/J=0/tag/control/status/prefix/fetch/truncation/STOP/budget controls, IE re-entry/FNSTSW/FNCLEX checks, literal FLD→FRNDINT→FNSTSW→FNCLEX→FRNDINT fixture, and JIT/native denial are recorded. Physical x86 behavior and general exception delivery remain open under ISSUE-019. See [S2-31dg contract](specs/CPU_CONTRACT.md#local-s2-31dg-canonical-signaling-nan-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dg-quiet-canonical-signaling-nan-in-bounded-frndint-d9-fc), [report](reports/STAGE_2_REPORT.md#s2-31dg--canonical-signaling-nans-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31dh evidence:** bare `D9 FC` admits historical IE using `FSW & 0x80de == 0`, while DE/ZE/OE/UE/SF/ES/B remain rejected. Coverage crosses the admitted zero, Normal, infinity, QNaN and SNaN classes with signs, all FCWs/TOPs, IE/PE and old C1; current inexact rounding still sets PE, current SNaN still sets IE and quiets, and IE+forbidden-status cases reject atomically. A quieted SNaN re-enters successfully without FNCLEX. The existing FNSTSW/FNCLEX fixture and JIT/native denial remain in place. Physical x86/native runtime behavior remains deferred under ISSUE-019. See [S2-31dh contract](specs/CPU_CONTRACT.md#local-s2-31dh-historical-sticky-ie-in-bounded-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dh-admit-historical-sticky-ie-to-bounded-frndint-d9-fc), [report](reports/STAGE_2_REPORT.md#s2-31dh--historical-sticky-ie-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31dj evidence:** `D9 FC` now admits historical sticky DE under the narrow status mask `0x80dc`; DE remains set after FRNDINT, current rounding and IE/PE behavior is unchanged, and the four accepted all-masked FCWs are unchanged. Interpreter vectors cover representative Normal exponents and all supported classes with sign/RC/TOP/sticky IE/DE/PE/prior-C1 combinations, and forbidden status bits paired with admitted flags remain atomic rejects. Literal m32/m64 minimum-subnormal FLD→FRNDINT→FNSTSW fixtures verify the loader's canonical-Normal+DE state flows through rounding. JIT/native denial remains in place. Physical x86 acceptance and general x87 exception delivery remain open; complete them in the combined final S2-31 acceptance substage. See [S2-31dj contract](specs/CPU_CONTRACT.md#local-s2-31dj-historical-sticky-de-in-bounded-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dj-admit-historical-sticky-de-to-bounded-frndint-d9-fc), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31dk evidence:** bare `D9 FC` changes only its local FSW gate from `0x80dc` to `0x80cc`, admitting and preserving historical UE. Independent interpreter vectors cover UE with every IE/DE/PE combination, prior C1, all four RCs, every TOP, and exact Normal, inexact Normal and canonical SNaN inputs. Exact UE-only does not create PE; current inexact rounding adds PE; SNaN adds IE without PE. ZE/OE/SF/ES/B remain atomic rejects with UE and admitted flags. Literal CLI fixtures cover seeded UE and FST m32 underflow→FRNDINT→FNSTSW composition. Native/JIT remain closed. Physical x86/Mac mini acceptance and general x87 exception delivery remain open under ISSUE-019. See [S2-31dk contract](specs/CPU_CONTRACT.md#local-s2-31dk-historical-sticky-ue-in-bounded-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dk-admit-historical-sticky-ue-to-bounded-frndint-d9-fc), [report](reports/STAGE_2_REPORT.md#s2-31dk--historical-sticky-ue-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31dl evidence:** bare `D9 FC` changes only its local FSW gate from `0x80cc` to `0x80c4`, admitting and preserving historical OE. Independent interpreter vectors cross OE with IE/DE/UE/PE, prior C1, all four RCs, every TOP, and exact/fractional Normal plus canonical SNaN. Exact/identity sources do not create PE; current inexact rounding adds PE and SNaN adds IE without PE. ZE/SF/ES/B remain atomic rejects when OE and admitted stickies are set. Literal CLI fixtures cover seeded OE and FST m32 overflow→FRNDINT→FNSTSW, including final OE+PE with C1 cleared; direct producer status is separately covered by the existing FST m32 overflow unit matrix. Native/JIT remain closed. Physical x86/Mac mini acceptance and general x87 exception delivery remain open under ISSUE-019. See [S2-31dl contract](specs/CPU_CONTRACT.md#local-s2-31dl-historical-sticky-oe-in-bounded-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dl-admit-historical-sticky-oe-to-bounded-frndint-d9-fc), [report](reports/STAGE_2_REPORT.md#s2-31dl--historical-sticky-oe-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31dm evidence:** the bounded FRNDINT gate changes only from `0x80c4` to `0x80c0`, admitting historical ZE while retaining independent IE/DE/OE/UE/PE and rejecting SF/ES/B. The existing class/sign/RC/TOP/prior-C1 matrix crosses ZE with all 32 combinations of the other sticky flags; exact inputs retain ZE without creating PE, fractions add PE, and current SNaN adds IE without PE. A literal seeded `D9 FC DF E0` fixture begins at FSW=0x000c (ZE+OE), explicitly documents that the input state was seeded with no guest producer, and asserts AX/FSW=0x000c. No FDIV-zero producer/domain or JIT/native admission changed. Physical x86/Mac mini acceptance and general x87 exception delivery remain open under ISSUE-019. See [S2-31dm contract](specs/CPU_CONTRACT.md#local-s2-31dm-historical-sticky-ze-in-bounded-frndint-d9-fc), [plan](plans/STAGE_2.md#s2-31dm-admit-historical-sticky-ze-to-bounded-frndint-d9-fc), [report](reports/STAGE_2_REPORT.md#s2-31dm--historical-sticky-ze-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31dn evidence:** bounded `D9 FC` uses `FSW & 0x8080 == 0` plus an explicit SF-without-IE reject. The independent 32,768-case matrix crosses historical SF+IE with DE/ZE/OE/UE/PE, all eight source classes, signs, four FCWs, TOP and prior C1; SF+IE and other historical sticky bits survive, while C1 is recomputed from the current result. SF-only rejects include current SNaN; ES/B and existing Empty/mismatch/raw-malformed controls remain rejected. The literal `D9 FC DF E0` project_fixture starts at source 1.0, FCW 0x037f and FSW 0x0241, observing AX=0x0041. IE+SF are seeded in the initial snapshot, are not guest-produced, and the fixture is not native evidence. No ES/B or pending/#MF/WAIT semantics changed; JIT/native remain closed. Physical x86 and general exception-delivery evidence remain open under ISSUE-019. See [S2-31dn contract](specs/CPU_CONTRACT.md#local-s2-31dn-historical-sfie-in-bounded-frndint-d9-fc), [plan](plans/STAGE_2.md), [report](reports/STAGE_2_REPORT.md#s2-31dn--historical-sticky-sf-with-ie-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31di evidence:** bare `D9 FC` admits Valid canonical-Normal inputs with unbiased exponent `-16382..=-1` through comparison-only rounding. Nearest-even returns one only strictly above 0.5 at e=-1; directed results use sign/direction; all sources are inexact, set sticky PE and set C1 exactly for magnitude one. Rounded signed zeros carry Zero tags. The sign/FCW/TOP/exponent/significand/IE-PE/C1 matrix, repeated FRNDINT, atomic wrong-tag/status controls, literal minimum-normal→negative-zero→FNSTSW fixture, and continued JIT/native denial are recorded. Physical x86/Mac mini acceptance remains deferred to the combined final S2-31 substage; broader x87 exception delivery remains unverified. See [S2-31di contract](specs/CPU_CONTRACT.md#local-s2-31di-small-canonical-normal-frndint-d9-fc), [plan](plans/STAGE_2.md), [report](reports/STAGE_2_REPORT.md#s2-31di--small-canonical-normal-values-through-bounded-frndint), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31b evidence:** bare `DB 18` implements a bounded `FISTP m32int [RAX]` for canonical-Normal `e=0..31`, all-masked PC64 FCW values and `FSW & 0x80df == 0`. Integer-only quotient rounding rejects signed-i32 overflow before access; the full canonical48 Dword range is checked before one atomic `write_integer` call. Tests cover exact unaligned four-byte mutation/canaries, no data read, exactly one write, all four RC modes on exact/inexact signed values, literal raw binary80 vectors on both rounding-induced i32 boundaries, overflow-before-access, write permission failure and injected #PF with unchanged CPU state. Literal fixtures admit `DB 18`, its exact `DB 18 DF E0` status composition, and unsupported tag/overflow cases. Native execution explicitly denies the operation. Flat fixture regions do not model MMU translation, page walks, MMIO, SMP, or architectural exception delivery. See [S2-31b contract](specs/CPU_CONTRACT.md#local-s2-31b-bounded-fistp-m32int-rax-db-18), [plan](plans/STAGE_2.md#s2-31b-bare-bounded-fistp-m32int-rax-db-18-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31b--bounded-fistp-m32int-rax-partial-slice), and [fixtures](../tests/fixtures/x86/stage2/README.md).
- **S2-31c evidence:** bare `DB 10` implements bounded non-pop `FIST m32int [RAX]` with the S2-31b integer conversion domain, four RC modes, post-rounded signed-i32 bounds, full canonical48 validation and one atomic Dword store. Successful commit changes only PE/C1, FIP/FDP, RIP and retirement while preserving TOP, every physical tag, all payloads and unrelated state. Literal fixtures admit `DB 10` and exact `DB 10 DF E0`; interpreter checks cover signed half-way rounding, overflow-before-access, write fault atomicity and no-read/single-write behavior. Native execution explicitly denies the operation. Flat fixture RAM does not establish MMU translation, page walks, MMIO, SMP, or architectural exception delivery. See [S2-31c contract](specs/CPU_CONTRACT.md#local-s2-31c-bounded-fist-m32int-rax-db-10), [plan](plans/STAGE_2.md#s2-31c-bare-bounded-fist-m32int-rax-db-10-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31c--bounded-fist-m32int-rax-partial-slice), and [fixtures](../tests/fixtures/x86/stage2/README.md).
- **S2-31d evidence:** bare `DF 18` implements bounded `FISTP m16int [RAX]` for canonical-Normal old physical TOP with exponent 0..15, exact PC64/all-masked FCW values, and `FSW & 0x80df == 0`. The shared integer quotient/remainder converter checks signed-i16 bounds after rounding; success validates canonical48 across two bytes, performs one atomic Word write without a read, then pops TOP and commits PE/C1/FIP/FDP/RIP once. Literal fixtures cover all RC modes with both signs, ties, exact/rounded limits, `DF 18 DF E0`, overflow and invalid tag/status/configuration. Interpreter checks include full CPU/payload preservation, no-access rejects, permission/#PF atomicity and canonical-range rejection. Native admission denies this operation. Flat fixtures do not establish MMU, MMIO, SMP, physical execution, or general x87 exception behavior. S2-31ck supersedes the original finite-overflow rejection with integer-indefinite/IE commit for admitted DF 18/DF 10 forms. See [S2-31d contract](specs/CPU_CONTRACT.md#local-s2-31d-bounded-fistp-m16int-rax-df-18), [plan](plans/STAGE_2.md#s2-31d-bare-bounded-fistp-m16int-rax-df-18-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31d--bounded-fistp-m16int-rax-partial-slice), and [fixtures](../tests/fixtures/x86/stage2/README.md).
- **S2-31e evidence:** bare `DF 10` implements bounded non-pop `FIST m16int [RAX]` using S2-31d's conversion domain and signed-i16 range after RC rounding. Success validates canonical48 across both bytes, makes one atomic Word write without reading data, then commits PE/C1/FIP/FDP/RIP and retires once while preserving TOP, every tag and all payloads. Literal fixtures cover `DF 10`, exact `DF 10 DF E0`, no-pop state, tie/boundaries, overflow and invalid tag/status/control/exponent. Interpreter checks include all RC modes/signs, permission/#PF atomicity and canonical-range rejection. Native admission explicitly denies this operation. Flat fixtures do not establish MMU, MMIO, SMP, physical execution or general x87 exception behavior. S2-31ck supersedes the original finite-overflow rejection with integer-indefinite/IE commit for admitted DF 10/DF 18 forms. See [S2-31e contract](specs/CPU_CONTRACT.md#local-s2-31e-bounded-fist-m16int-rax-df-10), [plan](plans/STAGE_2.md#s2-31e-bare-bounded-fist-m16int-rax-df-10-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31e--bounded-fist-m16int-rax-partial-slice), and [fixtures](../tests/fixtures/x86/stage2/README.md).
- **S2-31f evidence:** bare `DF 38` implements a bounded `FISTP m64int [RAX]` with canonical-Normal old physical TOP, exponent 0..63, exact PC64/all-masked FCW values and `FSW & 0x80df == 0`. Integer quotient/remainder rounding checks signed-i64 limits in `u128` before narrowing and encodes through unsigned Qword two's complement; exponent 63 has zero shift and zero remainder. Success validates all eight canonical48 bytes, makes one atomic Qword write without reading data, then pops old TOP and commits PE/C1/FIP/FDP/RIP once. Literal fixtures cover exact signed boundaries, DF 38 DF E0, Qword canaries and +2^63 rejection; interpreter vectors cover all RC/sign ties, all RCs for -2^63 and `2^63 - 0.5`, -(2^63+1), exponent 64, canonical-range rejection and write/#PF atomicity. Native admission denies the operation. Overflow returns local Unsupported instead of an indefinite integer; architectural exception delivery and physical x86 behavior remain unmodeled. See [S2-31f contract](specs/CPU_CONTRACT.md#local-s2-31f-bounded-fistp-m64int-rax-df-38), [plan](plans/STAGE_2.md#s2-31f-bare-bounded-fistp-m64int-rax-df-38-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31f--bounded-fistp-m64int-rax-partial-slice), and [fixtures](../tests/fixtures/x86/stage2/README.md).
- **S2-31g evidence:** bare `D9 38` stores any schema-valid raw FCW through one canonical48-checked Word write, accepts pending FSW/status bits and Empty tags, and preserves the complete x87 snapshot plus GPR/RFLAGS. Exact-only schema/composition and native-deny coverage are added; workspace tests cover canaries, second-byte store-fault atomicity and execution boundaries. `cargo test --workspace --offline`, workspace check, fmt and diff checks passed. Layout check still reports pre-existing `.serena` as unmapped; it was left untouched. General x87 exception delivery and physical behavior remain unverified, so this umbrella issue stays open.
- **S2-31h evidence:** bare `DD 38` stores raw pre-operation FSW with one canonical48-checked Word write and no data read. It accepts arbitrary schema-valid FCW/FSW and Empty tags, preserves every x87/GPR/RFLAGS field, and explicitly denies native admission. Literal `0000`/mixed/`ffff`, composition, sentinel/canary and canonical-crossing fixtures plus interpreter checks for permission/backend/fetch/stop/budget failures are documented in the [S2-31h CPU contract](specs/CPU_CONTRACT.md#local-s2-31h-bounded-fnstsw-m2byte-rax-dd-38), [plan](plans/STAGE_2.md#s2-31h-bare-bounded-fnstsw-m2byte-rax-dd-38-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31h--bounded-fnstsw-m2byte-rax-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md). This does not verify #MF/#NM/CR0 behavior, MMU/MMIO, or physical execution; the umbrella issue remains open.
- **S2-31i evidence:** bare `D9 28` accepts only the four bounded FCWs after validating the old environment and one canonical48 Word read. All 16 FCW pairs, RC-driven FRNDINT composition, invalid old/new state, memory and execution boundaries are covered; native admission remains closed. Pending/unmasked exception delivery, WAIT, `#MF`, `#NM`/CR0, MMU/MMIO, and physical behavior remain unverified. See the [S2-31i CPU contract](specs/CPU_CONTRACT.md#local-s2-31i-bounded-fldcw-m2byte-rax-d9-28), [plan](plans/STAGE_2.md#s2-31i-bare-bounded-fldcw-m2byte-rax-d9-28-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31i--bounded-fldcw-m2byte-rax-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31j evidence:** the bounded `D9 18` binary80-to-binary32 slice uses integer quotient/remainder rounding, admits only canonical Normal exponents -126..126 and finite normal results, and performs one canonical48-checked atomic Dword write before the physical TOP pop. Literal tie/RC/sign/exponent vectors, all TOPs, C1/PE behavior, canaries, canonical/store failure atomicity, and exact `D9 18`/`D9 18 DF E0` CLI fixtures cover the local contract. Hardware exception priority/delivery, native x87 state observation, MMU/MMIO, SMP and physical x86 behavior remain unverified; S2-31 remains open. See the [S2-31j CPU contract](specs/CPU_CONTRACT.md#local-s2-31j-bounded-fstp-m32fp-rax-d9-18), [plan](plans/STAGE_2.md#s2-31j-bare-bounded-fstp-m32fp-rax-d9-18-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31j--bounded-fstp-m32fp-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31m evidence:** exact bare `DD 10` implements bounded no-pop `FST m64fp [RAX]` by reusing the unchanged S2-31l binary80-to-binary64 helper. Literal RC/tie/exponent vectors and all-TOP payload/tag preservation checks cover the unrounded ST(0); Qword/no-read, canonical-range, backend/#PF atomicity, invalid preflight and execution-boundary checks are documented with exact `DD 10` and `DD 10 DF E0` fixtures. Native admission explicitly denies the operation. The -1022..1022 exponent interval and condition-code preservation/C1 behavior are local deterministic policies. Physical x86 state/exception behavior, MMU/MMIO/SMP and the broader S2-31 exception model remain unverified; ISSUE-019 stays open. See [S2-31m CPU contract](specs/CPU_CONTRACT.md#local-s2-31m-bounded-fst-m64fp-rax-dd-10), [plan](plans/STAGE_2.md#s2-31m-bare-bounded-fst-m64fp-rax-dd-10-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31m--bounded-fst-m64fp-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31n evidence:** exact bare `DB 00` pushes a signed Dword integer into physical `(TOP+7)&7` after the exact FCW/FSW/Empty-tag gates and one canonical48-checked Dword read. Integer-only packing is covered by independent binary80 literals for zero, ±1/2/3, `2^24±1`, and i32 extrema; matrices cover all TOP and RC values, stack wrap and unrelated tag/payload preservation. Read faults/backends, invalid preflight, canonical range, fetch/truncation/STOP/budget atomicity, exhaustive DB/prefix decoding, exact `DB 00`/`DB 00 DF E0` fixture schema and native denial are covered. C0/C2/C3 retention is a local policy. General exception delivery, hardware fault priority, MMU/MMIO/SMP and physical x86 behavior remain unverified; S2-31 stays partial and ISSUE-019 remains open. See [S2-31n CPU contract](specs/CPU_CONTRACT.md#local-s2-31n-bounded-fild-m32int-rax-db-00), [plan](plans/STAGE_2.md#s2-31n-bare-bounded-fild-m32int-rax-db-00-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31n--bounded-fild-m32int-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31o evidence:** exact bare `DF 00` pushes a signed Word integer through the S2-31n canonical48/Empty-destination preflight and one typed Word read. Sign extension feeds the shared integer-only i64→binary80 packer; zero receives the Zero tag. Literal binary80 values cover zero, ±1/2/3 and signed-word extrema over all TOP×RC settings, including `0x8000`/`0xffff`, stack wrap, stale Empty payload, unrelated tags/payloads, sticky PE and C1 clearing. Unaligned read/canaries prove a two-byte access; canonical range, read #PF/backend error, invalid preflight and fetch/truncation/STOP/budget failures preserve state. Exhaustive DF decode adds only DF 00 while retaining DF 10/18/38/E0/E9/F1; schema/CLI fixtures cover exact DF 00 and DF 00 DF E0; native admission denies the form. C0/C2/C3 retention is local policy. General exception delivery, hardware fault priority, MMU/MMIO/SMP and physical x86 behavior remain unverified; S2-31 stays partial and ISSUE-019 remains open. See [S2-31o CPU contract](specs/CPU_CONTRACT.md#local-s2-31o-bounded-fild-m16int-rax-df-00), [plan](plans/STAGE_2.md#s2-31o-bare-bounded-fild-m16int-rax-df-00-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31o--bounded-fild-m16int-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31p evidence:** exact bare `DF 28` pushes a signed Qword through the shared FILD gates and one complete canonical48-checked Qword read. The generalized integer-only i64→binary80 packer uses `unsigned_abs`, handles `i64::MIN`, emits the exact zero form, and does not depend on RC. Independent literals cover zero, ±1/2/3, i64 extrema, `2^53±1`, `2^53`, and signed values around `2^62` over all TOP×RC pairs. Checks cover occupied/Empty old ST(0), stale Empty payload, other tags/payloads, sticky PE/C1, unaligned canaries/exact read width, noncanonical and crossing ranges, range overflow, read #PF/backend errors, invalid gates, and fetch/truncation/STOP/budget atomicity. Exhaustive DF decode preserves existing DF and DB forms; exact `DF 28`/`DF 28 DF E0` fixture/schema/CLI checks validate bytes independently of case name, and native admission explicitly denies FILD m64. The m16/m32 regression tests exercise the shared packer. General x87 exception delivery, MMU/MMIO/SMP and physical x86 behavior remain unverified; S2-31 stays partial. See [S2-31p CPU contract](specs/CPU_CONTRACT.md#local-s2-31p-bounded-fild-m64int-rax-df-28), [plan](plans/STAGE_2.md#s2-31p-bare-bounded-fild-m64int-rax-df-28-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31p--bounded-fild-m64int-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31r evidence:** exact bare `DB 28` loads canonical-Normal m80fp from `[RAX]` after the shared bounded push gates and complete canonical48 ten-byte preflight. One exact slice read preserves raw bytes; all non-Normal classes reject after the read without CPU commit. Literal interpreter vectors cover every TOP and FCW, raw positive/negative fractional and exponent-boundary values, PE/C1 and local C0/C2/C3 policy, other tags/payloads, no-write behavior, invalid gates/ranges/#PF/backend errors and execution boundaries. Exhaustive DB decode preserves existing forms and rejects DB 30; exact DB 28, DB 28 DF E0 and invalid-class no-commit schema/CLI fixtures are added. Native x87 admission, physical behavior, general exception delivery, MMU/MMIO/SMP remain unverified; S2-31 remains partial. See [S2-31r CPU contract](specs/CPU_CONTRACT.md#local-s2-31r-bounded-fld-m80fp-rax-db-28), [plan](plans/STAGE_2.md#s2-31r-bare-bounded-fld-m80fp-rax-db-28-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31r--bounded-fld-m80fp-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31s evidence:** exact bare `D9 00` loads binary32 Normal `[RAX]` into the pushed physical x87 slot by integer field expansion. Independent raw binary80 literals cover ±1, adjacent fractions, minimum/maximum Normal and exponent 127 across every TOP×RC combination; tests also cover sticky PE/C1 policy, exact unaligned Dword access/canaries, invalid classes after one read, preflight gates, canonical crossing/overflow, #PF/backend errors and execution boundaries. Exact `D9 00`, `D9 00 DF E0`, and invalid-class CLI/schema fixtures are included; native admission denies the operation. Special values, general exception delivery, MMU/MMIO/SMP and physical behavior remain unverified. S2-31 and ISSUE-019 stay open. See [S2-31s CPU contract](specs/CPU_CONTRACT.md#local-s2-31s-bounded-fld-m32fp-rax-d9-00), [plan](plans/STAGE_2.md#s2-31s-bare-bounded-fld-m32fp-rax-d9-00-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31s--bounded-fld-m32fp-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31t evidence:** exact bare `DD 00` expands binary64 Normal `[RAX]` into the pushed x87 slot using integer field conversion, including unbiased exponent +1023. Independent raw binary80 tests cover ±1, adjacent fractions and Normal endpoints over every TOP×FCW combination; exact unaligned Qword access, class rejection, push gates, state retention, exhaustive DD decoding and literal CLI/schema fixtures are included. Native admission denies the operation. Access fault and execution-boundary cases are tracked in the S2-31t interpreter coverage; special values, general exception delivery, MMU/MMIO/SMP and physical behavior remain outside the current contract. S2-31 and ISSUE-019 stay open. See [S2-31t CPU contract](specs/CPU_CONTRACT.md#local-s2-31t-bounded-fld-m64fp-rax-dd-00), [plan](plans/STAGE_2.md#s2-31t-bare-bounded-fld-m64fp-rax-dd-00-partial-slice), [report](reports/STAGE_2_REPORT.md#s2-31t--bounded-fld-m64fp-partial-slice), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31u historical evidence:** extends exact bare `DD 00` to binary64 +0/-0 after the existing single Qword read; it emits signed binary80 zero and the Zero tag. Normal behavior is unchanged. At the S2-31u stage, positive/negative nonzero minimum subnormals and Inf/NaN remained Unsupported without CPU commit; S2-31z now supports masked nonzero subnormals. See current [S2-31z evidence](#s2-31z-evidence). Independent signed-zero vectors cover TOP×RC; rejection vectors verify signed minimum subnormals after one read. Sticky PE/C1, wrap and untouched state are covered. Success and rejection schema/CLI fixtures use positive zero and positive minimum subnormal respectively. Native admission remains closed. The S2-31t report remains historical and records its original Normal-only scope. General x87 exception delivery, MMU/MMIO/SMP and physical acceptance remain open under ISSUE-019; the final combined S2-31 substage still owns physical acceptance. See [S2-31u CPU contract](specs/CPU_CONTRACT.md#local-s2-31u-bounded-fld-m64fp-rax-signed-zero-extension), [plan](plans/STAGE_2.md#s2-31u-extend-bounded-fld-m64fp-rax-dd-00-to-signed-zero), [report](reports/STAGE_2_REPORT.md#s2-31u--bounded-fld-m64fp-signed-zero-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31v historical evidence:** at that point, exact bare `D9 00` accepted binary32 +0/-0, and rejected nonzero subnormals, infinity and NaNs after one read. The S2-31y current contract extends masked binary32 subnormals and replaces the invalid-class subnormal fixture with infinity. Preserve this row and report as the historical S2-31v result; see the current [S2-31y evidence](#s2-31y-evidence).
- **S2-31w evidence:** exact bare `DB 28` now accepts raw binary80 +0/-0 after the unchanged one-read preflight, preserves their ten bytes and assigns `X87Tag::Zero`; Normal remains byte-preserving with Valid. Both signs of minimum nonzero subnormal and other special classes remain Unsupported after one read with complete CPU-state preservation. All TOP×FCW literal matrices, exact +0/−0 CLI fixtures, and the nonzero-subnormal schema exception predicate are covered. Intel SDM093 FLD m80fp specifies no `#D` for this operand; broader denormal/unmasked handling is deferred. Native admission and physical acceptance remain closed; S2-31 and ISSUE-019 stay open. See [S2-31w CPU contract](specs/CPU_CONTRACT.md#local-s2-31w-bounded-fld-m80fp-rax-db-28-signed-zero-extension), [plan](plans/STAGE_2.md#s2-31w-extend-bounded-fld-m80fp-rax-db-28-to-signed-zero), [report](reports/STAGE_2_REPORT.md#s2-31w--bounded-fld-m80fp-signed-zero-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31x historical evidence:** exact bare `DB 28` accepted canonical binary80 subnormals, preserving all ten bytes and assigning `X87Tag::Special` after the same one-read preflight. Literal signed minimum/largest/interior vectors covered all TOP×FCW combinations with unchanged PE/status policy; positive-minimum and negative-largest CLI fixtures covered bare and FNSTSW composition paths. At that stage pseudo-denormal remained Unsupported and the narrow schema exception used positive pseudo-denormal; S2-31aa replaced that exception fixture with canonical QNaN, and S2-31ab replaces the current exception fixture with canonical SNaN while adding canonical QNaN loads. Native admission and physical acceptance remain closed, and broader x87 exception delivery remains open under ISSUE-019. See [S2-31x CPU contract](specs/CPU_CONTRACT.md#local-s2-31x-bounded-fld-m80fp-rax-db-28-canonical-subnormal-extension), [plan](plans/STAGE_2.md#s2-31x-extend-bounded-fld-m80fp-rax-db-28-to-canonical-subnormals), [report](reports/STAGE_2_REPORT.md#s2-31x--bounded-fld-m80fp-canonical-subnormal-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
<a id="s2-31y-evidence"></a>
- **S2-31y evidence:** at that stage, exact bare `D9 00` accepted nonzero binary32 subnormals under four all-masked FCWs, normalized fractions to canonical Normal binary80, assigned Valid, and set sticky DE. Its FSW gate permitted pre-existing DE (`0x80dd`); FILD and m64/m80 load gates stayed unchanged. Signed minimum/interior/largest vectors covered TOP×FCW, repeated loads, sticky PE/C1, UE/PE non-addition and DE retention. Positive D900 and negative D900/DFE0 fixtures exercised subnormal conversion. The S2-31y invalid fixture/schema used binary32 infinity or NaN; S2-31af temporarily narrowed that predicate to canonical QNaN after adding infinities, and S2-31ah now accepts QNaNs and requires canonical SNaN. Unmasked exception delivery, native admission and physical acceptance remain outside the slice; m80fp subnormal behavior stays distinct with no DE. See [S2-31y CPU contract](specs/CPU_CONTRACT.md#local-s2-31y-bounded-fld-m32fp-rax-d9-00-masked-subnormal-extension), [plan](plans/STAGE_2.md#s2-31y-extend-bounded-fld-m32fp-rax-d9-00-to-masked-subnormals), [report](reports/STAGE_2_REPORT.md#s2-31y--bounded-fld-m32fp-masked-subnormal-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
<a id="s2-31z-evidence"></a>
- **S2-31z evidence:** exact bare `DD 00` accepts nonzero binary64 subnormals under the four all-masked FCWs, normalizes their exact fractions to canonical Normal binary80, assigns Valid, and sets sticky DE. Its DD00-only preflight permits pre-existing DE (`0x80dd`); FILD and m80 load gates remain at `0x80df`. Literal signed minimum/interior/largest vectors cover all TOP×FCW pairs, sticky PE/C1, repeated loads and exact Qword reads; CLI fixtures cover positive Dd00 and negative Dd00/DFE0 with FNSTSW. Inf/NaN remain post-read Unsupported, and the schema predicate is exact DD00, budget one and no stop RIP. Unmasked exception delivery and physical acceptance remain outside the slice; native admission stays closed. See [S2-31z CPU contract](specs/CPU_CONTRACT.md#local-s2-31z-bounded-fld-m64fp-rax-dd-00-masked-subnormal-extension), [plan](plans/STAGE_2.md#s2-31z-extend-bounded-fld-m64fp-rax-dd-00-to-masked-subnormals), [report](reports/STAGE_2_REPORT.md#s2-31z--bounded-fld-m64fp-masked-subnormal-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
<a id="s2-31aa-evidence"></a>
- **S2-31aa historical evidence:** exact bare `DB 28` accepts canonical signed binary80 infinities only, preserves all ten raw bytes, and assigns Special under the unchanged gates. Its TOP×FCW matrices verify payload/tag, C1 clear, preserved flags and no new status. At that stage all NaNs were rejected and the narrow schema exception used positive QNaN; S2-31ab supersedes the QNaN behavior and switches the exception fixture to SNaN. Positive bare and negative DB28/DFE0 FNSTSW fixtures run in the CLI; JIT/native admission explicitly denies the operation. Physical acceptance remains deferred to the combined final S2-31 substage. See [S2-31aa CPU contract](specs/CPU_CONTRACT.md#local-s2-31aa-bounded-fld-m80fp-rax-db-28-canonical-infinities), [plan](plans/STAGE_2.md#s2-31aa-extend-bounded-fld-m80fp-rax-db-28-to-canonical-infinities), [report](reports/STAGE_2_REPORT.md#s2-31aa--bounded-fld-m80fp-canonical-infinity-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
<a id="s2-31ab-evidence"></a>
- **S2-31ab evidence:** exact bare `DB 28` also accepts only canonical QuietNaNs (biased exponent `0x7fff`, explicit integer bit and quiet bit set, nonzero fraction), preserves raw bytes, pushes Special, adds no status flags including IE, and preserves PE under the unchanged all-masked FCW/`FSW & 0x80df`/Empty/ten-byte/one-read profile. Independent positive-payload and negative-indefinite literals cover all TOP×FCW pairs. CLI fixtures exercise positive bare QNaN and negative indefinite QNaN with FNSTSW; JIT and native profiles still deny the operation. The narrow exact-DB28/budget-one/no-stop Unsupported exception now uses positive canonical SNaN, and canonical QNaNs, pseudo-NaNs, pseudo-denormals, pseudo-infinities and unnormals do not match it. Mac mini/physical acceptance remains deferred. See [S2-31ab CPU contract](specs/CPU_CONTRACT.md#local-s2-31ab-bounded-fld-m80fp-rax-db-28-canonical-quietnans), [plan](plans/STAGE_2.md#s2-31ab-extend-bounded-fld-m80fp-rax-db-28-to-canonical-qnans), [report](reports/STAGE_2_REPORT.md#s2-31ab--bounded-fld-m80fp-canonical-quietnan-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ac evidence:** exact bare `DB 28` accepts canonical SignalingNaNs (biased exponent `0x7fff`, explicit integer bit set, quiet bit clear, nonzero remaining fraction), preserves raw bytes, pushes Special, adds no flags including IE, and preserves PE/C0/C2/C3 under the unchanged all-masked/`FSW & 0x80df`/Empty/ten-byte/one-read profile. Minimum/middle/maximum payloads of both signs cover every TOP×FCW pair; bare and negative DB28/DFE0 FNSTSW fixtures run in the CLI. The narrow exact-DB28/budget-one/no-stop Unsupported exception uses positive pseudo-infinity; canonical SNaNs and pseudo-SNaNs do not match it. Native/JIT admission and physical acceptance remain closed. Mac mini/physical acceptance is deferred. See [S2-31ac CPU contract](specs/CPU_CONTRACT.md#local-s2-31ac-bounded-fld-m80fp-rax-db-28-canonical-signalingnans), [plan](plans/STAGE_2.md#s2-31ac-extend-bounded-fld-m80fp-rax-db-28-to-canonical-snans), [report](reports/STAGE_2_REPORT.md#s2-31ac--bounded-fld-m80fp-canonical-signalingnan-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ad evidence:** exact bare `DB 28` now also accepts pseudo-denormals (exponent zero, explicit integer bit set), preserving raw bytes/exponent zero, assigning Special, and adding no DE or other flags under the same all-masked/`FSW & 0x80df`/Empty/ten-byte/one-read gates. Signed zero/minimum/maximum fraction literals and exponent-one Normal controls cover TOP×FCW. Positive bare and negative DB28/DFE0 FNSTSW fixtures run through CLI. Pseudo-denormal encodings do not match the exact DB28/budget-one/no-stop pseudo-infinity Unsupported exception. Arithmetic consumption, unmasked exception delivery, native/JIT admission and physical acceptance remain outside scope; Mac mini/physical acceptance is deferred. See [S2-31ad CPU contract](specs/CPU_CONTRACT.md#local-s2-31ad-bounded-fld-m80fp-rax-db-28-pseudo-denormals), [plan](plans/STAGE_2.md#s2-31ad-extend-bounded-fld-m80fp-rax-db-28-to-pseudo-denormals), [report](reports/STAGE_2_REPORT.md#s2-31ad--bounded-fld-m80fp-pseudo-denormal-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **Следующий шаг:** дальнейшую конечную арифметику и architectural x87 effects выбирать отдельными срезами с независимыми vectors и доступным native state observation. S2-30n–p добавляют exact-only FMUL register forms; S2-30q–v add exact-only FDIV/FDIVR directions; S2-30w/x cover raw-copy FSTP/FST, S2-30y/z cover bounded FCHS/FABS sign-bit operations, S2-30aa covers bounded FTST comparison with zero, S2-30af covers bounded FCOMPP, S2-30ag/ah/ai cover bounded FUCOM/FUCOMP/FUCOMPP, and S2-30aj/ak/al add bounded FCOMI/FCOMIP/FUCOMI relation flags and definedness, with FCOMIP's physical TOP pop. These slices require future broader architectural coverage. Native x87 state observation remains unavailable. General rounding, special values, stack faults, exception delivery and CR0 behavior remain open. См. [локальный контракт S2-30y](specs/CPU_CONTRACT.md#local-s2-30y-bounded-fchs-d9-e0), [план S2-30y](plans/STAGE_2.md#s2-30y-bare-bounded-fchs-sign-bit-local-slice), [отчёт S2-30y](reports/STAGE_2_REPORT.md#s2-30y--bounded-fchs-sign-bit-slice), [локальный контракт S2-30z](specs/CPU_CONTRACT.md#local-s2-30z-bounded-fabs-d9-e1), [план S2-30z](plans/STAGE_2.md#s2-30z-bare-bounded-fabs-sign-bit-clear-local-slice), [отчёт S2-30z](reports/STAGE_2_REPORT.md#s2-30z--bounded-fabs-sign-clear-slice), [локальный контракт S2-30x](specs/CPU_CONTRACT.md#local-s2-30x-bounded-fst-sti-dd-d0i), [план S2-30x](plans/STAGE_2.md#s2-30x-bare-bounded-fst-sti-local-raw-copy-slice), [отчёт S2-30x](reports/STAGE_2_REPORT.md#s2-30x--bounded-fst-sti-local-raw-copy-slice), [локальный контракт S2-30v](specs/CPU_CONTRACT.md#local-s2-30v-exact-only-fdiv-sti-st0-dc-f8i), [план S2-30v](plans/STAGE_2.md#s2-30v-bare-exact-only-fdiv-sti-st0-bounded-local-slice), [отчёт S2-30v](reports/STAGE_2_REPORT.md#s2-30v--exact-only-fdiv-sti-st0-bounded-local-slice), and prior S2-30 slices. S2-29 and umbrella S2-28/S2-30 remain open.
- **S2-31ae evidence:** at this stage, exact bare `DD 00` accepted signed binary64 infinities, converting by integer fields to canonical binary80 Special with no new flags. The four FCWs, DD00-local `FSW & 0x80dd` gate, Empty slot, canonical48 Qword preflight/read-once, sticky DE/PE and push/pointer policy remain fixed. TOP×FCW signed literals, subnormal-then-infinity sticky status, QNaN/SNaN one-read rejection, bare and negative DFE0/FNSTSW fixtures, schema precision and native/JIT denial were covered. The Unsupported fixture then used QNaN; S2-31ag supersedes it by supporting canonical QNaNs and narrowing current rejection to SNaN. Unmasked exception delivery, physical behavior and native/JIT execution remain outside scope; binary80 subnormal behavior remains separately specified with no DE. The Linux x86 native-denial path is compile-checked only on the local arm64 host; `.serena` remains an existing layout-check exception. See [S2-31ae CPU contract](specs/CPU_CONTRACT.md#local-s2-31ae-bounded-fld-m64fp-rax-dd-00-signed-infinities), [plan](plans/STAGE_2.md#s2-31ae-extend-bounded-fld-m64fp-rax-dd-00-to-infinities), [report](reports/STAGE_2_REPORT.md#s2-31ae--bounded-fld-m64fp-signed-infinity-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31af evidence:** exact bare `D9 00` accepts signed binary32 infinities (exponent 255, zero fraction) after one Dword read and pushes canonical binary80 infinity with Special tag and no new flags. The four all-masked FCWs, D900-local `FSW & 0x80dd` gate, Empty destination, canonical48 preflight, sticky DE/PE and C1/TOP/pointer policy remain fixed. Signed TOP×FCW literals, subnormal-then-infinity sticky flags, one-read atomic QNaN/SNaN rejection, positive bare and negative D900/DFE0+FNSTSW fixtures, QNaN-only exact-D900 schema/CLI exception, JIT denial and native denial coverage are included. Unmasked exception delivery, physical behavior and native/JIT execution remain outside scope; binary80 m80 subnormals keep their separate no-DE behavior. The architecture-gated native-denial test compiles for Linux x86 but does not run on the local arm64 host; `.serena` remains the pre-existing layout-check exception. See [S2-31af CPU contract](specs/CPU_CONTRACT.md#local-s2-31af-bounded-fld-m32fp-rax-d9-00-signed-infinities), [plan](plans/STAGE_2.md#s2-31af-extend-bounded-fld-m32fp-rax-d9-00-to-infinities), [report](reports/STAGE_2_REPORT.md#s2-31af--bounded-fld-m32fp-signed-infinity-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ag evidence:** exact bare `DD 00` accepts canonical binary64 QNaNs (exponent `0x7ff`, quiet bit set, arbitrary payload) after one Qword read, converting integer fields to canonical binary80 with preserved sign/payload and Special tag. No status bits are added; sticky DE/PE and the existing four FCWs, `FSW & 0x80dd`, Empty destination, C1/TOP/pointer and retirement behavior remain. Signed quiet-only, low/middle/max payload literals span TOP×FCW; subnormal→QNaN checks DE/PE stickiness; SNaN rejection is one-read and atomic. Positive bare and negative payload-bearing DD00/DFE0+FNSTSW fixtures run through CLI. The exact DD00/budget-one/no-stop schema rejection now admits canonical SNaN only. JIT/native remain denied and unmasked behavior is deferred; the local host did not execute the architecture-gated native runtime denial. See [S2-31ag CPU contract](specs/CPU_CONTRACT.md#local-s2-31ag-bounded-fld-m64fp-rax-dd-00-canonical-qnans), [plan](plans/STAGE_2.md#s2-31ag-extend-bounded-fld-m64fp-rax-dd-00-to-canonical-qnans), [report](reports/STAGE_2_REPORT.md#s2-31ag--bounded-fld-m64fp-canonical-qnan-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ah evidence:** exact bare `D9 00` accepts canonical binary32 QNaNs (exponent `0xff`, quiet bit 22 set) of both signs and arbitrary lower payload, converting by integer fields to canonical binary80 with Special tag. No exception flags are added; sticky DE/PE and the four FCWs, `FSW & 0x80dd`, Empty destination, C1/TOP/pointer and retirement policy remain. Quiet-only, low, middle and maximum payload literals span TOP×FCW; subnormal→QNaN checks sticky DE/PE; S2-31ah recorded one-read atomic SNaN rejection; S2-31aj adds masked SNaN support and changes the exact D900/budget-one/no-stop schema exception to an invalid-environment predicate. JIT/native remain denied and unmasked behavior remains deferred; Linux x86 native-denial compilation is checked but runtime denial is not executed on the local arm64 host. See [S2-31ah CPU contract](specs/CPU_CONTRACT.md#local-s2-31ah-bounded-fld-m32fp-rax-d9-00-canonical-qnans), [plan](plans/STAGE_2.md#s2-31ah-extend-bounded-fld-m32fp-rax-d9-00-to-canonical-qnans), [report](reports/STAGE_2_REPORT.md#s2-31ah--bounded-fld-m32fp-canonical-qnan-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ai evidence:** exact bare `DD 00` accepts canonical binary64 SNaNs (exponent `0x7ff`, nonzero fraction, quiet bit clear) after one Qword read. Integer conversion sets binary80’s explicit integer and quiet bits, shifts payload by eleven, preserves sign, and assigns Special. Sticky IE alone is added; sticky DE/PE are preserved, and ES/SF/B or other flags are not introduced. Existing all-masked FCWs, `FSW & 0x80dd`, Empty destination, C1/TOP/pointer/retirement behavior remain. Signed low/middle/max payload literals cover TOP×FCW; FSW/FNSTSW, DE/PE stickiness, memory/fault atomicity, pre-read IM=0 and pre-existing-IE rejection, subsequent-load rejection and FNCLEX recovery are covered. Bare positive and negative DD00/DFE0+FNSTSW fixtures run through CLI; JIT/native remain denied. The exact DD00/budget-one/no-stop schema exception now represents an invalid FCW or status gate, independent of operand class. Unmasked behavior and physical acceptance remain outside scope. See [S2-31ai CPU contract](specs/CPU_CONTRACT.md#local-s2-31ai-bounded-fld-m64fp-rax-dd-00-canonical-snans), [plan](plans/STAGE_2.md#s2-31ai-extend-bounded-fld-m64fp-rax-dd-00-to-canonical-snans), [report](reports/STAGE_2_REPORT.md#s2-31ai--bounded-fld-m64fp-canonical-snan-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31aj evidence:** exact bare `D9 00` accepts canonical binary32 SNaNs (exponent `0xff`, nonzero fraction, quiet bit 22 clear) after one Dword read. Integer conversion sets binary80’s explicit integer and quiet bits, shifts payload by 40, preserves sign, and assigns Special. Sticky IE alone is added; sticky DE/PE and other admitted flags remain unchanged. Existing all-masked FCWs, `FSW & 0x80dd`, Empty destination, C1/TOP/pointer/retirement behavior remain. Signed low/middle/max payload literals cover TOP×FCW; clean TOP=0 FSW is `0x3801`; SNaN→quieted-QNaN, DE/PE stickiness, memory faults, pre-read unmasked-FCW/pre-existing-IE gates, subsequent-load rejection, and FNCLEX recovery are covered. Bare positive and negative D900/DFE0+FNSTSW fixtures run through CLI; native/JIT remain denied. The exact D900/budget-one/no-stop schema exception requires an invalid FCW or status bit in `0x80dd`, independent of operand class. Unmasked delivery and physical acceptance remain outside scope. See [S2-31aj CPU contract](specs/CPU_CONTRACT.md#local-s2-31aj-bounded-fld-m32fp-rax-d9-00-canonical-snans), [plan](plans/STAGE_2.md#s2-31aj-extend-bounded-fld-m32fp-rax-d9-00-to-canonical-snans), [report](reports/STAGE_2_REPORT.md#s2-31aj--bounded-fld-m32fp-canonical-snan-extension), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ak evidence:** exact D900/DD00 alone now admits all IE/DE/PE sticky combinations by changing the local mask from `0x80dd` to `0x80dc`; ZE/OE/UE/SF/ES/B still reject before read. The full sticky-combination matrix spans source classes, FCWs and TOP, with same-width/cross-width sequences, repeated SNaNs, second-load fault atomicity, FNCLEX and unchanged FILD/m80 gates. Exact D900/DD00 budget-one/no-stop schema predicates now use forbidden status rather than source-class exceptions; existing CLI/FNSTSW SNaN fixtures observe IE. Native/JIT and unmasked delivery remain outside the contract, so ISSUE-019 stays open. See [S2-31ak CPU contract](specs/CPU_CONTRACT.md#local-s2-31ak-sticky-exception-flags-for-bounded-m32m64-fld), [plan](plans/STAGE_2.md#s2-31ak-preserve-sticky-ie-de-pe-across-exact-m32m64-fld-loads), [report](reports/STAGE_2_REPORT.md#s2-31ak--sticky-ie-de-pe-across-bounded-m32m64-fld), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31al evidence:** exact D910/D918 now also store signed binary80 Zero when physical TOP is tagged Zero and raw exponent/significand are canonical zero. The sign maps to binary32 `+0`/`-0`; existing FCW/`FSW & 0x80df` gates and atomic Dword write remain. FST preserves stack state; FSTP changes TOP/tag only after successful store. Tests cover TOP×FCW×sign×PE/C1, FLD-zero compositions, mismatched encodings, backend/#PF and range atomicity, literal CLI/FNSTSW fixtures and native/JIT denial. General x87 special arithmetic and physical behavior remain open under ISSUE-019. See [S2-31al CPU contract](specs/CPU_CONTRACT.md#local-s2-31al-signed-zero-fst-m32fp-and-fstp-m32fp), [plan](plans/STAGE_2.md#s2-31al-store-signed-binary80-zero-through-bounded-m32-fstfstp), [report](reports/STAGE_2_REPORT.md#s2-31al--signed-zero-stores-for-bounded-m32-fstfstp), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31am evidence:** exact DD10/DD18 now store signed canonical binary80 Zero as binary64 `+0`/`-0`, requiring both the physical Zero tag and zero exponent/significand. The existing store FCW/`FSW & 0x80df` gates remain; one atomic Qword write precedes any FSTP pop, with no data read. TOP×FCW×sign×PE/C1 matrices, cross-width FLD-zero compositions, status/source rejection, write faults, canonical boundary checks, literal CLI/FNSTSW fixtures, and native/JIT denial are covered. Full x87 special arithmetic and physical behavior remain open under ISSUE-019. See [S2-31am CPU contract](specs/CPU_CONTRACT.md#local-s2-31am-signed-zero-fst-m64fp-and-fstp-m64fp), [plan](plans/STAGE_2.md#s2-31am-store-signed-binary80-zero-through-bounded-m64-fstfstp), [report](reports/STAGE_2_REPORT.md#s2-31am--signed-zero-stores-for-bounded-m64-fstfstp), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31an evidence:** exact DB38 FSTP m80fp now also stores raw signed canonical binary80 zero when physical TOP has the matching Zero tag; all ten bytes are preserved. The existing FCW/`FSW & 0x80df` gate, full-range preflight, one atomic write/no read, and post-success pop remain. Normal sources are unchanged. TOP×FCW×sign×PE/C1, canaries, mismatched source rejection, literal CLI/FNSTSW fixtures, and native/JIT denial are covered. General x87 special arithmetic and physical behavior remain open under ISSUE-019. See [S2-31an CPU contract](specs/CPU_CONTRACT.md#local-s2-31an-signed-zero-fstp-m80fp-rax-db-38), [plan](plans/STAGE_2.md#s2-31an-store-canonical-signed-binary80-zero-through-bounded-fstp-m80fp), [report](reports/STAGE_2_REPORT.md#s2-31an--signed-zero-binary80-stores-through-bounded-fstp-m80fp), and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ao evidence:** exact DB38 FSTP m80fp also stores canonical signed binary80 infinity with Special tag, preserving its raw ten-byte representation. Existing Valid+Normal and Zero+Zero pairs, four FCWs, `FSW & 0x80df`, no-read one-write, post-success pop, and pointer/retirement policy remain. Pseudo-infinity, NaNs, finite special encodings, and mismatched tags reject before access. TOP×FCW×sign×PE/C1, cross-width infinity loads, memory/range atomicity, CLI/FNSTSW and native/JIT denial are covered. General x87 exception delivery and physical behavior remain open under ISSUE-019. See [S2-31ao CPU contract](specs/CPU_CONTRACT.md#local-s2-31ao-signed-infinity-fstp-m80fp-rax-db-38), [plan](plans/STAGE_2.md#s2-31ao-store-canonical-signed-binary80-infinity-through-bounded-fstp-m80fp), [report](reports/STAGE_2_REPORT.md#s2-31ao--signed-infinity-binary80-store-through-bounded-fstp-m80fp), and [fixture notes](../tests/fixtures/x86/stage2/README.md).


- **S2-31ap evidence:** exact DD10/DD18 m64 stores now accept canonical signed
  binary80 infinity only with physical Special tag, emitting literal binary64
  infinity without new flags. Existing Normal/zero paths and their gates remain;
  Qword store is one-write/no-read, with FSTP pop after success. Matrices,
  all-width load/store sequences, malformed source/status rejection, atomicity,
  CLI/FNSTSW and native/JIT denial are covered. Broader x87 exception delivery
  and physical behavior remain open under ISSUE-019. See [S2-31ap CPU
  contract](specs/CPU_CONTRACT.md#local-s2-31ap-signed-infinity-fstfstp-m64fp-rax),
  [plan](plans/STAGE_2.md#s2-31ap-store-canonical-signed-binary80-infinity-through-bounded-m64-fstfstp),
  [report](reports/STAGE_2_REPORT.md#s2-31ap--signed-infinity-stores-for-bounded-m64-fstfstp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31ax evidence (historical gate, superseded by S2-31ay):** exact D910/D918 m32 stores now convert Special-tagged
  canonical binary80 SNaNs to quiet binary32 values, preserving sign and the
  upper 22 payload bits while truncating lower 40 without PE. Sticky IE is
  committed only after the atomic Dword write succeeds; source bytes and sticky
  PE remain unchanged, and D918 pops only after success. At the S2-31ax stage,
  the store gate blocked subsequent stores until FNCLEX cleared IE; S2-31ay
  now admits sticky IE for the exact m32/m64 FST/FSTP conversion helpers.
  Native runtime and physical
  acceptance remain unverified and deferred to the final combined S2-31
  substage under ISSUE-019. See [S2-31ax CPU contract](specs/CPU_CONTRACT.md#local-s2-31ax-canonical-signalingnan-fstfstp-m32fp-rax-d9-10-d9-18),
  [plan](plans/STAGE_2.md#s2-31ax-store-canonical-binary80-signalingnan-through-bounded-fstfstp-m32fp),
  [report](reports/STAGE_2_REPORT.md#s2-31ax--canonical-signalingnan-binary80-to-binary32-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31ay evidence:** exact D910/D918 and DD10/DD18 conversion helpers now
  admit masked sticky IE by changing only their local FSW mask to `0x80de`.
  ZE/DE/OE/UE/SF/ES/B, FCW restrictions, source class/tag rules, canonical range
  checks, and all other x87 instruction gates remain unchanged. Incoming IE/PE
  is preserved; successful SNaN writes OR IE and inexact finite writes OR PE
  only after the atomic store. Repeated stores remain available with IE set;
  FNCLEX can still clear status. Full validation, CLI fixture reachability, and
  independent review are recorded in the [S2-31ay report](reports/STAGE_2_REPORT.md#s2-31ay--sticky-ie-m32-m64-stores).
  Native runtime and physical acceptance remain deferred to the final combined
  S2-31 substage. See the [CPU contract](specs/CPU_CONTRACT.md#local-s2-31ay-admit-sticky-ie-fstfstp-m32-m64),
  [plan](plans/STAGE_2.md#s2-31ay-admit-sticky-ie-in-bounded-m32-m64-fst-fstp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31aq evidence:** exact D910/D918 now store Special-tagged canonical
  binary80 infinity as signed binary32 infinity using integer fields. The
  existing FCW/FSW gates, one-write/no-read Dword access, and post-success FSTP
  pop remain. Normal/zero paths and native/JIT denial are unchanged. Full
  general x87 exception delivery and physical behavior remain open under
  ISSUE-019; the final combined S2-31 substage owns physical acceptance. See
  [S2-31aq CPU contract](specs/CPU_CONTRACT.md#local-s2-31aq-signed-infinity-fstfstp-m32fp-rax),
  [plan](plans/STAGE_2.md#s2-31aq-store-canonical-signed-binary80-infinity-through-bounded-m32-fstfstp),
  [report](reports/STAGE_2_REPORT.md#s2-31aq--signed-infinity-stores-for-bounded-m32-fstfstp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31ar evidence:** exact DB38 now also stores canonical binary80 QuietNaN
  with a Special-tagged TOP, preserving the full ten-byte sign/payload without
  new flags. Existing Normal, Zero and Infinity pairs, the four-FCW/FSW gate,
  one-write/no-read access and pop-after-success remain. SNaN and unsupported
  encodings reject before access. General x87 exception delivery and physical
  behavior remain open under ISSUE-019; physical acceptance belongs to the
  combined final S2-31 substage. See [S2-31ar CPU contract](specs/CPU_CONTRACT.md#local-s2-31ar-canonical-quietnan-fstp-m80fp-rax-db-38),
  [plan](plans/STAGE_2.md#s2-31ar-store-canonical-binary80-quietnan-through-bounded-fstp-m80fp),
  [report](reports/STAGE_2_REPORT.md#s2-31ar--canonical-quietnan-binary80-store-through-bounded-fstp-m80fp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31as evidence:** exact DB38 now also raw-stores canonical binary80
  SignalingNaN from a Special-tagged TOP, preserving the signaling bit and
  payload without setting IE. Existing status gates, one-write/no-read access,
  and pop-after-success remain. General x87 exception delivery and physical
  behavior remain open under ISSUE-019; physical acceptance belongs to the
  combined final S2-31 substage. See [S2-31as CPU contract](specs/CPU_CONTRACT.md#local-s2-31as-canonical-signalingnan-fstp-m80fp-rax-db-38),
  [plan](plans/STAGE_2.md#s2-31as-store-canonical-binary80-signalingnan-through-bounded-fstp-m80fp),
  [report](reports/STAGE_2_REPORT.md#s2-31as--canonical-signalingnan-binary80-store-through-bounded-fstp-m80fp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31at evidence:** exact DB38 now also raw-stores canonical binary80
  subnormals from a Special-tagged TOP, preserving sign and all fraction bits
  without new DE/UE/PE. Existing status gates, one-write/no-read access, and
  pop-after-success remain. General x87 exception delivery and physical
  behavior remain open under ISSUE-019; physical acceptance belongs to the
  combined final S2-31 substage. See [S2-31at CPU contract](specs/CPU_CONTRACT.md#local-s2-31at-canonical-subnormal-fstp-m80fp-rax-db-38),
  [plan](plans/STAGE_2.md#s2-31at-store-canonical-binary80-subnormal-through-bounded-fstp-m80fp),
  [report](reports/STAGE_2_REPORT.md#s2-31at--canonical-subnormal-binary80-store-through-bounded-fstp-m80fp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31au evidence:** exact DD10/DD18 m64 stores also convert a Special-tagged
  canonical binary80 QuietNaN to binary64 by preserving sign and the upper 52
  fraction bits, truncating the lower 11 without new flags. Existing gates and
  atomic write/pop behavior remain. General x87 exception delivery, native
  runtime admission and physical behavior remain open under ISSUE-019; physical
  acceptance belongs to the final combined S2-31 substage. See [S2-31au CPU
  contract](specs/CPU_CONTRACT.md#local-s2-31au-canonical-quietnan-fstfstp-m64fp-rax-dd-10-dd-18),
  [plan](plans/STAGE_2.md#s2-31au-store-canonical-binary80-quietnan-through-bounded-fstfstp-m64fp),
  [report](reports/STAGE_2_REPORT.md#s2-31au--canonical-quietnan-binary80-to-binary64-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31av evidence:** exact D910/D918 m32 stores now convert Special-tagged
  canonical binary80 QuietNaNs to binary32, preserving sign and upper 23 payload
  bits while truncating lower 40 without rounding or new flags. Existing gates and
  atomic write/pop behavior remain. General x87 exception delivery, native
  runtime admission and physical behavior remain open under ISSUE-019; physical
  acceptance belongs to the final combined S2-31 substage. See [S2-31av CPU
  contract](specs/CPU_CONTRACT.md#local-s2-31av-canonical-quietnan-fstfstp-m32fp-rax-d9-10-d9-18),
  [plan](plans/STAGE_2.md#s2-31av-store-canonical-binary80-quietnan-through-bounded-fstfstp-m32fp),
  [report](reports/STAGE_2_REPORT.md#s2-31av--canonical-quietnan-binary80-to-binary32-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31aw evidence:** exact DD10/DD18 m64 stores now accept canonical
  Special-tagged binary80 SNaNs, quiet the binary64 destination, and add sticky
  IE only after a successful atomic write. The upper 51 payload bits and sign
  survive, the lower 11 truncate without PE, and the raw source remains
  unchanged; DD18 pops only after success. FSW gates, PE, C1/TOP behavior and
  ES/B/SF policy are covered along with FNCLEX recovery; native runtime and physical
  acceptance remain unverified and deferred to the final combined S2-31
  substage. See [S2-31aw CPU contract](specs/CPU_CONTRACT.md#local-s2-31aw-canonical-signalingnan-fstfstp-m64fp-rax-dd-10-dd-18),
  [plan](plans/STAGE_2.md#s2-31aw-store-canonical-binary80-signalingnan-through-bounded-fstfstp-m64fp),
  [report](reports/STAGE_2_REPORT.md#s2-31aw--canonical-signalingnan-binary80-to-binary64-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31az evidence:** exact D910/D918 and DD10/DD18 store helpers now admit
  sticky masked DE together with IE/PE by changing only their local FSW mask to
  `0x80dc`. ZE/OE/UE/SF/ES/B, FCWs, classes/tags, finite-result limits and other
  x87 gates remain unchanged. Incoming IE/DE/PE is preserved; SNaN adds IE and
  inexact finite conversion adds PE only after the atomic write, with no new DE.
  Cross-width binary32-subnormal FLD→binary64 FST/FSTP fixtures pass through
  interpreter; explicit JIT-denial checks pass, and native-admission denial is
  covered by an x86-target unit test that compiled but could not run on this
  arm64 host. The full offline workspace suite passed 775 tests and workspace
  plus Linux x86_64 runner checks passed. Independent Sol-medium review found no blocking defects.
  Native runtime and physical acceptance remain deferred to the final combined
  S2-31 substage under ISSUE-019. See [S2-31az CPU contract](specs/CPU_CONTRACT.md#local-s2-31az-admit-sticky-de-m32-m64-fstfstp),
  [plan](plans/STAGE_2.md#s2-31az-admit-sticky-de-in-bounded-m32-m64-fst-fstp),
  [report](reports/STAGE_2_REPORT.md#s2-31az--sticky-de-in-bounded-m32-m64-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31ba evidence:** exact DB38 raw FSTP now admits pre-existing masked IE
  and DE alongside PE by changing only its local gate to `0x80dc`. It preserves
  source bytes and incoming IE/DE/PE, generates no status flags, clears C1,
  preserves C0/C2/C3, and changes TOP/tag only after its one atomic ten-byte
  write. The existing source/tag pairs and all FCW/address checks remain.
  Cross-width subnormal/SNaN FLD compositions, raw-SNaN narrow-FST composition,
  all eight sticky combinations, forbidden controls, fault atomicity, CLI,
  JIT-denial and x86-target native-denial coverage are included. Native runtime
  is unexecuted on this arm64 host and physical acceptance remains deferred to
  the final combined S2-31 substage under ISSUE-019. See [S2-31ba CPU contract](specs/CPU_CONTRACT.md#local-s2-31ba-sticky-ie-de-raw-fstp-m80fp-db-38),
  [plan](plans/STAGE_2.md#s2-31ba-admit-sticky-ie-de-to-raw-binary80-fstp),
  [report](reports/STAGE_2_REPORT.md#s2-31ba--sticky-ie-de-on-raw-binary80-fstp-m80fp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31bb evidence:** exact D910/D918 finite m32 stores now admit canonical
  Normal binary80 sources through unbiased exponent 127. Existing integer
  rounding normalizes carry before the final range check: 126→127 is supported,
  while 127→128 and an exponent-128 input reject before data access and do not
  set OE. Sticky IE/DE/PE and all other gates remain unchanged; subnormal store
  results remain Unsupported. Separate signed max-finite FLD and store fixtures
  exercise exponent 127 through interpreter/schema, with JIT/native denial
  coverage.
  Native runtime and physical acceptance remain deferred under ISSUE-019. See
  [S2-31bb CPU contract](specs/CPU_CONTRACT.md#local-s2-31bb-finite-exponent-127-bounded-m32-stores),
  [plan](plans/STAGE_2.md#s2-31bb-finite-exponent-127-bounded-m32-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31bb--finite-exponent-127-bounded-m32-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31bc evidence:** exact DD10/DD18 finite m64 stores now admit canonical
  Normal binary80 sources through unbiased exponent 1023. Carry is normalized
  before the final range check: 1022→1023 is supported, while 1023→1024 and a
  source at exponent 1024 reject before access without setting OE. Subnormal
  store results, minimum exponent and all existing gates remain unchanged.
  Separate signed max-finite FLD and store fixtures exercise the interpreter
  path, with JIT/native denial coverage. Native runtime and physical acceptance
  remain deferred under ISSUE-019. See [S2-31bc CPU contract](specs/CPU_CONTRACT.md#local-s2-31bc-finite-exponent-1023-bounded-m64-stores),
  [plan](plans/STAGE_2.md#s2-31bc-finite-exponent-1023-bounded-m64-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31bc--finite-exponent-1023-bounded-m64-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31bd evidence:** exact D910/D918 stores now accept only exactly
  representable binary32 subnormal results from canonical Normal binary80
  sources with unbiased exponents -149 through -127. Integer downshift checks
  all discarded bits; inexact tiny results and sources below -149 remain
  Unsupported before access. Sticky flags are retained without generating DE,
  UE or PE. Boundary, FLD→store, write/range atomicity, CLI and JIT/native
  denial coverage is included. Native runtime and physical acceptance remain
  deferred under ISSUE-019. See [S2-31bd CPU contract](specs/CPU_CONTRACT.md#local-s2-31bd-exact-binary32-subnormal-results),
  [plan](plans/STAGE_2.md#s2-31bd-exact-binary32-subnormal-results-through-m32-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31bd--exact-binary32-subnormal-results-through-m32-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31be evidence:** exact DD10/DD18 stores now admit only representable
  binary64 subnormal results from canonical Normal binary80 sources with
  exponents -1074 through -1023. Integer downshift rejects nonzero discarded
  bits and values below the minimum subnormal before data access. Sticky flags
  remain unchanged and no DE/UE/PE is generated. Boundary, FLD→store,
  write/range atomicity, CLI and JIT/native denial coverage is included.
  Native runtime and physical acceptance remain deferred under ISSUE-019. See
  [S2-31be CPU contract](specs/CPU_CONTRACT.md#local-s2-31be-exact-binary64-subnormal-results),
  [plan](plans/STAGE_2.md#s2-31be-exact-binary64-subnormal-results-through-m64-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31be--exact-binary64-subnormal-results-through-m64-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31bf evidence:** masked inexact D910/D918 stores round only canonical
  Normal binary80 sources in the binary32 subnormal exponent range. They accept
  nonzero subnormal results, reject zero/minimum-normal promotion before access,
  and defer sticky UE+PE with C1 magnitude-increment state until after the
  atomic Dword write. Exact behavior is unchanged; no DE is added. Tie, shift,
  TOP/FCW/sticky-flag matrices, FNSTSW/FNCLEX, atomicity, CLI and JIT/native
  denial coverage is included. Native runtime and physical acceptance remain
  deferred under ISSUE-019. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-31bf-masked-inexact-binary32-subnormal-results),
  [plan](plans/STAGE_2.md#s2-31bf-masked-inexact-binary32-subnormal-results-through-m32-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31bf--masked-inexact-binary32-subnormal-results-through-m32-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31bg evidence:** masked inexact DD10/DD18 stores now round canonical
  Normal binary80 sources with exponents -1074 through -1023. They accept only
  nonzero binary64 subnormal fractions, reject below-range and promotion cases
  before access, and defer sticky UE+PE with C1 magnitude-increment state until
  after atomic Qword storage; exact behavior is unchanged and no DE is added.
  TOP/opcode/FCW/sign/sticky matrices, every shift, upper-bound checks,
  FNSTSW/FNCLEX, the then-current UE gate (superseded by S2-31bh),
  write/range atomicity, CLI and JIT/native denial are covered. Native runtime
  and physical acceptance remain deferred under this
  open issue. See [S2-31bg CPU contract](specs/CPU_CONTRACT.md#local-s2-31bg-masked-inexact-binary64-subnormal-results),
  [plan](plans/STAGE_2.md#s2-31bg-masked-inexact-binary64-subnormal-results-through-m64-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31bg--masked-inexact-binary64-subnormal-results-through-m64-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31bh evidence:** the m32/m64 FST/FSTP gates now admit incoming masked UE
  with IE/DE/PE while continuing to reject ZE/OE/SF/ES/B. UE alone does not
  synthesize PE, and any conversion-generated flags or FSTP state changes commit
  only after the atomic store. All 16 sticky combinations, repeated underflow,
  cross-width exact storage from the unchanged source, FNSTSW/FNCLEX, fault
  atomicity, CLI, and JIT/native denial are covered. Native runtime and physical
  acceptance remain deferred under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31bh-sticky-ue-re-entry-for-bounded-m32m64-stores),
  [plan](plans/STAGE_2.md#s2-31bh-admit-sticky-ue-in-bounded-binary32binary64-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31bh--sticky-ue-reentry-for-bounded-m32-m64-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).


- **S2-31bi evidence:** raw DB38 FSTP m80fp now admits incoming masked UE with
  IE/DE/PE under the narrowed `0x80cc` gate while retaining the existing source
  classes/tags, ten-byte payload and no-generated-flags policy. UE-only preserves
  a signaling NaN without adding IE/PE, normalizing or quieting it. All 16 sticky
  combinations, cross-width underflow-to-raw-store composition, FNSTSW/FNCLEX,
  write/range atomicity, CLI, and JIT/native denial are covered. Native runtime
  and physical acceptance remain deferred under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31bi-sticky-ue-on-raw-binary80-fstp-m80fp),
  [plan](plans/STAGE_2.md#s2-31bi-admit-sticky-ue-in-raw-binary80-fstp-m80fp),
  [report](reports/STAGE_2_REPORT.md#s2-31bi--sticky-ue-on-raw-binary80-fstp-m80fp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31bj evidence:** bounded D9 10/D9 18 now round canonical binary80
  exponents -16382..=-150 directly to signed zero or minimum binary32 subnormal,
  using explicit half comparison at -150 and avoiding oversized shifts.
  Inexact UE+PE and C1 commit only after atomic storage. CLI/JIT/native denial and
  ISA matrix coverage are documented in [contract](specs/CPU_CONTRACT.md#local-s2-31bj-extreme-binary32-underflow-to-zero-or-minimum-subnormal),
  [plan](plans/STAGE_2.md#s2-31bj-round-extreme-underflow-through-binary32-stores),
  and [report](reports/STAGE_2_REPORT.md#s2-31bj--extreme-binary32-underflow-to-zero-or-minimum-subnormal).
  Native runtime and physical acceptance remain deferred under ISSUE-019.

- **S2-31bk evidence:** bounded DD 10/DD 18 now round canonical binary80
  exponents -16382..=-1075 directly to signed zero or minimum binary64 subnormal,
  using an explicit half comparison at -1075 and avoiding oversized shifts.
  Inexact UE+PE and C1 commit only after atomic storage. CLI/JIT/native denial and
  ISA matrix coverage are documented in [contract](specs/CPU_CONTRACT.md#local-s2-31bk-extreme-binary64-underflow-to-zero-or-minimum-subnormal),
  [plan](plans/STAGE_2.md#s2-31bk-round-extreme-underflow-through-binary64-stores),
  and [report](reports/STAGE_2_REPORT.md#s2-31bk--extreme-binary64-underflow-to-zero-or-minimum-subnormal).
  Native runtime and physical acceptance remain deferred under ISSUE-019.

- **S2-31bl evidence:** bounded D9 10/D9 18 now round Special-tagged canonical
  binary80 subnormal sources to signed zero or signed minimum binary32 subnormal
  using the existing RC. All admitted results commit UE+PE after atomic storage;
  C1 tracks minimum-subnormal selection. ISA matrix, CLI fixture and JIT/native
  denial are documented in [contract](specs/CPU_CONTRACT.md#local-s2-31bl-binary80-subnormal-to-binary32-underflow-rounding),
  [plan](plans/STAGE_2.md#s2-31bl-round-binary80-subnormals-through-binary32-stores),
  and [report](reports/STAGE_2_REPORT.md#s2-31bl--binary80-subnormal-to-binary32-underflow-rounding).
  Native runtime and physical acceptance remain deferred under ISSUE-019.
- **S2-31bm evidence:** bounded DD10/DD18 now accept Special-tagged canonical
  binary80 subnormal sources and round them directly to signed zero or minimum
  binary64 subnormal. UE+PE and C1 commit only after successful Qword storage;
  no DE is added, and FSTP pop/status/pointers remain atomic. Four payloads,
  signs, RCs, TOPs, sticky flags, invalid encodings/gates, FNSTSW/FNCLEX,
  raw-m80 preservation, CLI fixtures and JIT denial are covered. Native runtime
  and physical x86 acceptance remain unverified under open ISSUE-019. See the
  [S2-31bm CPU contract](specs/CPU_CONTRACT.md#local-s2-31bm-binary80-subnormal-to-binary64-underflow-rounding),
  [plan](plans/STAGE_2.md#s2-31bm-round-binary80-subnormals-through-binary64-stores),
  and [report](reports/STAGE_2_REPORT.md#s2-31bm--binary80-subnormal-to-binary64-underflow-rounding).
- **S2-31bn evidence:** bounded D910/D918 now admit only exponent -127 Normal
  sources whose direct `S/2^41` rounding selects binary32 minimum normal. PE+C1
  follow the destination rounding; UE is independently decided by rounded
  `S/2^40`, with sticky UE preserved when no new underflow is indicated. The
  threshold oracle covers signed RC neighbors, the separate 24-bit UE boundary,
  all TOPs/sticky IE/DE/UE/PE states, both opcodes, atomic Dword failures,
  FNSTSW/FNCLEX and interpreter CLI fixtures; JIT/native admission remain
  closed. Native runtime and physical x86 acceptance remain unverified under
  open ISSUE-019. See the [S2-31bn CPU contract](specs/CPU_CONTRACT.md#local-s2-31bn-binary80-exponent--127-to-binary32-minimum-normal),
  [plan](plans/STAGE_2.md#s2-31bn-round-exponent--127-binary80-normals-directly-to-binary32-minimum-normal),
  and [report](reports/STAGE_2_REPORT.md#s2-31bn--binary80-normal-to-binary32-minimum-normal-rounding).
- **S2-31bo evidence:** bounded DD10/DD18 admit exponent -1023 Normal sources
  only when direct `S/2^12` rounding selects binary64 minimum normal. PE+C1
  follow that destination result; UE is independently decided by rounding the
  unbounded 53-bit significand from `S>>11`. Threshold/oracle coverage spans
  signed RC neighbors, the separate UE boundary, all TOPs/sticky IE/DE/UE/PE
  combinations, both opcodes, atomic Qword failures, FNSTSW/FNCLEX and CLI
  fixtures; JIT/native admission remain closed. Native runtime and physical
  acceptance remain deferred under ISSUE-019. See the [CPU contract](specs/CPU_CONTRACT.md#local-s2-31bo-binary80-exponent--1023-to-binary64-minimum-normal),
  [plan](plans/STAGE_2.md#s2-31bo-round-exponent--1023-binary80-normals-directly-to-binary64-minimum-normal),
  and [report](reports/STAGE_2_REPORT.md#s2-31bo--binary80-normal-to-binary64-minimum-normal-rounding).
- **S2-31bp evidence:** bounded D910/D918 exponent-127 Normal inputs whose
  sign-aware rounding carries `S/2^40` to `1<<24` now store signed binary32
  infinity and commit sticky OE+PE with C1 after the Dword write. The test
  matrix covers nearest/directed thresholds and neighbors, both signs, RCs,
  opcodes, TOPs and sticky IE/DE/UE/PE combinations; exponent-128 sources
  still reject. Atomicity, FNSTSW/FNCLEX, sticky-OE re-entry, CLI and JIT/native
  denial are covered. Native runtime and physical acceptance remain deferred
  under ISSUE-019. See the [contract](specs/CPU_CONTRACT.md#local-s2-31bp-binary80-exponent-127-overflow-to-binary32-infinity),
  [plan](plans/STAGE_2.md#s2-31bp-round-exponent-127-binary80-normals-to-binary32-infinity-on-overflow),
  and [report](reports/STAGE_2_REPORT.md#s2-31bp--binary80-normal-to-binary32-infinity-on-overflow).

- **S2-31bq evidence:** bounded DD10/DD18 exponent-1023 Normal values that
  round beyond maximum finite binary64 now store signed infinity and commit
  OE+PE+C1 after a successful Qword write. Exact source vectors cover nearest
  and directed thresholds, signs, RCs, both opcodes/TOPs and all 16 sticky
  IE/DE/UE/PE combinations; exponent-1024 still rejects. Atomicity, FNSTSW,
  FNCLEX, CLI fixtures and JIT/native denial are covered. Native runtime and
  physical acceptance remain deferred under ISSUE-019. See the [contract](specs/CPU_CONTRACT.md#local-s2-31bq-binary80-exponent-1023-overflow-to-binary64-infinity),
  [plan](plans/STAGE_2.md#s2-31bq-round-exponent-1023-binary80-normals-to-binary64-infinity-on-overflow),
  and [report](reports/STAGE_2_REPORT.md#s2-31bq--binary80-normal-to-binary64-infinity-on-overflow).

- **S2-31br evidence:** bounded D910/D918 and DD10/DD18 stores now admit all
  prior sticky IE/DE/OE/UE/PE combinations while ZE/SF/ES/B remain rejected.
  Conversion-generated flags and C1 are recomputed after a successful write;
  OE alone is preserved without adding PE. ISA matrices, overflow re-entry,
  FNSTSW/FNCLEX, atomicity and CLI/JIT/native-denial fixture checks cover this
  slice. Native runtime and physical acceptance remain deferred under
  ISSUE-019. See the [contract](specs/CPU_CONTRACT.md#local-s2-31br-sticky-status-admission-for-bounded-m32m64-fstfstp),
  [plan](plans/STAGE_2.md#s2-31br-admit-sticky-status-on-bounded-m32m64-stores),
  and [report](reports/STAGE_2_REPORT.md#s2-31br--sticky-status-admission-for-bounded-m32m64-stores).

- **S2-31bs evidence:** exact DB38 raw FSTP m80fp now admits incoming sticky OE
  with IE/DE/UE/PE under `FSW & 0x80c4`, while ZE/SF/ES/B remain rejected.
  The write preserves all ten source bytes and generates no flags; C1 clearing,
  status, pointers and pop remain atomic after the single write. Coverage spans
  all 32 sticky combinations, cross-width overflow-to-raw-store composition,
  OE-only SNaN/CLI, FNSTSW/FNCLEX, fault/range atomicity, and JIT/native denial.
  Native runtime and physical acceptance remain deferred under ISSUE-019. See
  [contract](specs/CPU_CONTRACT.md#local-s2-31bs-sticky-oe-on-raw-binary80-fstp-m80fp),
  [plan](plans/STAGE_2.md#s2-31bs-admit-sticky-oe-to-raw-binary80-fstp),
  and [report](reports/STAGE_2_REPORT.md#s2-31bs--sticky-oe-on-raw-binary80-fstp-m80fp).

- **S2-31bt evidence:** exact `DB 10`/`DB 18` m32 integer stores admit only
  Zero-tagged canonical binary80 zero with exponent/sign word `0x0000` or
  `0x8000` and zero significand. Both signs store integer zero under all RC
  modes, preserve PE and source payloads, and clear C1 after the atomic Dword
  write. FIST preserves tag/TOP; FISTP pops only after write success. Tests
  cover sign/RC/opcode/TOP/PE/C1, source and control rejection, and write/range
  atomicity. CLI fixtures observe each opcode with FNSTSW; JIT/native denial
  remain explicit. Native runtime and physical acceptance are deferred under
  ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31bt-exact-signed-zero-fist-m32int-rax-db-10-db-18),
  [plan](plans/STAGE_2.md#s2-31bt-exact-signed-zero-fist-m32int), and
  [report](reports/STAGE_2_REPORT.md#s2-31bt--exact-signed-zero-fist-m32int).

- **S2-31bu evidence:** exact `DF 10`/`DF 18` m16 integer stores admit only
  Zero-tagged canonical binary80 zeros, including both signs. They store Word
  zero under every admitted RC, preserve PE and raw source bytes, and clear C1
  after the atomic Word write. FIST retains tag/TOP; FISTP pops only after
  success. Tests cover sign/RC/opcode/TOP/PE/C1, unsupported tags/payloads,
  FCW/FSW/prefix rejection, and write/range atomicity. A CLI/FNSTSW fixture is
  registered for JIT/native denial. Native runtime and physical acceptance
  remain deferred under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31bu-exact-signed-zero-fist-m16int-rax-df-10-df-18),
  [plan](plans/STAGE_2.md#s2-31bu-exact-signed-zero-fist-m16int), and
  [report](reports/STAGE_2_REPORT.md#s2-31bu--exact-signed-zero-fist-m16int).

- **S2-31bv evidence:** exact `DF 38` FISTP m64 integer stores admit only
  Zero-tagged canonical binary80 zero with sign/exponent word `0x0000` or
  `0x8000` and zero significand. Both signs store Qword zero under all admitted
  RC modes, preserve PE and source bytes, and clear C1 after the atomic write.
  FISTP pops only after write success. ISA tests cover sign/RC/TOP/PE/C1, source
  and control rejection, and write/range atomicity; a CLI/FNSTSW fixture is
  registered for JIT/native denial. Native runtime and physical acceptance
  remain deferred under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31bv-exact-signed-zero-fistp-m64int-rax-df-38),
  [plan](plans/STAGE_2.md#s2-31bv-exact-signed-zero-fistp-m64int), and
  [report](reports/STAGE_2_REPORT.md#s2-31bv--exact-signed-zero-fistp-m64int).

- **S2-31bw evidence:** bare `DB 10`/`DB 18` now round Valid-tagged canonical
  binary80 Normal fractions with unbiased exponent -16382 through -1 to signed
  integer zero or one using the four existing RC modes. All admitted fractions
  add sticky PE; C1 is set iff magnitude one is selected. Comparisons handle
  the half threshold without exponent-sized shifts. ISA coverage crosses sign,
  RC, opcode, TOP, PE/C1, threshold neighbors and extreme exponents; fault/range
  atomicity, CLI/FNSTSW, and JIT/native denial are covered. Native runtime and
  physical acceptance remain deferred under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31bw-round-normal-binary80-fractions-m32-fist),
  [plan](plans/STAGE_2.md#s2-31bw-round-binary80-fractions-through-bounded-m32-fist),
  and [report](reports/STAGE_2_REPORT.md#s2-31bw--round-binary80-fractions-through-m32-fistfstp).

- **S2-31bx evidence:** bare `DF 10`/`DF 18` now round Valid-tagged canonical
  binary80 Normal fractions with unbiased exponent -16382 through -1 to Word
  zero or signed one, using the four existing RC modes and the same
  comparison-only threshold rule as m32. Each accepted fraction adds sticky
  PE; C1 reflects whether magnitude one was selected. ISA coverage crosses
  sign/RC/opcode/TOP/PE/C1, half neighbors and extreme exponents; class/gate
  rejection, write/range atomicity, CLI/FNSTSW and JIT/native denial are covered.
  Native runtime and physical acceptance remain deferred under ISSUE-019. See
  [contract](specs/CPU_CONTRACT.md#local-s2-31bx-round-normal-binary80-fractions-m16-fist),
  [plan](plans/STAGE_2.md#s2-31bx-round-binary80-fractions-through-bounded-m16-fist),
  and [report](reports/STAGE_2_REPORT.md#s2-31bx--round-binary80-fractions-through-m16-fistfstp).

- **S2-31by evidence:** bare `DF 38` now rounds Valid-tagged canonical
  binary80 Normal fractions with unbiased exponent -16382 through -1 to Qword
  zero or signed one. The existing comparison-only rule adds sticky PE for
  every admitted fraction and sets C1 iff magnitude one was selected, with no
  new IE/DE/OE/UE. Coverage includes signs/RC/TOP/PE/C1, threshold neighbors,
  extreme exponents, rejection controls, Qword canaries, write/range atomicity,
  CLI/FNSTSW, and JIT/native denial. Native runtime and physical acceptance
  remain deferred under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31by-round-normal-binary80-fractions-m64-fistp),
  [plan](plans/STAGE_2.md#s2-31by-round-normal-binary80-fractions-through-bounded-m64-fistp),
  and [report](reports/STAGE_2_REPORT.md#s2-31by--round-binary80-fractions-through-m64-fistp).

- **S2-31bz evidence:** bare `DB 10`/`DB 18` now convert Special-tagged
  canonical binary80 ±Infinity to Dword integer indefinite `0x80000000` under
  the admitted masked FCWs. After successful write, IE is sticky, C1 clears,
  incoming PE and C0/C2/C3 are preserved, and no other exception status is
  added. Tests cover signs/FCWs/opcodes/TOP/PE/C1, source/control rejection,
  finite -2^31 distinction, write/range atomicity, CLI/FNSTSW/FNCLEX, and
  JIT/native denial. Native runtime and physical acceptance remain deferred
  under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31bz-masked-canonical-infinity-fist-m32int),
  [plan](plans/STAGE_2.md#s2-31bz-masked-binary80-infinity-through-bounded-m32-fist),
  and [report](reports/STAGE_2_REPORT.md#s2-31bz--masked-binary80-infinity-through-m32-fistfstp).

- **S2-31ca evidence:** bare `DF 10`/`DF 18` now convert Special-tagged
  canonical binary80 ±Infinity to Word integer indefinite `0x8000` under the
  admitted masked FCWs. After successful write, IE is sticky, C1 clears,
  incoming PE and C0/C2/C3 are preserved, and no other exception status is
  added. Tests cover signs/FCWs/opcodes/TOP/PE/C1, malformed/tag/class/control
  rejection, finite -32768 distinction, write/range atomicity, CLI/FNSTSW/FNCLEX,
  and JIT/native denial. Native runtime and physical acceptance remain deferred
  under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31ca-masked-canonical-infinity-fist-m16int),
  [plan](plans/STAGE_2.md#s2-31ca-masked-binary80-infinity-through-bounded-m16-fist),
  and [report](reports/STAGE_2_REPORT.md#s2-31ca--masked-binary80-infinity-through-m16-fistfstp).

- **S2-31cb evidence:** bare `DF 38` now converts Special-tagged canonical
  binary80 ±Infinity to Qword integer indefinite `0x8000000000000000` under
  admitted masked FCWs. After successful write, IE is sticky, C1 clears,
  incoming PE and C0/C2/C3 are preserved, and no other exception status is
  added. Coverage includes signs/FCW/TOP/PE/C1, finite -2^63 distinction,
  source/control/prefix rejection, Qword write/range atomicity, CLI/FNSTSW/FNCLEX,
  and JIT/native denial. Native runtime and physical acceptance remain deferred
  under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31cb-masked-canonical-infinity-fistp-m64int),
  [plan](plans/STAGE_2.md#s2-31cb-masked-binary80-infinity-through-bounded-m64-fistp),
  and [report](reports/STAGE_2_REPORT.md#s2-31cb--masked-binary80-infinity-through-m64-fistp).


- **S2-31cc evidence:** bare `DB 10`/`DB 18` now admit Special-tagged
  canonical binary80 quiet NaNs (J=1, quiet bit=1) to m32 integer indefinite
  `0x80000000`. Successful writes add IE and clear C1 while preserving incoming
  PE/C0/C2/C3 and adding no other exception flags. Tests cover payloads/signs,
  FCW/opcode/TOP/PE/C1, rejection classes, write/range atomicity, CLI/FNSTSW/
  FNCLEX and JIT/native denial. Native runtime and physical acceptance remain
  deferred under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31cc-masked-canonical-qnan-fist-m32int),
  [plan](plans/STAGE_2.md#s2-31cc-masked-binary80-qnan-through-bounded-m32-fist),
  and [report](reports/STAGE_2_REPORT.md#s2-31cc--masked-binary80-qnan-through-m32-fistfstp).


- **S2-31cd evidence:** bare `DF 10`/`DF 18` now admit Special-tagged
  canonical binary80 quiet NaNs (J=1, quiet bit=1) to m16 integer indefinite
  `0x8000`. Successful writes add IE and clear C1 while preserving incoming
  PE/C0/C2/C3 and adding no other exception flags. Tests cover payloads/signs,
  FCW/opcode/TOP/PE/C1, rejection classes, Word write/range atomicity,
  CLI/FNSTSW/FNCLEX and JIT/native denial. Native runtime and physical
  acceptance remain deferred under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31cd-masked-canonical-qnan-fist-m16int),
  [plan](plans/STAGE_2.md#s2-31cd-masked-binary80-qnan-through-bounded-m16-fist),
  and [report](reports/STAGE_2_REPORT.md#s2-31cd--masked-binary80-qnan-through-m16-fistfstp).


- **S2-31ce evidence:** bare `DF 38` now admits Special-tagged canonical
  binary80 quiet NaNs (J=1, quiet bit=1) to m64 integer indefinite
  `0x8000000000000000`. Successful writes add IE and clear C1 while preserving
  incoming PE/C0/C2/C3 and adding no other exception flags. Tests cover
  payloads/signs, FCW/TOP/PE/C1, rejection classes, Qword write/range atomicity,
  CLI/FNSTSW/FNCLEX and JIT/native denial. Native runtime and physical
  acceptance remain deferred under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31ce-masked-canonical-qnan-fistp-m64int),
  [plan](plans/STAGE_2.md#s2-31ce-masked-binary80-qnan-through-bounded-m64-fistp),
  and [report](reports/STAGE_2_REPORT.md#s2-31ce--masked-binary80-qnan-through-m64-fistp).
- **S2-31cf evidence:** bare `DB 10`/`DB 18` now admit canonical Special-tagged
  binary80 SNaNs for m32 FIST/FISTP, storing integer indefinite, adding IE,
  clearing C1 and preserving raw source bytes. The four-payload sign×FCW×opcode×TOP×PE/C1
  matrix has 2,048 cases; pseudo-SNaN/QNaN, tag, control, prefix, width, and
  write/range atomicity controls are included with CLI/FNSTSW and JIT denial.
  Native denial compiles for Linux x86_64 but is architecture-gated and did not
  run on the local arm64 host. Physical x86 and native runtime behavior remain
  unverified; S2-31 and
  ISSUE-019 remain open. See [contract](specs/CPU_CONTRACT.md#local-s2-31cf-masked-canonical-snan-fist-m32int),
  [plan](plans/STAGE_2.md#s2-31cf-masked-binary80-snan-through-bounded-m32-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31cf--masked-binary80-snan-through-m32-fistfstp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31cg evidence:** bare `DF 10`/`DF 18` now admit canonical Special-tagged
  binary80 SNaNs for m16 FIST/FISTP, storing Word integer indefinite, setting
  IE, clearing C1 and preserving raw source bytes. The four-payload
  sign×FCW×opcode×TOP×PE/C1 matrix has 2,048 cases; pseudo-NaN/tag/control/
  prefix and write/range atomicity controls, CLI/FNSTSW, JIT denial, and Linux
  x86_64 native-denial compile coverage are included. The former m64 `DF 38`
  denial was superseded by S2-31ch. Physical x86 and
  native runtime behavior remain unverified; S2-31 and ISSUE-019 remain open.
  See [contract](specs/CPU_CONTRACT.md#local-s2-31cg-masked-canonical-snan-fist-m16int),
  [plan](plans/STAGE_2.md#s2-31cg-masked-binary80-snan-through-bounded-m16-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31cg--masked-binary80-snan-through-m16-fistfstp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ch evidence:** bare `DF 38` admits canonical Special-tagged binary80
  SNaNs for m64 `FISTP`, storing Qword integer indefinite, setting IE, clearing
  C1 and preserving raw source bytes. The 1,024-case payload/sign/FCW/TOP/
  PE/C1 matrix includes Qword write/range atomicity, malformed class/tag and
  control rejection, FNSTSW/FNCLEX, CLI fixture and JIT denial. Native denial
  is architecture-gated and compiled in the Linux x86_64 check; native runtime
  and physical behavior remain unverified. S2-31 and ISSUE-019 remain open.
  See [contract](specs/CPU_CONTRACT.md#local-s2-31ch-masked-canonical-snan-fistp-m64int),
  [plan](plans/STAGE_2.md#s2-31ch-masked-binary80-snan-through-bounded-m64-fistp),
  [report](reports/STAGE_2_REPORT.md#s2-31ch--masked-binary80-snan-through-m64-fistp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ci evidence:** incoming sticky IE is admitted only at the five
  bounded integer-store gates (DB 10, DB 18, DF 10, DF 18, DF 38), alongside
  already accepted PE. DE/ZE/OE/UE/SF/ES/B remain rejected. The 17,920-case
  matrix covers accepted source classes/signs/forms/FCWs/TOP and historical
  C1; all-form IE+PE write/range atomicity and IE-plus-forbidden-status
  controls pass. Masked-invalid FIST→FISTP without FNCLEX, IE-only exact finite
  storage, FNSTSW/FNCLEX, two CLI fixtures and JIT/native denial are covered.
  Native runtime and physical x86 behavior remain unverified; S2-31 and
  ISSUE-019 remain open. See
  [contract](specs/CPU_CONTRACT.md#local-s2-31ci-admit-sticky-ie-bounded-integer-stores),
  [plan](plans/STAGE_2.md#s2-31ci-admit-sticky-ie-on-bounded-integer-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31ci--sticky-ie-on-bounded-integer-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31cj evidence:** canonical finite binary80 Normal values that round
  outside signed i32 on bounded DB10/DB18 now store integer-indefinite and add
  IE after the atomic write. The exact quotient/remainder RC calculation is
  retained for exponents through 31, so values just outside the real range
  that round back into range remain finite; exponent 32 and above classify
  directly invalid. m16/m64, subnormal, malformed and unmasked cases remain
  outside this S2-31cj slice (m16 finite overflow is extended by S2-31ck and
  m64 finite overflow is added by S2-31cl).
  Native runtime and physical x86 checks remain unverified;
  S2-31 and ISSUE-019 remain open. See [contract](specs/CPU_CONTRACT.md#local-s2-31cj-masked-finite-overflow-m32-fist),
  [plan](plans/STAGE_2.md#s2-31cj-masked-finite-overflow-to-m32-fistfstp),
  [report](reports/STAGE_2_REPORT.md#s2-31cj--masked-finite-overflow-to-m32-fistfstp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31ck evidence:** canonical finite binary80 Normal values rounded outside
  signed i16 through bounded DF10/DF18 now store Word integer-indefinite and
  add IE after the atomic write. Values that round back into range remain
  finite; exponent 16 and above classify invalid before wide shifts. The prior
  S2-31d/S2-31e overflow and exponent-16 fixtures now execute successfully,
  and S2-31ck adds
  CLI/FNSTSW examples for invalid overflow and roundback. m32 overflow,
  subnormal, malformed and unmasked behavior remain outside this slice; m64
  finite overflow is added by S2-31cl. Native
  runtime and physical x86 checks remain unverified; S2-31 and ISSUE-019 remain
  open. See [contract](specs/CPU_CONTRACT.md#local-s2-31ck-masked-finite-overflow-m16-fist),
  [plan](plans/STAGE_2.md#s2-31ck-masked-finite-overflow-to-m16-fistfstp),
  [report](reports/STAGE_2_REPORT.md#s2-31ck--masked-finite-overflow-to-m16-fistfstp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31cl evidence:** only bounded `DF 38` now converts canonical
  Valid-tagged binary80 Normal values whose RC-rounded integer is outside
  signed i64, storing Qword integer-indefinite and adding IE after the atomic
  write. Values outside the real bounds that round back into range remain
  finite, including the negative half-integer counterpart; exponent 64+ is
  classified without a wide shift. Subnormal, malformed and unmasked behavior
  remain unsupported. Focused overflow/fault/CLI coverage and final
  workspace/host/Linux check evidence are recorded in the linked report.
  Native execution and physical x86 behavior remain deferred; S2-31 and
  ISSUE-019 remain open. See [contract](specs/CPU_CONTRACT.md#local-s2-31cl-masked-finite-overflow-m64-fistp),
  [plan](plans/STAGE_2.md#s2-31cl-masked-finite-overflow-to-m64-fistp),
  [report](reports/STAGE_2_REPORT.md#s2-31cl--masked-finite-overflow-to-m64-fistp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cm evidence:** bounded integer stores DB10/DB18/DF10/DF18/DF38 now
  admit sticky DE via the shared `0x80dc` status mask while preserving IE/DE/PE
  and rejecting ZE/OE/UE/SF/ES/B. DE alone adds no conversion flags. All five
  forms preserve full state and memory on denied-write/range faults with
  IE|DE|PE set; DE combined with each forbidden status rejects before access.
  FLD subnormal composition and FISTP/FNSTSW fixtures are documented in the
  linked report. The older S2-31d/e invalid-status fixtures now use ZE (0x4504)
  instead of DE (0x4502), which this slice admits. Native runtime and physical
  x86 behavior remain unverified;
  S2-31 and ISSUE-019 remain open. See
  [contract](specs/CPU_CONTRACT.md#local-s2-31cm-admit-sticky-de-bounded-integer-stores),
  [plan](plans/STAGE_2.md#s2-31cm-admit-sticky-de-on-bounded-integer-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31cm--sticky-de-on-bounded-integer-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cn evidence:** the same five bounded integer stores now admit and
  preserve sticky UE alongside IE/DE/PE via the shared `0x80cc` mask. UE alone
  does not alter the integer result or add IE/PE; current C1 is recomputed.
  The 71,680-case sticky matrix, 40-case FST m32 underflow composition, and
  five-form UE-bearing write/range atomicity plus UE|forbidden-status checks
  pass. New m32/m64 CLI fixtures verify UE survives FISTP and FNSTSW. JIT/native
  admission remains closed; native runtime and physical x86 behavior remain
  unverified, and S2-31/ISSUE-019 remain open. See
  [contract](specs/CPU_CONTRACT.md#local-s2-31cn-admit-sticky-ue-bounded-integer-stores),
  [plan](plans/STAGE_2.md#s2-31cn-admit-sticky-ue-on-bounded-integer-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31cn--sticky-ue-on-bounded-integer-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).


- **S2-31co evidence:** bounded DB10/DB18/DF10/DF18/DF38 now admit historical
  OE through their shared `0x80c4` FSW gate alongside IE/DE/UE/PE; ZE/SF/ES/B
  remain rejected. Current conversion behavior, including integer-indefinite
  plus IE on invalid conversion, is independent of prior OE, and current C1 is
  recomputed. The 143,360-case matrix, actual FST m32 overflow composition,
  five-form atomicity/forbidden-status controls, OE-only conversion,
  FNSTSW/FNCLEX, CLI fixtures, and JIT/native denial are documented in the
  linked report. Native runtime and physical x86 acceptance remain deferred
  under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31co-admit-sticky-oe-bounded-integer-stores),
  [plan](plans/STAGE_2.md#s2-31co-admit-sticky-oe-on-bounded-integer-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31co--sticky-oe-on-bounded-integer-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cp evidence:** bounded DB10/DB18/DF10/DF18/DF38 now admit historical
  ZE through the shared `0x80c0` FSW gate with IE/DE/OE/UE/PE; SF/ES/B remain
  rejected. ZE alone has no effect on the current conversion, generated flags,
  or C1. The 286,720-case matrix, ZE-only and current-PE/current-IE combinations,
  consecutive stores without FNCLEX, FNSTSW/FNCLEX, five-form all-six-flag fault
  atomicity, forbidden-status controls, literal fixtures, and JIT/native denial
  are captured in the report. No ZE-producing form or unmasked delivery was
  added. Binary80 subnormal and pseudo-denormal integer operands remain outside
  the bounded profile. Physical x86 acceptance remains deferred to the combined
  final S2-31 substage under ISSUE-019. See
  [contract](specs/CPU_CONTRACT.md#local-s2-31cp-admit-historical-sticky-ze-bounded-integer-stores),
  [plan](plans/STAGE_2.md#s2-31cp-admit-historical-sticky-ze-on-bounded-integer-stores),
  [report](reports/STAGE_2_REPORT.md#s2-31cp--historical-sticky-ze-on-bounded-integer-stores),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cq evidence:** only bare DB10/DB18 m32 integer stores now admit
  Special-tagged canonical binary80 subnormals. The complete 65,536-case
  sign/RC/opcode/TOP/sticky-flag/C1 matrix, FLD m80 composition, invalid-class
  controls, write/range atomicity, FISTP/FNSTSW CLI fixture, FNCLEX composition
  and JIT/native denial are tracked in the linked report. This does not resolve the broader
  S2-31/ISSUE-019 scope: native runtime and physical x86 acceptance remain
  deferred to the combined final substage. See [contract](specs/CPU_CONTRACT.md#local-s2-31cq-canonical-subnormal-m32-fist),
  [plan](plans/STAGE_2.md#s2-31cq-canonical-subnormal-m32-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31cq--canonical-subnormal-m32-fist),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cr evidence:** only bare DF10/DF18 m16 integer stores now admit
  Special-tagged canonical binary80 subnormals. The linked report tracks the
  complete 65,536-case matrix, FLD m80 composition, rejection and atomicity
  controls, Word canaries, literal FISTP/FNSTSW fixture, and JIT/native
  denial. Native runtime and physical x86 acceptance remain deferred to the
  combined final S2-31 substage under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31cr-canonical-subnormal-m16-fist),
  [plan](plans/STAGE_2.md#s2-31cr-canonical-subnormal-m16-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31cr--canonical-subnormal-m16-fist),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cs evidence:** only bare DF38 m64 FISTP now admits
  Special-tagged canonical binary80 subnormals. The 32,768-case
  fraction/sign/RC/TOP/sticky/C1 matrix, FLD m80 and FNSTSW/FNCLEX composition,
  malformed/tag/control rejection, Qword canaries, write/range atomicity,
  TOP wrap and raw source preservation, literal CLI fixture, and JIT/native
  denial are tracked in the report. Physical x86 acceptance and native runtime
  remain deferred to the combined final S2-31 substage under ISSUE-019. See
  [contract](specs/CPU_CONTRACT.md#local-s2-31cs-canonical-subnormal-m64-fistp),
  [plan](plans/STAGE_2.md#s2-31cs-canonical-subnormal-m64-fistp),
  [report](reports/STAGE_2_REPORT.md#s2-31cs--canonical-subnormal-m64-fistp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31ct evidence:** only DB10/DB18 m32 FIST/FISTP admit Special-tagged
  J=0 pseudo-infinity (zero significand, all-one exponent, either sign),
  storing integer-indefinite and adding IE. The 16,384-case matrix checks all
  FCWs, forms, TOP values, six sticky flags and old C1; additional coverage
  checks pseudo-NaNs/tags/widths, raw-source preservation, status observation,
  FNCLEX, canaries, write/range atomicity, CLI and JIT/native denial. Physical
  x86 acceptance and native runtime remain deferred to the combined final
  S2-31 substage under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31ct-pseudo-infinity-m32-fist),
  [plan](plans/STAGE_2.md#s2-31ct-pseudo-infinity-m32-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31ct--special-tagged-pseudo-infinity-through-m32-fist),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cu evidence:** only bare DF10/DF18 m16 FIST/FISTP now admit a
  Special-tagged binary80 pseudo-infinity with raw sign/exponent `0x7fff` or
  `0xffff` and zero significand. They store Word integer-indefinite `0x8000`,
  add IE, clear C1, add no PE/other new flags, preserve sticky/C0/C2/C3, and
  commit after the atomic Word write. The 16,384-case matrix checks FCW/form/
  TOP/sticky/C1, source bytes, stack and status; rejection, Word canary,
  denied-write/range atomicity, FNSTSW/FNCLEX, literal CLI, JIT denial and
  Linux x86_64 cross-target native-denial compile coverage are included.
  Linking the Linux test binary is unavailable with the macOS host linker;
  native runtime and physical x86 acceptance remain deferred under ISSUE-019. See
  [contract](specs/CPU_CONTRACT.md#local-s2-31cu-pseudo-infinity-m16-fist),
  [plan](plans/STAGE_2.md#s2-31cu-pseudo-infinity-m16-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31cu--special-tagged-pseudo-infinity-through-m16-fist),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cv evidence:** only bare DF38 m64 FISTP now admits Special-tagged
  pseudo-infinity with zero significand and raw sign/exponent `0x7fff` or
  `0xffff`. The 8,192-case FCW/TOP/sticky/C1 matrix, Qword indefinite result,
  IE-without-new-PE status, raw source retention, TOP wrap, atomic write/range
  rejection, malformed-class/tag/control checks, seeded FNSTSW/FNCLEX CLI
  fixture, and JIT/native denial are recorded in the linked report. FLD
  composition is excluded. Native runtime and physical x86 acceptance remain
  deferred to the combined final S2-31 substage under ISSUE-019; Mac mini
  validation remains pending. See [contract](specs/CPU_CONTRACT.md#local-s2-31cv-pseudo-infinity-m64-fistp),
  [plan](plans/STAGE_2.md#s2-31cv-pseudo-infinity-m64-fistp),
  [report](reports/STAGE_2_REPORT.md#s2-31cv--special-tagged-pseudo-infinity-through-m64-fistp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cw evidence:** DB10/DB18 m32 FIST/FISTP admit only Special-tagged
  J=0 binary80 pseudo-NaNs (all-one exponent, nonzero fraction), including
  Q-bit-clear and Q-bit-set payloads of either sign. A 65,536-case
  independently expected matrix checks integer-indefinite, IE without PE,
  C1 clearing, sticky/condition status preservation, source bytes, Dword
  canaries, single-write/no-read and TOP/tag rules. Wrong tags, pseudo-infinity,
  canonical J=1 QNaN/SNaN, non-all-one exponent, m16/m64, FCW/FSW controls and
  write/range atomicity are covered. The direct CLI fixture observes IE with
  FNSTSW; an ISA FNSTSW/FNCLEX sequence clears it. JIT/native denial is also
  covered; FLD, unmasked behavior and physical x86 acceptance remain outside
  this bounded slice. The latter stays deferred to
  the combined final S2-31 substage under ISSUE-019. See [S2-31cw CPU
  contract](specs/CPU_CONTRACT.md#local-s2-31cw-special-pseudo-nan-m32-fist),
  [plan](plans/STAGE_2.md#s2-31cw-special-pseudo-nan-m32-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31cw--special-tagged-pseudo-nans-through-m32-fist),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).


- **S2-31cx evidence:** only bare DF10/DF18 m16 FIST/FISTP admit
  Special-tagged J=0 binary80 pseudo-NaNs with nonzero fraction, including
  both bit-62 variants and both signs. The 65,536-case independent matrix
  checks Word integer-indefinite, IE without PE, C1 clearing, sticky/condition
  preservation, source bytes, canaries, one write/no read and TOP/tag rules.
  Wrong tags/classes, other exponents, m64, forbidden controls/status and
  denied-write/canonical-range atomicity are covered; FNSTSW/FNCLEX, literal
  CLI, and JIT/native denial checks are included. The m32 behavior remains
  unchanged. FLD and unmasked delivery remain outside this bounded slice.
  Physical x86 acceptance and Mac mini validation remain deferred to the
  combined final S2-31 substage under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-31cx-special-pseudo-nan-m16-fist),
  [plan](plans/STAGE_2.md#s2-31cx-special-pseudo-nan-m16-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31cx--special-tagged-pseudo-nans-through-m16-fist),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cy evidence:** only bare DF38 m64 FISTP admits a Special-tagged J=0
  pseudo-NaN with exponent `0x7fff` and nonzero fraction, including either
  bit-62 variant and both signs. The 32,768-case independent matrix checks
  Qword integer-indefinite, IE/no new PE, C1 and sticky/condition preservation,
  raw ten-byte payload, Qword canaries, one write/no read and TOP wrap. Neighbor
  classes/tags, controls, write/range atomicity, FNSTSW/FNCLEX, a seeded CLI
  fixture, and JIT/native denial are covered. m16/m32 regression behavior is
  unchanged; FLD/unmasked behavior is excluded. Mac mini and physical x86
  acceptance remain deferred to the combined final S2-31 substage under this
  issue. See [contract](specs/CPU_CONTRACT.md#local-s2-31cy-special-pseudo-nan-m64-fistp),
  [plan](plans/STAGE_2.md#s2-31cy-special-pseudo-nan-m64-fistp),
  [report](reports/STAGE_2_REPORT.md#s2-31cy--special-tagged-pseudo-nan-through-m64-fistp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31cz evidence:** only bare DB10/DB18 m32 FIST/FISTP admit a
  Special-tagged unnormal with biased exponent 1..=`0x7ffe`, J=0 and arbitrary
  fraction. The independent 196,608-case exponent/fraction/sign/FCW/form/TOP/
  sticky/C1 matrix checks Dword indefinite, IE without PE/DE, status and raw
  payload preservation, canaries, one write/no read and TOP/tag behavior.
  Boundary/source/control rejects, write/range atomicity, FNSTSW/FNCLEX, a
  seeded CLI fixture, and JIT/native denial are covered. Physical x86 and Mac
  mini acceptance remain deferred to the combined S2-31 substage; ISSUE-019
  remains open. See [contract](specs/CPU_CONTRACT.md#local-s2-31cz-special-unnormals-m32-fist),
  [plan](plans/STAGE_2.md#s2-31cz-special-unnormals-m32-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31cz--special-tagged-unnormals-through-m32-fist),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31da evidence:** bare DF10/DF18 m16 FIST/FISTP now admit only
  Special-tagged finite-exponent binary80 unnormals (J=0); the 196,608-case
  literal matrix checks Word indefinite, IE without PE/DE, flags/status, raw
  payload, canaries and stack behavior. Boundary/tag/width/control rejects,
  write/range atomicity, FNSTSW/FNCLEX, CLI fixture and JIT/native denial are
  covered. Mac mini and physical x86 acceptance remain deferred; ISSUE-019
  stays open. See [contract](specs/CPU_CONTRACT.md#local-s2-31da-special-unnormals-m16-fist),
  [plan](plans/STAGE_2.md#s2-31da-special-unnormals-m16-fist),
  [report](reports/STAGE_2_REPORT.md#s2-31da--special-tagged-unnormals-through-m16-fist),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-31db evidence:** bare DF38 admits only Special-tagged finite-exponent
  binary80 unnormals (J=0) for m64 FISTP; the 98,304-case matrix checks Qword
  indefinite, IE without PE/DE, status/payload preservation, canaries, one
  write/no read, and pop behavior. Boundary/tag/control rejects, status
  visibility, CLI fixture, and JIT/native denial are covered. Mac mini and
  physical x86 acceptance remain deferred; ISSUE-019 stays open. See [contract](specs/CPU_CONTRACT.md#local-s2-31db-special-unnormals-m64-fistp),
  [plan](plans/STAGE_2.md#s2-31db-special-unnormals-m64-fistp),
  [report](reports/STAGE_2_REPORT.md#s2-31db--special-tagged-unnormals-through-m64-fistp),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md).
- **S2-31do evidence:** bare Long64 `D9 FC` admits canonical Special-tagged
  nonzero binary80 subnormals and adds current DE+PE with signed-zero/±1
  rounding and Zero/Valid result tags. The independent state matrix, atomic
  neighbors, repeated FRNDINT, FNSTSW/FNCLEX and seeded
  `project_fixture` composition are local evidence only. JIT/native, physical
  x86 and Mac mini acceptance remain deferred to the combined final S2-31
  substage; ISSUE-019 remains open. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-31do-canonical-binary80-subnormal-frndint-d9-fc),
  [plan](plans/STAGE_2.md#s2-31do-canonical-binary80-subnormal-through-bounded-frndint),
  [report](reports/STAGE_2_REPORT.md#s2-31do--canonical-binary80-subnormal-through-bounded-frndint),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-31do-frndint-canonical-subnormal-fixture).
- **S2-31dp evidence:** bare Long64 `D9 FC` additionally admits Special-tagged
  binary80 pseudo-denormals (E=0/J=1/any fraction) through the bounded FRNDINT
  path. Local equivalence matrices, neighbor rejects, FLD→FRNDINT→FNSTSW
  project fixture, re-entry and status-clearing coverage are recorded in the
  linked report. Native/JIT and physical x86/Mac mini acceptance remain
  deferred; ISSUE-019 remains open. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-31dp-special-tagged-binary80-pseudo-denormal-frndint-d9-fc),
  [plan](plans/STAGE_2.md#s2-31dp-special-tagged-binary80-pseudo-denormal-through-bounded-frndint),
  [report](reports/STAGE_2_REPORT.md#s2-31dp--special-tagged-binary80-pseudo-denormal-through-bounded-frndint),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-31dp-frndint-pseudo-denormal-fixture).
- **S2-31dt evidence:** exact bare D900/DD00 now admit and preserve historical UE through their local `0x80cc` gates; ZE/OE/SF/ES/B remain pre-read rejects, and unrelated FILD/m80/store/FRNDINT/native/JIT predicates are unchanged. The interpreter matrix crosses both widths, eight source classes, four FCWs, all TOPs, prior C1 and all IE/DE/UE/PE combinations with complete state and memory checks. UE-only normal-load `FNSTSW` project fixtures, current SNaN IE/quieting and subnormal DE, same/cross-width underflow-store→load re-entry, repeated loads and FNCLEX are covered. Bad controls, occupied destinations, forbidden UE combinations, backend/range/read/boundary atomicity remain checked. Physical x86 and native runtime behavior were not performed; ISSUE-019 remains open. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-31dt-historical-sticky-ue-for-bounded-m32m64-fld), [plan](plans/STAGE_2.md#s2-31dt-admit-historical-sticky-ue-through-exact-binary32binary64-fld), [report](reports/STAGE_2_REPORT.md#s2-31dt--historical-sticky-ue-for-bounded-m32m64-fld), and [fixture notes](../tests/fixtures/x86/stage2/README.md).

- **S2-30as bounded FSTP ST(i) signed-zero evidence:** exact Zero-tagged ±0 is copied with its tag to `(old_TOP+i)&7`, the old physical TOP is then marked Empty, and TOP advances once. The `i=0` payload remains with final Empty tag; `i=1` becomes new ST(0). Normal FSTP behavior remains covered. Local matrices and both-width compositions plus exact fixture `d900ddd9dfe0` do not establish native/JIT or physical behavior; ISSUE-019 remains open. See [contract](specs/CPU_CONTRACT.md#local-s2-30as-signed-zero-through-fstp-sti-dd-d8i), [plan](plans/STAGE_2.md#s2-30as-admit-canonical-signed-zero-to-bare-fstp-sti-dd-d8i), [report](reports/STAGE_2_REPORT.md#s2-30as--canonical-signed-zero-through-fstp-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30as-signed-zero-fstp-sti).
- **S2-30bc historical IE evidence:** the shared `FCHS/FABS` source gate admits IE only by changing its FSW mask to `0x80fe`; other exception/summary bits remain rejected, including IE+SF/DE. The 6,144-case state matrix, same-width m32/m64 SNaN compositions, schema-v2 fixtures, and status observation/clearing checks are local evidence only. At that substage, DE and IE+DE were still rejected; S2-30bd supersedes that status restriction. Native/JIT and physical x86 behavior remain unverified; ISSUE-019 remains open for combined acceptance. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bc-historical-ie-fchs-fabs), [plan](plans/STAGE_2.md#s2-30bc-admit-historical-sticky-ie-to-bounded-fchsfabs), [report](reports/STAGE_2_REPORT.md#s2-30bc--historical-sticky-ie-through-bounded-fchs-fabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bc-historical-ie-fchs-fabs).
- **S2-30bd historical IE/DE evidence:** the shared `FCHS/FABS` gate now admits sticky DE alongside IE via FSW mask `0x80fc`; all other forbidden status bits remain closed. The 12,288-state matrix, binary32/64 subnormal load/sign/store compositions, and project fixtures are local evidence only. Native/JIT and physical behavior remain unverified; ISSUE-019 stays open. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bd-historical-de-fchs-fabs), [plan](plans/STAGE_2.md#s2-30bd-admit-historical-sticky-de-to-bounded-fchsfabs), [report](reports/STAGE_2_REPORT.md#s2-30bd--historical-sticky-de-through-bounded-fchsfabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bd-historical-de-fchs-fabs).

- **S2-30be historical IE/DE/PE evidence:** the bounded `FCHS/FABS` source gate now uses FSW mask `0x80dc`, admitting exactly IE/DE/PE combinations and continuing to reject ZE/OE/UE/SF/ES/B combinations. The 24,576-state matrix, FRNDINT PE producer checks, signed-zero subnormal→FRNDINT→FCHS→FABS checks of complete raw/tag/FSW state, six schema-v2 CLI fixtures, and JIT denial are local evidence. Native/runtime and physical x86/Mac mini acceptance remain deferred; ISSUE-019 stays open. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30be-historical-pe-fchs-fabs), [plan](plans/STAGE_2.md#s2-30be-admit-historical-sticky-pe-to-bounded-fchsfabs), [report](reports/STAGE_2_REPORT.md#s2-30be--historical-sticky-pe-through-bounded-fchsfabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30be-historical-pe-fchs-fabs).

- **S2-30bf historical IE/DE/UE/PE evidence:** the bounded sign-operation source gate now uses mask `0x80cc`, admitting all 16 sticky IE/DE/UE/PE combinations and rejecting ZE/OE/SF/ES/B across each. Local 49,152-case matrix, masked-underflow FST producers and CLI status fixtures do not establish native/JIT execution or physical x86 behavior. ISSUE-019 remains open; next step is combined final-substage acceptance. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bf-historical-ue-fchs-fabs), [plan](plans/STAGE_2.md#s2-30bf-admit-historical-sticky-ue-to-bounded-fchsfabs), [report](reports/STAGE_2_REPORT.md#s2-30bf--historical-sticky-ue-through-bounded-fchs-fabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bf-historical-ue-fchs-fabs).
- **S2-30bg historical IE/DE/OE/UE/PE evidence:** the bounded sign-operation source gate now uses mask `0x80c4`, admitting all 32 sticky histories while preserving ZE/SF/ES/B atomic rejects. The 98,304-state full matrix, 3,072 forbidden-status cases, real m32/m64 FST overflow-midpoint producers and neighbors, seeded OE CLI case, ZE rejection fixtures, and JIT denial are local evidence only. No Mac mini, SSH, native runtime or physical x86 checks were performed. ISSUE-019 remains open for combined host/physical acceptance. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bg-historical-oe-through-bounded-fchsfabs), [plan](plans/STAGE_2.md#s2-30bg-admit-historical-sticky-oe-to-bounded-fchsfabs), [report](reports/STAGE_2_REPORT.md#s2-30bg--historical-sticky-oe-through-bounded-fchsfabs), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bg-historical-oe-through-bounded-fchsfabs).

- **S2-30bh historical IE/DE/ZE/OE/UE/PE evidence:** the bounded `FCHS/FABS`
  gate uses mask `0x80c0`, admitting all 64 sticky histories while SF/ES/B
  remain atomic rejects. The 196,608 full-state matrix, 4,608 forbidden-status
  product, seeded ZE/all-flags CLI fixtures, FRNDINT compositions and JIT denial
  are local evidence only. ZE is explicitly seeded; no divide-by-zero producer
  is claimed. No Mac mini, SSH, native runtime or physical x86 checks were run.
  ISSUE-019 remains open for combined final acceptance. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bh-historical-ze-through-bounded-fchsfabs),
  [plan](plans/STAGE_2.md#s2-30bh-admit-historical-sticky-ze-to-bounded-fchsfabs),
  [report](reports/STAGE_2_REPORT.md#s2-30bh--historical-sticky-ze-through-bounded-fchsfabs),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bh-historical-ze-through-bounded-fchsfabs).
- **S2-30bi historical SF|IE evidence:** the bounded FCHS/FABS gate accepts
  SF only when IE is also set; standalone SF and SF|DE without IE still reject.
  The 294,912-state full-state matrix, 768 no-IE SF rejects, 6,144 ES/B cross
  product, seeded basic and FRNDINT-composition CLI fixtures, and execution
  boundaries are local evidence only. IE|SF is seeded in the initial project
  snapshot; there is no guest SF producer in scope, and FRNDINT only composes
  with the seeded history. Empty, ES, B, #MF/WAIT delivery and native/physical
  behavior remain outside this slice. Further ES/B relaxation requires a
  separate pending-exception contract plus final physical x86 acceptance.
  ISSUE-019 remains open. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bi-historical-sf-ie-fchs-fabs),
  [plan](plans/STAGE_2.md#s2-30bi-admit-historical-sf-only-with-ie-to-bounded-fchsfabs),
  [report](reports/STAGE_2_REPORT.md#s2-30bi--historical-sf-only-with-ie-through-bounded-fchsfabs),
  and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bi-historical-sfie-through-bounded-fchsfabs).
- **S2-30bm bounded TOP adjustment evidence:** bare `D9 F7`/`D9 F6` now adjust only the TOP field, clear C1 and preserve all eight raw cells plus the full tag word without inspecting them. The 1,536-case opaque-state matrix, wrap/repeat/inverse checks, all nine status-gate rejects, schema allowlist, and seeded status-observation fixtures are local evidence. Preserving undefined C0/C2/C3 is a local deterministic policy. ES/B, #MF/#NM/WAIT/pending-exception/IE/SF behavior, JIT/native execution, and physical x86 acceptance remain outside this slice; ISSUE-019 stays open. See [contract](specs/CPU_CONTRACT.md#local-s2-30bm-bounded-register-only-fincstp--fdecstp), [plan](plans/STAGE_2.md#s2-30bm-bounded-register-only-fincstp--fdecstp), [report](reports/STAGE_2_REPORT.md#s2-30bm--bounded-register-only-fincstp-and-fdecstp), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bm-bounded-fincstpfdecstp).
- **S2-30bt bounded FXAM evidence:** exact bare `D9 E5` classifies only Valid-tagged canonical Normal and Zero-tagged exact signed zero under the strict FCW/status gate. The 5,120 success states cover four canonical Normal representatives (minimum normal, 1, 1+ULP, maximum finite) and exact Zero; the 10,752 class/tag rejects cover each nonadmitted class/tag pair under both signs, TOP values and prior CC patterns. Seven exact schema streams, literal producer/repeat/sign fixtures and five atomic rejects are local interpreter evidence; JIT/native remain denied and physical behavior remains unverified. FTST's gate is unchanged. ISSUE-019 remains open for broader x87 semantics and final host/physical acceptance. See [contract](specs/CPU_CONTRACT.md#local-s2-30bt-bounded-bare-fxam-d9-e5), [plan](plans/STAGE_2.md#s2-30bt-bounded-bare-fxam-normal-and-signed-zero-classification), [report](reports/STAGE_2_REPORT.md#s2-30bt--bounded-bare-fxam), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bt-bounded-fxam).
- **S2-30bz indexed FCMOVE evidence:** exact bare `DA C8..CF` uses the low ModR/M bits as the physical source index. For nonzero indices it validates old TOP and `(TOP+i)&7` before integer RFLAGS.ZF; self-index validates only TOP. The 363,520 success and 161,280 invalid class/tag snapshots, unchanged FCW/FSW gates, exact 19-stream schema, indexed fixtures and complete runner x87 snapshots are local-only evidence; FSW.C3 is not the condition. JIT/native execution, exception delivery and physical acceptance remain unresolved under ISSUE-019. The prior ST(1)-only scope is historical and superseded. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30bz-bounded-indexed-bare-fcmove-st0-sti-da-c8cf), [plan](plans/STAGE_2.md#s2-30bz-bare-fcmove-st0-sti-da-c8cf), [report](reports/STAGE_2_REPORT.md#s2-30bz--bounded-indexed-fcmove-st0-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30bz-bounded-indexed-fcmove-st0-sti).

- **S2-30cf indexed FCMOVNBE acceptance remains local:** bare `DB D0..D7` now uses the strict shared x87 environment and participant gates; local matrices, exact schema, producer/repeat fixtures, complete runner snapshots and JIT denial are recorded in the S2-30cf report. Native runtime, x87 exception delivery and physical x86/Mac mini acceptance have not been performed. Status: open under ISSUE-019; next step is combined acceptance. See [contract](specs/CPU_CONTRACT.md#local-s2-30cf-bounded-indexed-fcmovnbe-st0-sti-db-d0d7), [plan](plans/STAGE_2.md#s2-30cf-indexed-bare-fcmovnbe-st0-sti-db-d0d7), [report](reports/STAGE_2_REPORT.md#s2-30cf--indexed-bare-fcmovnbe-st0-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30cf-indexed-bare-fcmovnbe-st0-sti-db-d0d7).
- **S2-30ce historical FCMOVNBE extent:** exact unprefixed Long64 `DB D1` had a local ST(1)-only interpreter slice with strict x87 environment/operand preflight and raw copy only when integer CF and ZF were both clear. That bounded scope is superseded by indexed S2-30cf; its 51,200 success and 86,016 invalid participant matrices and original six-stream/producer coverage remain historical evidence. Exception delivery, native/JIT execution and physical x86 acceptance remain unresolved under ISSUE-019. See [contract](specs/CPU_CONTRACT.md#local-s2-30ce-bounded-bare-fcmovnbe-st0-st1-db-d1), [plan](plans/STAGE_2.md#s2-30ce-exact-bare-fcmovnbe-st0-st1-db-d1), [report](reports/STAGE_2_REPORT.md#s2-30ce--bare-fcmovnbe-st0-st1), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30ce-bare-fcmovnbe-st0-st1-db-d1).
- **S2-30cg FCMOVU historical extent:** its original exact `DA D9` ST(1)-only local slice covered 512 success and 768 invalid class/tag snapshots. Those counts describe only the earlier one-index matrix; S2-30ch supersedes it for indexed execution. Its two TEST producer fixtures remain compatibility evidence. No native/JIT or physical x86 behavior is established.
- **S2-30ch indexed FCMOVU evidence:** exact unprefixed Long64 `DA D8..DF` carries the source index, preflights TOP and `(TOP+i)&7` before PF for `i>0`, and checks TOP alone for self-index. PF=1 raw-copies the exact ten bytes/tag; PF=0 retains the destination. The bounded profile remains FCW `0x037f`, `FSW & 0x82ff == 0`, Valid canonical Normal or Zero exact signed zero; C1 and Special/NaN semantics remain closed. Local coverage includes 363,520 successes, 161,280 invalid class/tag snapshots, per-index environment/boundary checks, 18 exact schema streams, nine new bare success fixtures, repeat, two TEST producer fixtures, rejection fixtures and JIT denial. Native/JIT execution, exception delivery and physical acceptance remain unverified; ISSUE-019 stays open. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30ch-bounded-indexed-fcmovu-st0-sti-da-d8df), [plan](plans/STAGE_2.md#s2-30ch-indexed-bare-fcmovu-st0-sti-da-d8df), [report](reports/STAGE_2_REPORT.md#s2-30ch--indexed-bare-fcmovu-st0-sti), and [fixture notes](../tests/fixtures/x86/stage2/README.md#s2-30ch-indexed-bare-fcmovu-st0-sti-da-d8df).

**S2-30ck local FSQRT slice:** exact unprefixed Long64 `D9 FA` now executes in
the interpreter only for Valid-tagged canonical positive nonzero Normal
binary80 at physical TOP, under the twelve all-masked PC24/53/64 FCW values and
`FSW & 0x80df == 0`. The full 1,536-state success matrix, literal direct-rounding
vectors and atomic class/environment/boundary rejects are local checks. This
assumes x87 available, CR0.EM=CR0.TS=0 and no pending exception. The existing
selector-bearing x87 gate remains unchanged; FDP is preserved locally, and
CPUID.07H.00H:EBX[6] concerns FDP behavior only. General x87 exceptions,
pending delivery, native hardware and physical acceptance remain unresolved.
See [S2-30ck contract](specs/CPU_CONTRACT.md#local-s2-30ck-bounded-register-only-fsqrt-st0-d9-fa),
[plan](plans/STAGE_2.md#s2-30ck-bare-register-only-fsqrt-st0-d9-fa), and
[report](reports/STAGE_2_REPORT.md#s2-30ck--bounded-bare-fsqrt-st0).

**S2-30cl local FPREM slice:** exact unprefixed Long64 `D9 F8` is admitted only
for positive canonical Normal operands, all-masked FCW, clean required FSW
fields, exponent gap below 64 and a divisor exponent cutoff that proves results
normal or zero. The local full-reduction path updates the exact quotient CCs;
partial reduction, underflow, general exception delivery, JIT/native execution
and physical acceptance remain outside scope. The 3,072-state matrix, literal
quotient/remainder vectors, atomic denials and JIT denial pass. Native denial
was added but could not be executed under ISSUE-029. See [S2-30cl contract](specs/CPU_CONTRACT.md#local-s2-30cl-bounded-register-only-fprem-st0-st1-d9-f8),
[plan](plans/STAGE_2.md#s2-30cl-bare-register-only-fprem-st0-st1-d9-f8), and
[report](reports/STAGE_2_REPORT.md#s2-30cl--bounded-bare-fprem-st0-st1).

**S2-30cm local FPREM1 slice:** exact unprefixed Long64 `D9 F5` executes only
for Valid canonical positive Normal operands, all-masked FCW, clean required
FSW fields, complete-reduction gap `D<64`, and the proven local divisor cutoff
`e1>=-16318`. The signed nearest-even remainder helper is used for all admitted
gaps including `D=-1`; partial reduction and subnormal results are excluded.
Full-state matrices, literal quotient/remainder cases, atomic denials and JIT
denial pass. Linux-only native denial coverage is present but was not run on
this macOS host; ISSUE-029 tracks that unverified backend route. Physical x86
acceptance and general x87 exception delivery remain open. See [S2-30cm
contract](specs/CPU_CONTRACT.md#local-s2-30cm-bounded-register-only-fprem1-st0-st1-d9-f5),
[plan](plans/STAGE_2.md#s2-30cm-bare-register-only-fprem1-st0-st1-d9-f5), and
[report](reports/STAGE_2_REPORT.md#s2-30cm--bounded-bare-fprem1-st0-st1).

**S2-30cn local FDIV slice:** exact bare `D8 F0..F7` executes for Valid
canonical nonzero Normal operands, all-masked PC24/53/64 FCW, and clean required
FSW fields. It uses the direct finite division precision helper; only finite
Normal results are admitted. PE accumulates mathematical inexactness and C1
uses the helper's pre-carry increment diagnostic; C0/C2/C3 are preserved by
local policy. DC/DE directions and pop forms retain their prior exact-only
scope; bounded reverse `FDIVR ST(0), ST(i)` is the separate S2-30cs slice.
Literal tests, full-state denials, `od-isa`, `od-testkit`,
runner, JIT-denial, workspace and clean Sol/high review results are recorded in
[S2-30cn report](reports/S2_30CN_X87_FDIV_PRECISION.md). Native/JIT execution,
guest exception delivery and physical x86 acceptance remain outside scope;
ISSUE-019 remains open. See [contract](specs/CPU_CONTRACT.md#local-s2-30cn-bounded-fdiv-st0-sti-d8-f0-f7),
[plan](plans/STAGE_2.md#s2-30cn-bare-fdiv-st0-sti-with-bounded-fcw-precision), and
[shared report](reports/STAGE_2_REPORT.md#s2-30cn--bounded-fdiv-st0-sti).

**S2-30cs local FDIVR slice:** exact bare Long64 `D8 F8..FF` directly rounds
`ST(i) / ST(0)` through the existing finite division precision helper, with
all-masked PC24/53/64 FCWs and the same strict x87 state, operand and normal
result boundary as S2-30cn. It writes old physical TOP without popping, ORs PE,
replaces C1 from magnitude `incremented`, preserves C0/C2/C3 and other state,
and retires once. The p64 1/3 rejection is superseded by a positive rounded
case; actual zero/special/subnormal/malformed, unsupported environment and
out-of-domain cases remain atomic refusals. Full TOP/index/sign/FCW, midpoint,
carry, order, sticky status, fetch/STOP/budget and canary coverage is recorded
in [S2-30cs report](reports/S2_30CS_X87_FDIVR_PRECISION.md). JIT/native,
guest exception delivery and physical x86 acceptance remain outside scope;
ISSUE-019 remains open. See [contract](specs/CPU_CONTRACT.md#local-s2-30cs-bounded-fdivr-st0-sti-d8-f8ff),
[plan](plans/STAGE_2.md#s2-30cs-bare-fdivr-st0-sti-with-bounded-fcw-precision),
and [shared report](reports/STAGE_2_REPORT.md#s2-30cs--bounded-fdivr-st0-sti).

**S2-30co local FMUL slice:** exact bare `D8 C8..CF` now uses target-precision
binary80 multiplication only for Valid canonical nonzero Normal operands and
the twelve all-masked FCW settings. The self-alias squares and can be inexact;
PE accumulates `inexact` and C1 uses pre-carry magnitude `incremented`. Other
FMUL directions and FMULP remain exact-only. Focused/full interpreter tests,
runner regressions, workspace checks, and independent Sol/high review pass.
No native/JIT or physical x86 support is claimed. See [contract](specs/CPU_CONTRACT.md#local-s2-30co-bounded-fmul-st0-sti-d8-c8-cf),
[plan](plans/STAGE_2.md#s2-30co-bare-fmul-st0-sti-with-bounded-fcw-precision),
and [report](reports/STAGE_2_REPORT.md#s2-30co--bounded-fmul-st0-sti).

**S2-30cp local FADD slice:** bare `D8 C0..C7` now uses direct target
precision for Valid canonical nonzero Normal operands under the twelve
all-masked FCWs, with a normalized exponent-gap limit of 64 enforced before
the helper call. PE accumulates `inexact`, C1 uses pre-carry magnitude
`incremented`, and the alias `i=0` doubles ST(0). Reverse, pop and subtraction
forms remain exact-only. Focused/full interpreter tests, runner regressions,
workspace checks, formatting and independent Sol/high review pass. JIT/native
and physical x86 acceptance are not claimed. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30cp-bounded-fadd-st0-sti-d8-c0-c7),
[plan](plans/STAGE_2.md#s2-30cp-bare-fadd-st0-sti-with-bounded-fcw-precision),
and [standalone report](reports/S2_30CP_X87_FADD_PRECISION.md). S2-30 and
ISSUE-019 remain open.

**S2-30cq local FSUB slice:** bare `D8 E0..E7` now uses the same bounded
target precision, FCWs, operand gates, gap≤64 limit and PE/C1 commit policy.
It flips only a validated local copy of the RHS sign bit and reuses the
existing finite-addition precision helper; `i=0` cancellation and all other
zero/subnormal/non-Normal results reject atomically. Focused direct tests cover
all TOP/index pairs, signs and FCWs, p53 midpoint neighbors and directed
rounding, gaps 1/63/64 and 65+, sticky PE/C1, malformed/range refusals,
fetch/STOP/budget and no-data-access behavior. Focused, full od-isa,
testkit/runner, workspace, formatting and diff checks passed. Graphify AST
refresh passed. Layout validation reproduces the existing `.git` worktree
pointer and missing historical oracle manifest findings in ISSUE-023;
independent Sol/high review is clean after correcting the refusal-test baseline
and duplicate admission check. JIT/native and physical x86 behavior are not
claimed. See [CPU
contract](specs/CPU_CONTRACT.md#local-s2-30cq-bounded-fsub-st0-sti-d8-e0-e7),
[plan](plans/STAGE_2.md#s2-30cq-bare-fsub-st0-sti-with-bounded-fcw-precision),
and [standalone report](reports/S2_30CQ_X87_FSUB_PRECISION.md). ISSUE-019
remains open.

**S2-30cr local FSUBR slice:** bare `D8 E8..EF` now uses the bounded target
precision, twelve all-masked FCWs, canonical Normal operand gates, normalized
gap≤64 limit and PE/C1 commit policy. It computes `ST(i) - ST(0)` by flipping
only a validated local copy of ST(0) and reusing the existing finite-addition
precision helper. `i=0` cancellation and zero/subnormal/overflow/non-Normal
results reject atomically. Direct checks cover all TOP/index pairs, sign
combinations and FCWs, p53 midpoint neighbors, directed rounding for both
result signs, gaps 1/63/64 and 65+, selectors/tags/status/malformed/range
denials, source/canaries, fetch/STOP/budget and no-data-access behavior. Focused
and full od-isa, testkit/runner, workspace, formatting, layout, diff and
Graphify checks are recorded in [S2-30cr report](reports/S2_30CR_X87_FSUBR_PRECISION.md).
Independent Sol/high review completed clean. JIT/native and physical x86
acceptance remain open. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-30cr-bounded-fsubr-st0-sti-d8-e8-ef),
[plan](plans/STAGE_2.md#s2-30cr-bare-fsubr-st0-sti-with-bounded-fcw-precision),
and [shared report](reports/STAGE_2_REPORT.md#s2-30cr--bounded-fsubr-st0-sti).

### ISSUE-020 — Не подтверждён победитель dual-fault для CMPS RSI/RDI

- **Дата обнаружения:** 2026-10-26.
- **Компонент:** `od-isa`, локальные Long64 CMPS B/W/D/Q S2-18i/S2-18n, REPE CMPSB/W/D/Q S2-19f/S2-19h, REPE CMPSB address-size-32 S2-19n, REPE CMPSW/D/Q address-size-32 S2-19p, REPNE CMPSB address-size-32 S2-19r и REPNE CMPSW/D/Q address-size-32 S2-19t.
- **Статус:** открыта; это не подтверждённый дефект.
- **Суть и последствия:** bounded CMPS profiles читают width-sized flat DS:[RSI]/DS:[ESI] до flat ES:[RDI]/ES:[EDI]. REP CMPS B/W/D/Q коммитит compare/index/count только после обоих чтений; если второе чтение отказывает, первый read остаётся в trace. Address-size-32 S2-18n separately records four dual-unmapped fixtures (`s2_18n_dual_unmapped_b.case.json` and `_w`, `_d`, `_q`) and expects the local ESI-first error; S2-19n, S2-19p, REPNE S2-19r and S2-19t preserve source-first behavior for dual-invalid addresses. At two simultaneously invalid addresses this policy does not establish hardware exception priority. The implementation makes no MMIO/backend side-effect rollback claim.
- **Основание:** S2-18i, S2-18n, S2-19f, S2-19h, S2-19n, S2-19p, S2-19r and S2-19t specify RSI/ESI-first local policy. Unit tests/fault matrices check first/second failures, replay and the local dual-invalid result, but do not compare it with hardware. See [CPU contract](specs/CPU_CONTRACT.md#local-s2-19r-bounded-repne-cmpsb-address-size-32-contract), [S2-18n contract](specs/CPU_CONTRACT.md#local-s2-18n-non-rep-address-size-32-cmps-contract), [S2-19h plan](plans/STAGE_2.md#s2-19h--bounded-f3-a7-repe-cmpswdq), [S2-19n plan](plans/STAGE_2.md#s2-19n--bounded-repe-cmpsb-с-address-size-32), [S2-19p plan](plans/STAGE_2.md#s2-19p--bounded-repe-cmpswdq-с-address-size-32), [S2-19r plan](plans/STAGE_2.md#s2-19r--bounded-repne-cmpsb-с-address-size-32), and [S2-19t plan](plans/STAGE_2.md#s2-19t--bounded-repne-cmpswdq-addr32).
- **Следующий шаг:** если потребуется аппаратный dual-fault claim, сначала найти нормативное evidence либо сформировать изолированный dual-invalid case, затем проверить его на physical x86 в общем финальном acceptance substage. До такого результата сохранять локальную политику и не выдавать её за hardware priority.

### ISSUE-021 — Не подтверждён приоритет dual-invalid для MOVS RSI/RDI

- **Дата обнаружения:** 2026-10-26.
- **Компонент:** `od-isa`, локальный Long64 REP MOVSB/W/D/Q профиль S2-19d/S2-19l и non-REP address-size-32 MOVS S2-18o.
- **Статус:** открыта; это не подтверждённый дефект.
- **Суть и последствия:** bounded interpreter performs source read `DS:[RSI/ESI]` before destination write `ES:[RDI/EDI]`, including the S2-19l address-size-32 REP form. This local order means a source failure prevents destination access; it does not establish hardware exception priority when both are invalid. Destination failure follows a successful source read and precedes commit of destination bytes and index/count state for that iteration under the bounded all-or-error memory contract. No MMIO rollback is claimed.
- **Основание:** S2-19d and S2-19l `rep_movs_faults_preserve_only_fully_transferred_iterations` matrices check source-first traces and retry behavior. S2-18o adds `movs_address32_dual_invalid_accesses_observe_local_source_first_policy` and four `s2_18o_dual_unmapped_{b,w,d,q}.case.json` fixtures. These local tests are not a physical x86 comparison.
- **Следующий шаг:** при необходимости аппаратного dual-fault claim найти нормативное основание либо провести изолированное dual-invalid сравнение на physical x86 в общем финальном acceptance substage. До результата оставлять порядок source-first только local policy. Реализация S2-19l описана в [плане](plans/STAGE_2.md#s2-19l-bounded-rep-movs-с-address-size-32) и [контракте](specs/CPU_CONTRACT.md#local-s2-19l-bounded-rep-movs-address-size-32).

### ISSUE-023 — Layout checker reports worktree metadata and stale oracle links

- **Дата обнаружения:** 2026-10-27.
- **Компонент:** `tools/check_layout.py`, current Git worktree metadata and archived Stage 1/2 oracle report links.
- **Статус:** открыта; инфраструктурная проблема checkout, воспроизведена также при проверке S2-35.18.
- **Суть и последствия:** Layout validation reports the root `.git` worktree pointer as an unmapped file and two missing historical oracle manifests referenced by `docs/reports/STAGE_2_REPORT.md`. This prevents a clean layout-check result in this checkout; it does not affect interpreter or fixture execution.
- **Воспроизведение:** `python3 tools/check_layout.py` from the repository root in the linked Stage 2 worktree reports `Unmapped root file: .git` and broken local links to `work/stage2-oracle/stage2-20261022-230853-3684f650/manifest.json` and `work/stage1-oracle/stage1-20261022-230241-3b73e810/manifest.json`.
- **Следующий шаг:** update the layout checker to recognize Git worktree pointer files and restore the referenced manifests or correct their archived references in the report. Do not hide the failures by moving the checkout metadata or fabricating evidence.

### ISSUE-024 — S2-36 result APIs lacked large-gap rounding

- **Дата обнаружения:** 2026-10-28.
- **Компонент:** `od-isa` finite binary addition/subtraction result APIs `Binary32`/`Binary64`, S2-36o/p.
- **Статус:** решена для новых result APIs в S2-36p; старые low-level exact-alignment и legacy convenience APIs намеренно сохраняют bounded контракт gap≤64.
- **Суть и последствия:** до S2-36p новые result APIs возвращали `None` для конечных нормализованных операндов с exponent gap больше 64. Из-за этого полный finite диапазон локального add/sub result path не обрабатывался. Это было ограничением локального API, а не подтверждённым дефектом CPU и не определяло поведение x86-инструкций.
- **Историческое воспроизведение:** при прежней реализации gap 64 входил в bounded профиль, а gap 65 был первым отказом для result API. После S2-36p новые `add_finite_aligned_result`/`subtract_finite_aligned_result` обрабатывают gap до максимального конечного диапазона; literal-векторы проверяют gap 65 и 2097, все режимы округления, оба знака, операции и границы subnormal/overflow. Low-level `add_finite_aligned`/`subtract_finite_aligned` остаются bounded по контракту и не входят в закрытую проблему.
- **Результат:** private large-gap tail decomposition и round-equivalent bridge для precision 24/53 реализованы, независимо проверены и документированы в [плане S2-36p](plans/STAGE_2.md#s2-36p--finite-binary-addition-and-subtraction-result-apis-across-the-full-finite-exponent-range) и [отчёте S2-36p](reports/STAGE_2_REPORT.md#s2-36p--finite-binary-addition-and-subtraction-result-apis-across-the-full-finite-exponent-range). Авторские проверки: binary-float 48/48, x87 47/47, workspace offline check, format, diff check и Graphify update. Аппаратная/SSH регрессия отложена до общего финального substage и этим закрытием не заявляется.
- **Следующий шаг:** отсутствует для S2-36p result path. При отдельном расширении публичного low-level/legacy API потребуется явно определить новый контракт; их текущий лимит сам по себе не является открытой проблемой S2-36p.

### ISSUE-025 — Intel RCPSS/RCPPS threshold inequalities overlap

- **Дата обнаружения:** 2026-10-28.
- **Компонент:** нормативное описание legacy SSE binary32 reciprocal approximation; локальный reference helper S2-36ab и RCPSS/RCPPS profiles S2-37b/S2-37d/S2-37dy/S2-37ea.
- **Статус:** открыта; подтверждённая неоднозначность опубликованного текста, не подтверждённый runtime-дефект.
- **Основание и последствия:** Intel SDM Rev. 093, Vol. 2B, RCPPS pp. 4-538–4-539 and RCPSS pp. 4-540–4-541 describe threshold conditions for finite inputs/results, but the printed inequalities overlap on a numeric region. Thus the document does not yield a unique universal hardware cutoff around the normal/subnormal reciprocal-result boundary. The same entries state that the result is approximate with relative error at most `1.5 * 2^-12`, the operation is independent of MXCSR RC, denormal sources are treated as signed zero, tiny results are flushed to signed zero, SNaN is quieted, QNaN is returned, and no SIMD FP exceptions are signaled. The local S2-36ab helper therefore defines an explicit deterministic policy (exact same-format `1.0/source` rounded nearest-ties-even, then flush subnormal outputs), not a bitwise claim about hardware in the ambiguous region.
- **Воспроизведение/условие:** inspect the RCPPS and RCPSS threshold inequalities on the cited pages; compare their stated cutoff ranges numerically. The ambiguity affects expected hardware bits for normal inputs whose reciprocal lies near the normal/subnormal output transition. Local tests validate the stated error bound with integer-only products and do not assert a universal hardware cutoff.
- **Следующий шаг:** final Mac mini hardware acceptance should validate the `3/8192` relative-error envelope for non-flushed normal outputs, without requiring exact equality to the helper's bits. Treat the ambiguous tiny-result boundary as a separate unresolved question; resolve the overlapping inequalities against Intel errata or another authoritative clarification. Until then, retain the explicit local policy and do not claim a universal hardware cutoff or exact helper-bit match near the boundary. See [S2-36ab report](reports/STAGE_2_REPORT.md#s2-36ab--deterministic-raw-binary32-sse-reciprocal-reference), [S2-37b report](reports/STAGE_2_REPORT.md#s2-37b--local-only-long64-legacy-register-rcpss), [S2-37d report](reports/STAGE_2_REPORT.md#s2-37d--local-only-long64-legacy-register-rcpps), [S2-37ea report](reports/STAGE_2_REPORT.md#s2-37ea--rcpss-xmm0m32-rax), [S2-36ab plan](plans/STAGE_2.md#s2-36ab--deterministic-raw-binary32-sse-reciprocal-reference), [S2-37b plan](plans/STAGE_2.md#s2-37b--local-long64-register-only-legacy-rcpss), [S2-37d plan](plans/STAGE_2.md#s2-37d--local-long64-register-only-legacy-rcpps), and [S2-37ea plan](plans/STAGE_2.md#s2-37ea--local-long64-memory-legacy-rcpss-at-rax).

### ISSUE-026 — Plain Long64 FXSAVE/FXRSTOR target lacks selector state

- **Дата обнаружения:** 2026-10-29.
- **Компонент:** `od-isa::state::X87State`; plain Long64 `FXSAVE/FXRSTOR` in S2-39.
- **Статус:** resolved for independent selector representation and the bounded S2-39c/d save plus S2-39e restore profiles. Broader S2-39 forms and physical acceptance remain open outside this issue.
- **Суть и последствия:** `X87State` and strict schema-v2 `X87Snapshot` represent FCS/FDS independently as optional 16-bit selectors while preserving 64-bit FIP/FDP. `None` means unavailable/unrepresented; `Some(0)` is known zero. Plain `s2-long-fxsave-rip-v1` executes only unprefixed Long64 `0F AE 05 disp32` and requires both selectors plus x87/SSE state. The separate `s2-long-fxsave64-rax-disp8-v1` and `s2-long-fxrstor64-rbx-disp8-v1` profiles execute only exact forms `48 0F AE 40 40` and `48 0F AE 4B 40`; both accept selectors independently optional because the 64-bit image stores full FIP/FDP. Existing unrelated profiles still omit/reject selectors, and modeled x87 transitions fail closed when selector values are present. Guest exception delivery, broader addressing forms, and full system save/restore remain outside this issue.
- **Основание и воспроизведение:** S2-39b schema/profile tests cover selector availability and boundaries. S2-39c/d/e local tests cover pointer layouts, TOP rotation, abridged tags, XMM registers, immutable MXCSR policy, preserved/ignored image bytes, strict decode, callback order/failure and bounded round-trip where tag information is representable. Sources remain the selected target's leaf-7-zero rule and Intel SDM Rev. 093 Vol. 2A Tables 3-45..3-49 / Vol. 1 §§10.5.1..10.5.3; Ubuntu static mappings are in [S2-39a evidence](reports/STAGE_2_LINUX_PROFILE.md#s2-39a--fxsave-evidence-only).
- **Следующий шаг:** no further work is required to resolve selector representation for these bounded forms. Physical/Mac mini validation is deferred to the combined final hardware substage; no such result is claimed here. Broader S2-39 requirements remain tracked in the [Stage 2 plan](plans/STAGE_2.md#s2-39-сохранение-и-восстановление-fpsimd), [S2-39e contract](specs/CPU_CONTRACT.md#s2-39e--exact-long64-fxrstor64-rbx0x40), and [S2-39e report](reports/STAGE_2_REPORT.md#s2-39e--exact-long64-fxrstor64-rbx0x40).

### ISSUE-027 — First EFER access and startup control snapshot are unproven

- **Дата обнаружения:** 2026-10-29.
- **Компонент:** S2-00/S2-25 startup evidence for Ubuntu 7.0.0-38.38, IA32_EFER and initial CR0/CR4.
- **Статус:** открыта; это ограничение статического evidence, не подтверждённый дефект реализации.
- **Суть и последствия:** exact source/image mapping proves an EFER RDMSR and conditional WRMSR in the main-kernel direct64 `common_startup_64` continuation. It does not establish that this is the absolute first EFER access across all earlier compressed/decompression/`__pi_*` helpers, nor does static code reveal the actual EFER/CR0/CR4 entry snapshot. The nearby compiled CR0 load `0x80050033` includes AM and is not an entry snapshot; existing S2-25a uses a synthetic `0x80010033` base. Treating these as observed state or admitting EFER WRMSR as storage-only would overstate the evidence and omit mode/paging effects.
- **Основание:** authenticated source/archive hashes and image-derived/debug ELF mapping at `ffffffff8139c1b4` (`RDMSR`) and `ffffffff8139c1d5` (conditional `WRMSR`), with source `arch/x86/kernel/head_64.S:379–401`; see [S2-25d mapping](reports/STAGE_2_LINUX_PROFILE.md#ia32-efer-on-the-mapped-main-kernel-direct64-continuation).
- **Следующий шаг:** audit all earlier compressed/decompression and `__pi_*` helper paths for EFER accesses and capture/establish the relevant startup control-state assumptions through independent evidence. Keep any next CPU slice read-only until WRMSR mode and paging effects have an owning contract; Stage 4 remains owner of system translation effects.
- **S2-25e:** implemented a local exact read-only `RDMSR IA32_EFER` fixture profile over four synthetic values with bounded current CR0/CR4 prerequisites. This does not change the open status: no actual Ubuntu entry value, initial CR snapshot, absolute-first-access ordering, WRMSR behavior, guest exception delivery, paging effect, or runtime boot observation is established. See [S2-25e CPU contract](specs/CPU_CONTRACT.md#s2-25e--exact-read-only-long64-rdmsr-ia32_efer) and [implementation report](reports/STAGE_2_REPORT.md#s2-25e--exact-read-only-long64-rdmsr-ia32_efer).
