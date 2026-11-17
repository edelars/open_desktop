# Модель памяти гостя x86 и владение RAM (S6-04)

Статус: контракт модели для последующей реализации; это не описание уже реализованного SMP backend. Ограниченные host primitive choices для aligned same-width accesses измерены в S6-06 ниже; завершённый RAM backend остаётся за последующими задачами. Гостевая модель — x86-TSO для обычной когерентной WB-памяти; MMIO, page-walk side effects и смешанные/разделённые доступы ограничиваются отдельно ниже.

## Нормативная модель и источники

Основная модель — x86-TSO из Owens, Sarkar и Sewell, *A Better x86 Memory Model: x86-TSO* (extended version, revision 1746, 25 March 2009), §§3.1–3.2 и Appendix A ([PDF](https://www.cl.cam.ac.uk/~pes20/weakmemory/x86tso-paper.pdf)). У неё есть store buffer на каждом логическом процессоре: загрузка может обойти более раннюю запись того же процессора к другому адресу, а записи становятся видны другим процессорам в порядке программы. Поэтому Store Buffering (SB) с результатом `0,0` разрешён. Независимые чтения не могут наблюдать записи в разных порядках: IRIW исход запрещён. Эти примеры также приведены в финальной публикации CACM 2010, §§1, 2.2 и 3.1.

Статья описывает когерентные WB-области и выровненные accesses; она исключает faults, смешанные размеры и невыровненные accesses, non-temporal операции и обновления таблиц страниц. Эти случаи определяются Intel SDM и явными контрактами проекта, а не экстраполяцией из модели статьи. Нормативная сверка x86 выполнена по Intel SDM revision 093 (September 2026): Vol. 3A, §§11.1.1–11.1.2.3, 11.2.2, 11.2.3.4, 11.2.3.7–11.2.3.9 и 5.8; Vol. 2A, `LOCK` (pp. 3-565–566); Vol. 2D, `XCHG` (pp. 6-32–34). [Vol. 3A](https://cdrdv2.intel.com/v1/dl/getContent/671190), [Vol. 2A](https://cdrdv2.intel.com/v1/dl/getContent/671199), [Vol. 2D](https://cdrdv2.intel.com/v1/dl/getContent/671143).

Для ARM-кандидатов использована Arm ARM DDI 0487H.a, ID020222 (2022), §§B2.2.1, B2.3.3–B2.3.6, B2.3.11, B2.5.2, tables B2-1 and alignment discussion (B2-152, B2-162–165, B2-173, B2-177–178, B2-186–187) ([Arm ARM](https://documentation-service.arm.com/static/623b2de33b9f553dde8fd3b0)). Это зафиксированная редакция источника, не утверждение, что она последняя.

Гарантии инструкций fence дополнительно сверены по Intel SDM revision 093: Vol. 3A §11.2.5 (p. 11-18), Vol. 2A `LFENCE` (p. 3-552), Vol. 2B `MFENCE` (p. 4-15) и `SFENCE` (p. 4-628): [Vol. 2B](https://cdrdv2.intel.com/v1/dl/getContent/671241).

## Гарантии и классы доступа

«Режим чередования» означает, что все vCPU и все источники записи устройства в RAM работают на одном host-потоке; переключение происходит только на границах инструкции/события. Для будущего параллельного режима строки таблицы задают достаточные по смыслу кандидаты, а не выбранную реализацию.

| Класс | Гарантия гостю | Обеспечение в режиме чередования | Кандидаты для параллельного режима |
|---|---|---|---|
| Обычная загрузка RAM | Обычный load подчиняется x86-TSO; естественно выровненные 1/2/4/8-байтовые RAM-accesses целостны. Load не получает неявный глобальный acquire. | Один поток; полный доступ завершается до переключения. | Rust `SeqCst` атомики одинакового размера дают достаточный порядок и запрещают IRIW, но сильнее TSO (в частности запрещают SB 0,0); это не решение для перекрывающихся разных размеров. Acquire-only недостаточен для IRIW. ARM64: aligned `LDR` с последующим `DMB ISHLD` либо `LDAR` — кандидат на нужные load-order edges; область — Normal Inner-Shareable memory и условия выравнивания ниже. IRIW sufficiency для этой схемы — вывод из Arm ARM §§B2.3.3/B2.3.6, а не отдельная прямая гарантия примера. |
| Обычная запись RAM | Записи одного CPU наблюдаются в порядке программы; допускается TSO store-buffer обход более ранней записи загрузкой. | Один поток, без конкурентного наблюдателя во время инструкции. | Rust `SeqCst` stores/loads достаточны, но могут запрещать SB; release-only не задаёт общего порядка IRIW. ARM64 aligned `STR` перед `DMB ISHST`, либо `STLR`, — кандидат на store-order edges; для пары load/store использовать схему load-side `DMB ISHLD` и store-side `DMB ISHST` либо LDAR/STLR. Все схемы — только aligned Normal ISH accesses, не доказательство Rust safety. |
| `LOCK` формы, принимаемые текущим декодером | Каждая операция атомарна относительно всех CPU и имеет полный порядок locked operations; locked operation упорядочивает обычные loads/stores до и после. Декодированные формы: `LOCK CMPXCHG m32/m64`; `LOCK INC/DEC m32/m64`; `LOCK OR m8/m32/m64, imm`; `LOCK ADD m8/m32/m64, imm`; `LOCK ADD m32/m64, reg`; `LOCK XADD m32/m64`; `LOCK AND m8`; `LOCK BTS/BTR m64, register-index`; `LOCK BTS m32/m64, imm`. DS-substituted XADD/CMPXCHG не считаются настоящим `LOCK`; отличие сохранить и пересмотреть в S6-20 step 6. | Инструкция выполняется без переключения; атомарная операция — единый шаг последовательного исполнения. Для операнда, помещающегося в выровненные 16 байт, целевая реализация S6-31. Split LOCK: остановка остальных vCPU по S6-34 step 8; до этого явная ошибка со счётчиком. | Один общий сериализованный atomic/lock protocol для памяти и всех наблюдателей, включая DMA; выбранная примитива должна покрыть размер, порядок и ошибки. Rust атомики разных размеров не дают общего безопасного mixed-width API. ARM64 exclusive/LSE или lock protocol могут быть исследованы; ARM barriers сами по себе не делают RMW атомарным. Не выбирать primitive до S6-06/31. |
| `XCHG` с памятью | Memory `XCHG` implicitly locked, даже без префикса `LOCK`; имеет те же atomicity/order требования locked операции. Текущие CPL0 формы — m32/m64. | Единый шаг без переключения. | Тот же общий RMW serialization contract, что для `LOCK`; Rust атомарный обмен допустим только при согласованной ширине/алиасинге и полном порядке. ARM LSE/exclusive или lock protocol плюс необходимые барьеры — кандидаты, не выбранное решение. |
| `MFENCE` | Все предшествующие loads и stores становятся глобально видимыми до последующих loads и stores (Intel SDM Vol. 3A §11.2.5; Vol. 2B `MFENCE`). Это полный барьер обычной памяти. | Инструкция является точкой порядка в последовательной очереди событий. | Rust `SeqCst` fence — кандидат только внутри модели синхронизированных атомарных accesses; он не исправляет data race. ARM `DMB ISH` вокруг границы — более сильный кандидат; безбарьерные варианты недостаточны. |
| `SFENCE` | Упорядочивает предшествующие записи относительно последующих записей; точные эффекты для WC/non-temporal вынесены в упрощения. | Очередь событий уже сохраняет порядок; отдельное действие не требуется. | Rust release/SC fence только при правильно синхронизированных atomics; ARM `DMB ISHST` кандидат для store→store. Для принятого профиля обычной WB RAM отдельный fence может быть избыточен; non-temporal модель упрощена ниже. |
| `LFENCE` | Все предыдущие инструкции завершаются локально до завершения fence; последующие инструкции не начинают исполнение до этого. Предыдущие stores при этом могут оставаться глобально невидимыми (SDM Vol. 3A §11.2.5; Vol. 2A `LFENCE`). | Очередь событий задаёт требуемую границу между инструкциями. | Для одной только наблюдаемой memory-order части ARM `DMB ISHLD` — кандидат, выведенный из Arm ARM §§B2.3.3/B2.3.6; он не доказывает и сам по себе не реализует сериализацию исполнения всей инструкции. Нужна отдельная модель CPU-профиля, если локальная execution serialization наблюдаема. |
| Page walk и A/D bits | Каждая запись PTE читается одним 8-байтовым доступом. Обновление accessed/dirty — атомарный compare-and-replace; если значение таблицы изменилось между чтением и заменой, обход повторяется с этого уровня, чтобы не потерять бит другого vCPU (правило §4.2 плана). Intel SDM Vol. 3A §5.8 описывает установку A/D, а §11.1.2.1 — locked cycles. | Page walk выполняется в ходе одного события; A/D write — отдельная RAM commit transaction. | Через общий RAM atomic path, согласованный с CPU stores, DMA и TLB/page-walk rules. Одних DMB или Rust Ordering недостаточно без compare-and-replace/retry и sticky A/D semantics. |
| Устройство пишет в RAM (DMA) | RAM-изменение от устройства участвует в когерентном наблюдении по порядку события, определённому моделью устройства и CPU; не может обходить контракт общей RAM. | Устройство доставляет запись как event на том же host-потоке между инструкциями. | Общий RAM ownership/commit protocol и определённый device ordering handoff; release/acquire или barriers — лишь возможные части протокола, не самостоятельная гарантия DMA. |
| MMIO access | Это device access с порядком и side effects конкретного устройства; не обычная когерентная WB RAM операция и не подменяется RAM fence semantics. | Синхронная dispatch к модели устройства на границе события. | Device model задаёт сериализацию и порядок; отдельный lock/queue per device и архитектурный fence mapping — кандидаты. Не использовать обычный RAM atomic API как контракт MMIO. |
| Дескрипторные side effects (отдельный класс) | Изменение descriptor Accessed byte, TSS busy state и аналогичные side effects подчиняются instruction-specific x86 fault/commit semantics; не объединяются с ordinary operand store или page-table A/D update. Контракт bounded paths — S6-26 ниже. | Текущая семантика инструкции выполняется единым событием; side effect фиксируется только в определённой инструкции точке commit. | Для S6-30 нужна descriptor-update transaction с архитектурным atomicity scope, сохранением нецелевых битов и видимыми generation/callback semantics; это отдельный контракт от guest LOCK operations. Rust Ordering или DMB сами по себе его не задают. |

## Host-кандидаты и ограничения языковой модели

### Rust atomics

Сверка выполнена по официальным Rust std docs 1.100.0 (commit `0ef1bc386`, 2026-11-09): [atomic access memory model](https://doc.rust-lang.org/std/sync/atomic/index.html#memory-model-for-atomic-accesses), [Ordering](https://doc.rust-lang.org/std/sync/atomic/enum.Ordering.html), [`AtomicU64::from_ptr`](https://doc.rust-lang.org/std/sync/atomic/struct.AtomicU64.html#method.from_ptr), [`UnsafeCell`](https://doc.rust-lang.org/std/cell/struct.UnsafeCell.html).

`Acquire`/`Release` синхронизируют только когда acquire наблюдает значение, выпущенное release; сами по себе они не создают единого общего порядка и не запрещают IRIW. `SeqCst` задаёт единый total order всех участвующих SeqCst операций и потому запрещает IRIW, но также запрещает разрешённый x86-TSO SB исход `0,0`; это корректное, более сильное упорядочение только для участков, представимых без несинхронизированных конфликтующих mixed-size accesses.

Rust atomic access model запрещает несинхронизированные конфликтующие atomic accesses разных размеров к перекрывающимся байтам. Более сильный `Ordering` этого запрета не снимает: гостевой byte write против пересекающегося qword read нельзя просто транслировать в разные по ширине `AtomicU*` views. Поэтому атомарный host candidate требует общего lock/serialization scheme либо доказуемо единой ширины и layout для всех перекрывающихся обращений; конкурентные plain accesses также должны быть исключены или синхронизированы.

`UnsafeCell<T>` разрешает внутреннее изменение значения через shared reference, но не отменяет уникальность активного `&mut T` и не синхронизирует потоки. Он не предотвращает гонки данных. Поэтому заменить `Vec<u8>` на `UnsafeCell<Vec<u8>>` недостаточно: без протокола владения два host-потока всё ещё могут одновременно обращаться к одним байтам, включая конфликтующие ширины, и нарушить Rust memory safety.

### ARM64 assembly candidate

Кандидаты по Arm ARM DDI 0487H.a: aligned `LDAR`/`STLR`, либо `DMB ISHLD` после load и `DMB ISHST` перед store, дают необходимые TSO program-order edges для Normal Inner-Shareable accesses; `DMB ISH` вокруг операций сильнее и может запретить SB. Запрет IRIW для LDAR/ISHLD mappings — вывод из описания наблюдаемого порядка в §§B2.3.3/B2.3.6, а не прямой отдельный тест/утверждение спецификации. Ни один barrier сам по себе не обеспечивает атомарность RMW или безопасность параллельного Rust-кода.

Выравнивание проверяется по host pointer, не только по guest address. Обычные `LDR/STR` имеют single-copy atomicity при естественном выравнивании (размер доступа) по B2.2.1. `LDAR/STLR` требуют естественного выравнивания и иначе fault (B2.3.11/B2-178). При `SCTLR.A=0` обычные невыровненные Normal accesses допустимы, но без FEAT_LSE2 имеют только byte-level single-copy atomicity. FEAT_LSE2 обеспечивает full-width single-copy atomicity для access, все байты которого находятся внутри одного aligned 16-byte block и Normal Inner/Outer WB cacheable memory; выход за эту область может оставить только byte-level atomicity либо fault в зависимости от `nAA` (B2.5.2). Невыровненные LDAR/STLR подчиняются этому же LSE2 envelope; без него они fault при `SCTLR.A=0`, а при `SCTLR.A=1` fault. Проект может выбрать более узкий aligned-only assembly contract.

Нельзя вывести требуемое host pointer alignment лишь из guest alignment или типа backing allocation: [`Vec<T>` layout contract](https://doc.rust-lang.org/std/vec/struct.Vec.html#guarantees) выравнивает allocation по `T`, а `u8` имеет alignment 1. S6-30 должен явно предусмотреть и проверить выравнивание host backing для всякой atomic/LDAR/STLR ширины или обеспечить её другим контролируемым layout/primitive.

## Выбор по замеру (S6-06)

Три correctness-checked release-прогона на Apple M3 Max, macOS 26.7.1,
`aarch64-apple-darwin`, с 128-byte cache line, сравнили кандидатов и сохранили
raw samples, разброс, fingerprints и disassembly в
[отчёте S6-06](../reports/data/stage6/s6-06-host-summary.md). Каждая обычная
операция/кандидат агрегирует 1,200 paired sample ratios (3 процесса × 20 пар ×
4 ширины × 5 thread/layout профилей); RMW 1/2/4/8 агрегирует 240 пар на профиль,
а 16-byte RMW — 60. Выбор ниже ограничен измеренными aligned same-width
accesses; это не готовый parallel RAM backend.

- Для ordinary load/store одного размера из набора 1/2/4/8 байт выбран Rust
  `SeqCst` atomic API как safe host candidate. Load имел 2.237× relaxed-baseline
  elapsed time (pooled IQR 1.855–2.260); store имел 1.047× (IQR 1.000–2.276),
  поэтому ускорение store не заявляется. `LDAR`/`STLR` и `LDR/STR; DMB`
  измерены, но не выбраны: inline assembly сама не задаёт Rust synchronization
  contract. Same-line store spread был широким.
- Для same-width atomic RMW 1/2/4/8 байт выбран Rust std `fetch_add(SeqCst)`.
  Disassembly этого benchmark показал `LDADDALB/H/W/X`. Без конкуренции Rust
  std имел paired ratio 0.541× LL/SC (IQR 0.539–0.544), почти как explicit LSE
  0.542× (0.539–0.544). При четырёх потоках Rust std pooled ratio был 0.870×
  (IQR 0.740–1.015); pooled IQR включает 1.0, как и width-specific IQR для трёх
  из четырёх размеров. Custom LSE probe с `DMB ISH` вокруг операции занимал
  1.588× время LL/SC (IQR 1.352–1.843) и не выбран.
- Для aligned 16-byte atomic RMW выбран LL/SC pair (`LDAXP`/`STLXP`) вместо
  `CASPAL`: CASP занял 1.265× времени LL/SC без конкуренции (IQR 1.259–1.271)
  и 3.672× при четырёх потоках (3.014–4.176). Rust std не предоставляет
  128-bit atomic type, поэтому будущая `UnsafeCell`/ownership обёртка требует
  отдельного аудита.

Rust `SeqCst` задаёт более сильный порядок, чем x86-TSO, и запрещает разрешённый
guest SB `0,0` outcome. Кроме того, atomic accesses разных размеров к
перекрывающимся байтам нельзя смешивать по Rust memory model. Поэтому эти
результаты задают host primitive для однородных aligned width cases, но не
выбирают завершённый RAM mapping, не снимают mixed-width ограничения и не
решают общий writer/reader ownership protocol. Page walk, DMA, descriptor,
split-lock и MMIO transactions сохраняют свои отдельные требования.

## Атомарность и разделённые доступы

- Обычные естественно выровненные 1/2/4/8-байтовые guest RAM accesses выполняются целиком. Intel SDM Vol. 3A §11.1.1 также гарантирует для P6+ невыровненные 16-, 32- и 64-битные accesses, если они целиком находятся в одной cache line; проектная модель не опирается на это расширение: D8 разрешает обычным невыровненным accesses быть видимыми по частям. Это явное упрощение для избежания остановки всех vCPU на каждом таком access.
- Обычная невыровненная RAM операция делится на упорядоченные возрастающие по адресу однобайтовые logical commits. Virtual range сначала режется по границам guest page; каждый page segment переводится по таблицам своей страницы. Host memory operation не пересекает cache line или page boundary. Гарантия естественно выровненного 1/2/4/8-байтового доступа относится к access, который не разбит этим правилом. Реализация может объединять host операции только если сохраняет эту byte-level visibility и порядок.
- В выбранном проектном контракте до записи данных при page-crossing write проверяются права всех затронутых guest pages; затем байты фиксируются по возрастанию адреса. Page fault при preflight не фиксирует байты операнда, но уже совершённые page-walk A/D side effects остаются committed. Ошибка RAM после начала byte commits может оставить уже записанный prefix. При faulting read destination state следует CPU contract конкретной инструкции; page-walk A/D side effects от успешных частей translation могут остаться committed. Это правило bounded profile, а точный fault priority каждой инструкции остаётся её CPU contract.
- Access, пересекающий cache line или guest page, не имеет общей гарантии atomicity. Переводы и commits происходят по указанному split rule; здесь не утверждается all-or-nothing store при поздней ошибке host или иной instruction-specific fault.
- Все `LOCK`-операции и memory `XCHG` остаются атомарными. Операнд, который помещается в aligned 16-byte block, использует будущую S6-31 primitive. Остальные split LOCK требуют stop-all-vCPUs из S6-34 step 8; до готовности этого пути операция возвращает явную учитываемую ошибку. Это политика D8, не реализация hardware split-lock #AC.
- Split-lock detection `#AC` не моделируется и не сообщается гостю. Split LOCK следует проектной atomicity policy либо текущей counted error.

## Текущее владение RAM и commit

Текущая `PhysicalMemory` хранит RAM в `Vec<u8>` по регионам: `read_bytes(&self)` и `write_bytes(&mut self)` задают эксклюзивный API, а не параллельный доступ. `PagedMovMemoryAdapter` также имеет mutable memory ownership и сообщает committed write callback.

Одна успешная `PhysicalMemory::write_bytes` physical RAM commit transaction увеличивает generation каждого затронутого физического RAM page, затем вызывает committed callback ровно один раз после этого commit. Ошибка валидации до commit сохраняет bytes/generations и не вызывает callback. Это свойство одной backing-memory transaction, а не всей гостевой инструкции: инструкция может породить несколько отдельных A/D и operand commits, каждый со своим generation/callback; более поздний fault не отменяет уже committed A/D side effects. Framebuffer backing вне RAM generation/callback: его записи не bump-ят generation и не уведомляют committed-write consumer. Эти правила нужно сохранить при общей transaction boundary.

В interleaving mode все RAM writers (vCPU и устройства) должны идти через одного host-поточного владельца и переключаться на instruction/event boundaries. Параллельный режим потребует общего проверяемого ownership/synchronization protocol для всех RAM writers и readers; долгоживущие `&mut` aliases к concurrently mutable RAM недопустимы. Обычный RAM access не берёт глобальную блокировку как окончательное архитектурное требование, но S6-04 не выбирает primitive и не задаёт fast path. MMIO остаётся в модели устройства.

## Побочные записи дескрипторов (S6-26)

Descriptor side effects — отдельный класс instruction commits. Их нельзя
отождествлять с operand store или page-table A/D updates. Intel SDM Vol. 3A
§11.1.2.1 задаёт `LOCK` semantics для implicit Accessed-bit update, а Vol. 2A
`LTR` задаёт locked RMW всего TSS descriptor; для 64-bit mode descriptor
имеет 16 байт. Эти семантические границы не задают физическую ширину bus
transaction. Они также не превращают descriptor update в guest `LOCK`
instruction из S6-20/S6-31. Источники: [Vol. 3A §3.4.5.1 (revision 093,
p. 3-13)](https://cdrdv2.intel.com/v1/dl/getContent/671190#page=101), [Vol. 3A
§11.1.2.1 (revision 093, p. 11-3)](https://cdrdv2.intel.com/v1/dl/getContent/671190#page=317),
[Vol. 2A `LTR` (revision 093, pp. 3-576–3-577)](https://cdrdv2.intel.com/v1/dl/getContent/671199#page=694),
[Vol. 2A `IRET`](https://cdrdv2.intel.com/v1/dl/getContent/671199#page=608),
[Vol. 2B `RET`](https://cdrdv2.intel.com/v1/dl/getContent/671241#page=583).

Таблица ниже описывает видимые commit points и эффекты именно текущих
ограниченных emulator profiles, а не все fault-ordering детали Intel CPU и не
внутреннюю ширину его bus accesses. Сначала выполняются проверки, которые
перечислены для конкретного пути; затем эмулируемый side-effect write; после
его успеха меняется соответствующее CPU state. Intel SDM описывает проверки
инструкций, но не задаёт здесь общий rollback протокол для A-bit writes при
поздней ошибке; модель не делает такого утверждения. Любые page-walk A/D
commits остаются отдельными эффектами своего пути.

| Путь профиля | Проверки до descriptor write | Логический GuestMemory write и граница CPU commit | Поздняя ошибка |
|---|---|---|---|
| [`far_return_cpl0` (`LRETQ`)](CPU_CONTRACT.md#lretq-на-cpl0-в-наборе-s2-47b) | Два stack qword read; selector/GDT bounds; CS type, long-mode flags, DPL, present; target canonicality. | Если A=0, один логический qword write дескриптора с A=1; после его успеха коммитятся CS/RIP/RSP. Если A=1, descriptor write отсутствует. [Intel RET](https://cdrdv2.intel.com/v1/dl/getContent/671241#page=583). | До descriptor write CPU state не меняется. Уже произошедшие page-walk A/D commits сохраняются. Ошибка descriptor write не коммитит CS/RIP/RSP. |
| [`iretq_cpl0` (`IRETQ`)](CPU_CONTRACT.md#iretq-на-cpl0-в-наборе-s4-38l5b) | Текущий bounded CPL0→CPL0 path читает пять frame qwords и проверяет CS вместе с non-null SS до side effects; это профильное поведение, не общий IRET privilege-transition contract. [Intel IRET](https://cdrdv2.intel.com/v1/dl/getContent/671199#page=608). | При A=0 CS, затем при A=0 SS записываются отдельными логическими qword writes; CPU state коммитится после обоих. | Если запись SS завершается ошибкой после успешной CS write, CS A-bit и предшествующие page-walk effects сохраняются, CPU state не коммитится. |
| [`load_sreg_cpl0` (ограниченные segment loads)](CPU_CONTRACT.md#mov-sreg-ax-с-ненулевым-селектором-на-cpl0-в-наборе-s4-38l6c) | Selector и descriptor read/validation соответствующей формы завершаются до записи; [DS](CPU_CONTRACT.md#s2-27b--exact-long64-mov-ds-ax), [ES](CPU_CONTRACT.md#s2-27c--exact-long64-mov-es-ax), [CPL3 FS](CPU_CONTRACT.md#s2-22f--cpl3-gdt-only-mov-fs-ax-descriptor-load), [CPL3 GS](CPU_CONTRACT.md#s2-22h--cpl3-gdt-only-mov-gs-ax). [Intel MOV Sreg](https://cdrdv2.intel.com/v1/dl/getContent/671199). | Если A=0, записывается логический qword descriptor image с A=1; segment state коммитится после успешной записи. | Ошибка write оставляет CPU state прежним; ранее committed page-walk A bits остаются. |
| Bounded DS/ES/FS/GS GDT helper ([DS](CPU_CONTRACT.md#s2-27b--exact-long64-mov-ds-ax), [ES](CPU_CONTRACT.md#s2-27c--exact-long64-mov-es-ax), [FS](CPU_CONTRACT.md#s2-22f--cpl3-gdt-only-mov-fs-ax-descriptor-load), [GS](CPU_CONTRACT.md#s2-22h--cpl3-gdt-only-mov-gs-ax)) | Descriptor lookup and selector checks precede the update. For CPL3 FS/GS, actual CPL governs selector checks; `DescriptorTable` accesses use Ring0 page-protection privilege while preserving CPL in the context. | Если A=0, один logical byte write по `descriptor + 5`; segment cache/selector commit следует за его успехом. [Intel Vol. 3A §11.1.2.1](https://cdrdv2.intel.com/v1/dl/getContent/671190#page=317). | Ошибка write не коммитит новый segment state; page-walk effects не откатываются. |
| [`load_tr_cpl0` (`LTR r/m16`, register form)](CPU_CONTRACT.md#ltr-rm16-rexb-0f-00-3-регистровая-форма-в-наборе-cpl0-s4-38l7am) | Проверки selector/GDT limit и canonical range, затем два qword read и TSS descriptor validation. 16-byte descriptor, пересекающий страницу, отклоняется профилем до descriptor accesses. [Intel LTR](https://cdrdv2.intel.com/v1/dl/getContent/671199#page=694). | Один logical qword write low half с TSS busy bit; только затем коммитится TR. High qword не записывается. Реализованный write footprint не меняет Intel locked-RMW semantic scope «entire descriptor». | Ошибка write сохраняет прежний TR. Уже выполненные translation A updates остаются. Пересечение страницы — отказ ограниченного профиля, не обещание guest fault. |
| [`commit_cpl0_gate` (bounded interrupt/trap delivery)](CPU_CONTRACT.md#внешнее-аппаратное-прерывание-через-idt-в-наборе-cpl0-s4-38l8a) | Gate/target checks выполняются до commit helper; helper пишет frame slots по очереди. Intel event/gate flow: [Vol. 3A §§7.12.1, 7.14.1](https://cdrdv2.intel.com/v1/dl/getContent/671190#page=215). | После frame writes, если A=0, один logical 8-byte target code descriptor write с A=1; затем коммитятся CPU entry state. | Descriptor-write error не откатывает уже записанные stack slots или page-walk effects; CPU entry state остаётся прежним. Ошибка frame write сохраняет предыдущие успешно записанные slots. |
| Bounded page-fault/double-fault delivery ([`deliver_page_fault`](../../crates/od-isa/src/interrupt.rs#L93), [`deliver_double_fault`](../../crates/od-isa/src/interrupt.rs#L110)) | Target descriptor checks; preflight descriptor-byte write вместе со всеми stack slots; затем последовательные stack writes. Intel exception delivery: [Vol. 3A §§6.14, 7.14.1](https://cdrdv2.intel.com/v1/dl/getContent/671190#page=215). | Если A=0, один logical byte write по `descriptor + 5` после frame; затем коммитится CPU entry state. `#PF` обновляет CR2 до этих accesses; `#DF` сам CR2 не меняет, но secondary `#PF` может его обновить. | Preflight failure не пишет frame. Поздняя ошибка stack/descriptor write оставляет уже committed RAM effects и актуальное значение CR2, но не коммитит CPU entry state. Это профильная emulator-visible последовательность, не заявление о microarchitectural fault visibility. |
| [`commit_paged_ltr_descriptor` (paged LTR)](CPU_CONTRACT.md#s4-18d--bounded-paged-ltr-ax-state) | Read translation, 16-byte descriptor read/validation, затем write-permission translation; 16-byte descriptor crossing 4 KiB page отвергается. Read/write translations должны совпасть. [Intel LTR](https://cdrdv2.intel.com/v1/dl/getContent/671199#page=694). | Page-walk A bits могут commit-иться во время проверок. После них leaf D bit commit-ится отдельной записью, затем одной 16-byte `PhysicalMemory::write_bytes` backing transaction записывается busy image; caller после успеха коммитит TR. | A bits, leaf D и callbacks, committed до ошибки descriptor image, сохраняются. Caller исключает DMA/vCPU/host writers на всю операцию; это не SMP-transaction. |

**Память, наблюдатели и будущий shared backend.** В таблице указана ширина
логического `GuestMemory` write; она не гарантирует один физический commit.
Один успешный `PhysicalMemory::write_bytes` — backing-memory transaction:
generation каждой затронутой RAM page увеличивается, затем один
committed-write callback сообщает физический диапазон. Но текущий
`PagedMovMemoryAdapter` для разрешённого cross-page store сначала проверяет
обе линейные страницы, затем пишет каждый байт отдельно. Поэтому один
логический qword descriptor write может дать несколько backing commits и
generation/callback событий; поздняя ошибка может оставить частично изменённый
descriptor. LTR paths отдельно отказывают для 16-byte descriptor через 4 KiB
границу. В serial/interleaving режиме эти byte writes выполняются внутри одного
синхронного instruction path без переключения vCPU или устройств; callbacks
могут видеть несколько commits, а поздний отказ может оставить их prefix.
Сохранить это как профильное поведение не означает объявить его пригодным для
parallel observers. Более поздний guest fault не откатывает уже committed descriptor,
frame, A/D или D effects; CPU entry state коммитится только в отмеченной
строке таблицы точке. Исключение — `CR2`, обновляемый при входе в bounded `#PF`
до попыток записи exception frame. Ошибка отдельного backing write не меняет
байты/generation и не вызывает его callback. Page-walk A/D принадлежат S6-25 и
проходят собственный update/commit path (реализован: один compare-and-replace
`PhysicalMemory::compare_replace_u64`, повтор обхода с уровня, см. `MACHINE_CONTRACT.md`, «S6-25»); descriptor A/Bits и busy image не
являются A/D.

**Общее правило descriptor access privilege.** Все descriptor-table memory
accesses используют `AccessOrigin::DescriptorTable`: при page protection это
Ring0, но `AccessContext.privilege` остаётся фактическим CPL и используется
instruction semantics для selector checks. Поэтому Ring0 page privilege не
снимает проверок CPL3 selector load. Это явно зафиксировано для bounded CPL3
[FS](CPU_CONTRACT.md#s2-22f--cpl3-gdt-only-mov-fs-ax-descriptor-load) и
[GS](CPU_CONTRACT.md#s2-22h--cpl3-gdt-only-mov-gs-ax) GDT paths.

В текущем serial instruction model между обращениями нет конкурирующего
наблюдателя. Для parallel shared RAM S6-30 должен предоставить descriptor-update
операцию, которая атомарно устанавливает Accessed/Busy state относительно
guest RAM readers/writers, DMA и других vCPU и сохраняет все нецелевые биты.
Для Accessed bit операция следует Intel `LOCK` semantics; для `LTR` её
семантический scope — весь descriptor. Обычная последовательность «прочитать
qword, отпустить ownership, записать старый image с изменённым битом» не
допускается: она может затереть конкурентное изменение. Cross-page logical
descriptor write может переводиться в несколько несмежных guest-physical
spans. Будущая atomic descriptor-update transaction должна сначала проверить
всю группу, затем сделать все spans видимыми вместе; generation обновляется
для всех затронутых RAM pages, а callback выдаёт один `GuestPhysicalRange` на
каждый непрерывный physical span только после общего group commit. Если
callback API не может представить группу, S6-30 должен расширить notification
contract или до первого byte descriptor write отказать этот cross-page path по
явной profile policy. Наблюдатель не должен увидеть span до commit остальных.
Инструкционный commit не делает весь stack frame, две IRET descriptor updates
или page-walk effects единой атомарной транзакцией.

Проверенный snapshot descriptor и snapshot translation должны быть связаны с
тем же atomic update, из которого берутся descriptor bytes для CPU cache/TR.
S6-30 может обеспечить это эксклюзивным transaction от read/validation до
update либо compare-and-retry: сравнить descriptor и translation с
проверенными snapshots, а при изменении заново выполнить соответствующие
instruction-specific checks и translations до commit. Нельзя применить
Accessed/Busy bit к изменённой записи и закоммитить CS/SS/TR из прежнего
snapshot. Это проектная политика shared backend; Intel SDM не задаёт общего
retry protocol. Для конкурирующих `LTR` она требует, чтобы после первого
успешного перехода Available→Busy второй `LTR` переоценил Busy descriptor и
отказал, а не закоммитил второй stale TR. В paged paths translation identity,
permissions и descriptor bytes должны быть подтверждены тем же update point;
если page tables изменились, путь повторно делает translation и descriptor
checks. Page-walk A/D effects предыдущей попытки остаются видимыми.

Доступ, меняющий descriptor, обязан оставаться полным по ширине, объявленной
его path (byte, qword или 16-byte image). Если его полный диапазон нельзя
проверить и коммитить без частично видимого descriptor write, shared adapter
должен отвергнуть его до первого байта; уже произошедшие page-walk A/D
effects при этом остаются. Два LTR paths уже явно исключают descriptor через
4 KiB boundary. Cross-page behavior других paths в текущем paged adapter может
состоять из последовательных byte commits, как описано выше; это наблюдаемое
bounded behavior, не контракт shared descriptor atomicity. В будущем shared
режиме такие paths должны использовать grouped physical-span transaction либо
отказ до первого descriptor byte по явной profile policy.

Эти операции не направляются через guest `LOCK` API S6-20/S6-31. В bounded
профилях гостевая инструкция не содержит LOCKed operand RMW: процессор сам
обновляет часть descriptor при загрузке selector, возврате/доставке события
или `LTR`. Переносимый низкоуровневый LOCK primitive сам по себе не сохраняет
instruction-specific validation, fault order, staged CPU-state commit или
описанную частичную видимость. S6-31 может предоставить primitive, который
будет использован внутри descriptor transaction, только после определения
этого отдельного контракта и без объявления guest LOCK semantics.

## Согласование адресных пространств (S6-17)

Свойство (Intel SDM Vol. 3A 4.10.4: инвалидация; ссылка по памяти, с первоисточником не сверена): после того как vCPU выполнил инвалидацию — `INVLPG` по адресу страницы, запись `CR3`, смена `CR4` (PGE, PAE, PSE, PCIDE, SMEP, SMAP…) или `CR0` (PG, WP), влияющая на перевод (список битов CR0/CR4, сбрасывающих кеш, — по памяти, не сверен), — он не использует старый перевод этой страницы (при записи `CR3` — ни одной не глобальной записи: `MOV CR3` не сбрасывает глобальные записи при `CR4.PGE`=1; по памяти, SDM 4.10.x, не сверено; при смене CR — по списку битов выше). До своей инвалидации vCPU после чужой записи в таблицы может видеть и старый, и новый перевод; оба допустимы. Запись в таблицу страниц сама по себе не инвалидирует кеш другого vCPU: это делает гость (shootdown: запись PTE, IPI, `INVLPG` на каждом получателе, подтверждение).

Требования к реализации:
1. Каждый кеш перевода (TLB, кеш выборки `FetchTranslationCache`, кеш S4-14i) принадлежит одному vCPU. Общий кеш без ключа vCPU запрещён.
2. Кеш, который сбрасывается по уведомлению о записи в кадр таблицы страниц (политика `WriteWatch`), обязан получать такое уведомление от записи **любого** vCPU: гостевой записи, записи A/D-битов обходом другого vCPU и записи устройством. Для режима чередования уведомление проходит через общий `committed_write` памяти; для параллельного режима это требование к S6-30 (межпоточное уведомление), без которого кеш с `WriteWatch` в параллельном режиме запрещён. Политика `GuestOnly` (кеш как TLB) этого не требует: её инвалидации приходят от самого vCPU.
3. Свою инвалидацию vCPU выполняет до следующего доступа после retire инструкции, не на границе кванта: перенос кеша через границу хода допустим только если события инвалидации внутри хода уже его сбросили (правило S4-14i «поколение» и «запечатывание» вызова).

Состояние на 2026-11-17: кеша перевода, переживающего квант vCPU в планировщике, нет (квант платы без кеша; `INVLPG m` — no-op, S4-38L7ab; постоянный кеш S4-14i к плате не подключён), поэтому свойство выполняется тривиально. Проверка: сценарий `shootdown` probe ([MACHINE_CONTRACT.md](MACHINE_CONTRACT.md#s6-17--согласование-адресных-пространств-и-shootdown), [SMP.md](../testing/SMP.md)); чувствительность не проверялась (S4-42 не выполнялась).

## Упрощения и исключённое поведение

- Если CPU profile допускает non-temporal write, она исполняется как обычный coherent RAM store. Это сильнее исходной семантики и намеренное упрощение; `SFENCE` поэтому не имеет дополнительного эффекта для обычной принятой WB RAM.
- Отдельный тип/семантика WC memory не поддерживается. WC-specific operation исключена выбранным профилем; MMIO продолжает идти через device contract, не через WC RAM.
- `CLFLUSH` не является memory barrier. Инструкция остаётся unsupported, если явно не включена профилем CPU; её нельзя эмулировать как fence.
- Split-lock detection `#AC` не моделируется; применяется проектная atomicity policy и до S6-34 — counted error, как указано выше.

## Связанные документы

- [CPU contract](CPU_CONTRACT.md) фиксирует связь CPU access semantics с этим профилем.
- [Архитектура проекта](../ARCHITECTURE.md#память-и-mmu) задаёт границы памяти, MMU и ownership.
- [План этапа 6](../plans/STAGE_6.md#s6-04-описать-контракт-модели-памяти-и-владения-гостевой-ram) описывает дальнейшую реализацию и приёмку.
