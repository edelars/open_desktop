# Испытательный стенд S0

Состояние на 22 октября 2026 года. Это инвентаризация доступного окружения, а не подтверждение прохождения тестов.

## S2-09: Linux native v2 проверен на Mac mini

Подготовлены native v2 ptrace path, Python harness и три hardware-layout
fixtures. Их [границы](CPU_ORACLE.md#native-v2-s2-09-границы-физического-подтверждения)
включают full-GPR capture с RSP, actual memory, low readable mappings без W+X
и точный count только по TRAP_TRACE. Host signals не трактуются как guest faults.
Физическая машина ниже теперь проверила и положительный MOV/memory v2 срез.

Локальные fmt/check/workspace tests и 17 Python harness tests прошли на M3 Max.
На Mac mini прошли Linux workspace check all-targets и od-x86-runner tests
(library 9, binary 22). Legacy run `stage1-20261022-211911-7d05dd0b` подтвердил
1010 cases ×2, native v2 run `stage2-20261022-212031-027fe280` — три MOV/memory
cases ×2 с full16/RSP/RIP, flags `0xcd7`, всей fixture memory и exact counts.
Расхождений нет, повторы стабильны, общий fingerprint
`bcade2e1c1282285f923e43e89dde2ffff7a70d900922022c3368f9aaf04cf5d`.
S2-09 принят в этих границах; полный CPU и этап 2 не приняты.
[Evidence](../reports/data/stage2/native-2026-10-22.json) фиксирует точные hashes,
times, hosts/build/limits и команды; [отчёт](../reports/STAGE_2_REPORT.md#s2-09--native-v2-и-физическая-проверка)
описывает результат. Предыдущий `39e0a6c8…` относится только к S2-05.

## S2-10: Linux и transfer oracle прошли

На том же Mac mini actual Linux workspace tests прошли, включая 39 od-isa,
54 od-testkit и native 9 library/23 binary tests. Legacy run
`stage1-20261022-213920-13e82953` подтвердил 1010 cases ×2; затем
`stage2-20261022-214049-874a0bf0` подтвердил восемь v2 cases ×2, включая
пять новых transfer программ. Full16/RSP/RIP, flags `0xcd7`, вся memory и
exact counts совпали, повторы стабильны. Общий fingerprint
`93ac5dfd316316cfc76f27d83fad993e3148ba6349ca77feede314da43e10d2c`;
[evidence](../reports/data/stage2/transfers-2026-10-22.json) фиксирует exact
hashes/times/build/limits и reproduction. S2-10 принят в
[описанных границах](CPU_ORACLE.md#s2-10-дополнительные-transfer-формы-подтверждены),
полный этап 2 и остальные CPU возможности этим не подтверждаются.

## S2-11: Linux arithmetic oracle прошёл

На том же Intel i5-3210M/kernel 6.17.0-41-generic прошли actual Linux workspace
tests (154) и run `stage2-20261022-215943-adeb1874`: 321 v2 cases ×2,
642 native snapshots, ноль различий и 321 стабильный repeat2. Full16/RIP,
RFLAGS mask `0xcd7`, memory/exception/count доступны. Linked legacy
`stage1-20261022-215743-751a2dd1` прошёл 1010 cases ×2 на том же fingerprint
`db01a6db7b28c41c9fd21761f308a3ef8c60a61a808530bc73ac36525bd391ba`.
На Mac прошли 135 workspace tests и Linux-target check; это отдельно от Linux
execution. [Evidence](../reports/data/stage2/arithmetic-2026-10-22.json)
не заявляет hardware guest #PF: no-commit проверен injected local faults,
native profile положительный. Полный этап 2 остаётся открытым.

## S2-12: logic oracle прошёл

На том же Intel i5-3210M/Linux6.17.0-41-generic прошли 167 workspace tests,
на Mac — 146. Run `stage2-20261022-221233-96c6116b` подтвердил 518 v2 cases
×2, включая 197 logic/TEST/NOT cases: 1036 snapshots, ноль различий,
518 стабильных repeat2. В обоих backend semantic masks распределены
684×`0xcd7` и 352×`0xcc7` (undefined AF после logic/TEST), NOT сохраняет mask.
Linked legacy `stage1-20261022-221121-5c3dfa43` прошёл 1010×2 на тех же
исходниках. [Evidence](../reports/data/stage2/logic-2026-10-22.json)
содержит exact fingerprint/hashes/times/build; injected local no-commit
не выдан за physical guest #PF. Полный этап 2 и ISSUE-011/012 остаются открытыми.

## Основной host

| Поле | Зафиксированное значение |
|---|---|
| Устройство | MacBook Pro (Mac15,11), Apple M3 Max, 14 CPU-ядер |
| Память | 36 GB |
| Host OS | macOS Tahoe 26.7.1, build 25G241 |
| Host page size | 16 KiB |
| Архитектура | arm64 |
| Rust | rustc 1.99.0 (b940084d7 2026-09-28); cargo 1.99.0 (5f94df478 2026-08-27) |
| Apple toolchain | Xcode 27.1 (27A9275), Apple Swift 6.4 (swiftlang-6.4.0.34.1), target arm64-apple-macosx26.0; macOS SDK 27.0 |
| Ad-hoc Hardened Runtime JIT smoke вне debugger | Подтверждён на основном M3 Max; VMHost подписан ad-hoc с `allow-jit`, подпись проверена, JIT исполнился |
| Developer ID / notarization / конечное приложение | `security find-identity -v -p codesigning` вернула `0 valid identities found`; Developer ID и notarization не проверены. Локальный `.app` smoke bundle проверен отдельно ниже, конечное приложение не собрано |

## Дополнительные машины

| Роль | Доступность | Необходимо инвентаризировать |
|---|---|---|
| Контрольный Apple Silicon Mac | MacBook Pro с Apple M1 Pro, MacBookPro18,3, 16 GB RAM | Проверен 22 октября: macOS 14.6.1 (23G93), arm64, host page size 16,384 bytes. Swift 5.10, Apple clang 15.0, Xcode developer directory доступен; `rustc` не установлен. Минимальный ad-hoc JIT smoke выполнен; source rebuild на M1 не выполнялся |
| Физический x86 oracle | Mac mini (Late 2012), 16 GB RAM; Ubuntu 25.10, Linux 6.17.0-41-generic, x86_64, page size 4096 bytes; Intel Core i5-3210M, family 6/model 58/stepping 9, microcode `0x21`, 2 cores/4 threads. SSH `senya@192.168.1.14` подтверждён с проверкой host key | GCC 15.2.0; Rust stable 1.99.1 установлен в пользовательский каталог 22 октября 2026 года. Checkout — `~/projects/open_desktop-stage1`. Linux workspace check/build/tests прошли. V1/S0 подтверждён этапом 1 и последующими регрессиями; в S2-09 дополнительно прошли три native v2 MOV/memory cases ×2 и legacy 1010 cases ×2 на тех же исходниках, см. [отчёт](../reports/STAGE_2_REPORT.md#s2-09--native-v2-и-физическая-проверка) |

Порядок подготовки этого Mac mini и физического сравнения описан в [плане этапа 1](../plans/STAGE_1.md). Адрес SSH записан со слов владельца 22 октября 2026 года; наличие адреса не подтверждает исполнение oracle-проверок.

Open Desktop пока не загружает гостевые ОС; Windows ISO подтверждён владельцем как рабочий. По прежнему порядку проверка M1 Pro следовала после контрольного Windows запуска в стороннем гипервизоре, но по прямому запросу владельца минимальный smoke выполнен 22 октября до этого запуска. Физический oracle Mac mini подтвердил ограниченный v1/S0 register-only профиль в [этапе 1](../reports/STAGE_1_REPORT.md) и [регрессии S2-05](../reports/STAGE_2_REPORT.md#s2-05--пошаговое-исполнение). Позднее [S2-09](../reports/STAGE_2_REPORT.md#s2-09--native-v2-и-физическая-проверка) отдельно подтвердил три положительные v2 MOV/memory программы ×2 и новую legacy регрессию 1010 cases ×2 на тех же исходниках. Это не подтверждает полную ISA или privileged faults. Не записывать серийные номера, hardware UUID, учётные данные и другие секреты.

## Гостевые образы и fixtures

- Windows 11 Pro x64 26H2 — установочный файл: `iso/Windows11_Client_x64_en-us_26300_9457.iso`, 9,047,330,816 bytes, bootable ISO 9660, label `CCCOMA_X64FRE_EN-US_DV9`. Локальный SHA-256: `bd4307df32bc8af33b39ccecb1174aeb345386630f89a2b86c7a4e36b55ea650`. В `sources/install.wim` найдено 11 образов; Windows 11 Pro — индекс 6, x86_64, en-US, version 10.0, base build 26300.9457. На 22 октября 2026 года Microsoft release information указывает cumulative update KB5127648/build 26300.9711 (13 октября); build фактически установленной VM нужно зафиксировать после запуска. WIM metadata прочитаны `wimlib-imagex info` после read-only подключения штатной командой `hdiutil attach -readonly -nobrowse ...`. По сведениям владельца, ISO получен из официального канала Microsoft; конкретный URL и дата загрузки не записаны. Файл исключён из Git правилом `.gitignore`.
- Предыдущая ошибка `hdiutil: attach failed - Device not configured` возникала при задании отдельного `-mountpoint`. Без него штатное подключение прошло успешно в `/Volumes/CCCOMA_X64FRE_EN-US_DV9`; ISO затем отключён. Это была ошибка способа подключения, а не повреждение ISO.
- Статическая инвентаризация `sources/install.wim`, индекс 6, выполнена `wimlib-imagex dir` и `wimlib-imagex extract`: `stornvme.inf` связывает `PCI\CC_010802` со службой `stornvme`, настроенной как `SERVICE_BOOT_START`; в `mshdc.inf` найден AHCI class ID `PCI\CC_010601`, в DriverStore есть `storahci.sys`; `nete1g3e.inf` содержит hardware ID `PCI\VEN_8086&DEV_100E` (Intel 82540EM-compatible). `vioscsi.inf`, `viostor.inf` и `netkvm.inf` в выбранном образе не найдены. Проверка метаданных и INF не подтверждает загрузку этих устройств в виртуальной машине.
- Дополнительно проверены входные драйверы в `install.wim` (Pro, index 6) и `boot.wim` (WinPE index 1 и Windows Setup index 2). Во всех трёх образах присутствуют `usbxhci.inf`, `usbhub3.inf` и `input.inf`; `usbxhci.inf` сопоставляет стандартный PCI class ID `PCI\CC_0C0330` со службой `USBXHCI`, а `input.inf` связывает generic USB HID class `03` со службой `hidusb`. Это даёт основание выбрать xHCI + USB HID как candidate input profile для платы. Статический осмотр WIM не подтверждает запуск контроллера, работу клавиатуры/мыши или наличие USB/HID пути в собственной UEFI.
- В ISO присутствуют `efi/boot/bootx64.efi`, `setup.exe`, `sources/boot.wim` и `sources/install.wim`. В `boot.wim` два образа: WinPE (индекс 1) и Windows Setup (индекс 2), оба x86_64, version 10.0, build 26100.9457; целевая Windows Pro в `install.wim` — build 26300.9457. Это различие записано как факт metadata; его влияние на установку ещё не проверено и дефектом не считается.
- Ubuntu Server 26.04.1 LTS amd64 — выбранный кандидат. Официальный SHA-256 из манифеста: `cc8a95cde20f6ced61a322420de00f10cc3c90ced545daa46cb9c1a117f1d927`; локальное наличие и хеш ещё не проверены.
- Сценарные приложения выбраны в [матрице поддержки](../specs/SUPPORT_MATRIX.md). Собственные входы для 7-Zip, Writer, Node.js и Blender добавлены в [`tests/fixtures/guest/stage0/`](../../tests/fixtures/guest/stage0/) с `SHA256SUMS`. Хеши fixtures проверены; Node workload на Node.js v26.12 на основном host возвращает ожидаемый JSON. Publisher reference SHA-256 для точных x64 installers записаны в матрице, но локальные package files ещё не получены/не сверены с ними. Прогоны в Windows VM, выходные артефакты и resource budgets не проверены.

Сырые измерения и локальные образы хранить в `work/`, а не в Git. В репозитории сохранять только малые собственные fixtures и агрегированные отчёты, если это соответствует правилам проекта.

## Состояние готовности

Описан основной host; ad-hoc Hardened Runtime smoke с JIT вне debugger подтверждён на M3 Max и минимально повторён на M1 Pro. На M3 Max fault в сгенерированном коде завершил дочерний процесс; отдельный protocol-level test helper подтвердил, что launcher сообщает `SIGSEGV` и завершается с ошибкой. Локальная `.app` smoke-упаковка с ad-hoc подписью проверена. Для четырёх CPU fixtures проведён повторный пилот S0-13 по 20 samples; сырые результаты и диапазоны измерений сохранены локально, сводка включена в [CPU baseline report](../reports/STAGE_0_CPU_BASELINE_2026-10-22.md). Стенд и протокол S0-06 на основном M3 Max готовы. Mac mini и x86 runner перенесены вместе с S0-12/E03; Developer ID/notarization и поставка конечного продукта остаются непроверенными.

Повторная проверка рабочей копии на основном host 22 октября 2026 года: `cargo fmt --all -- --check`, `cargo test --workspace --offline` (18 тестов), `cargo check --workspace --all-targets --offline`, `python3 tools/check_layout.py` и SHA-256 manifests x86/guest fixtures прошли. Повторная проверка относительных Markdown-путей в `docs/` прошла для 20 файлов: 0 отсутствующих целей.

## M1 Pro JIT smoke

22 октября 2026 года по SSH запущены три согласованные smoke-проверки на MacBook Pro с Apple M1 Pro (`MacBookPro18,3`): macOS 14.6.1, build 23G93, arm64, 16 GB RAM, page size 16,384 bytes. На M1 доступны Swift 5.10 / Apple clang 15.0 и Xcode developer directory `/Applications/Xcode.app/Contents/Developer`; Rust toolchain (`rustc`) отсутствует, репозитория на машине нет.

Поэтому проверялся переносимый release smoke, собранный локально для `arm64-apple-macosx14.0`: `od-host` static library с `MACOSX_DEPLOYMENT_TARGET=14.0`, Swift VMHost helper и launcher с `-target arm64-apple-macosx14.0`. Helper подписан ad-hoc с Hardened Runtime и `com.apple.security.cs.allow-jit`; на M1 `codesign --verify --strict` прошёл, entitlements прочитаны и включали `allow-jit`. Сверка SHA-256 локальных и удалённых файлов прошла.

| Файл | SHA-256 |
|---|---|
| `od-vmhost-m1-smoke` | `ea0844d620ec11bf0b39cd88e782daf7c13f0d4ef76d10d7589614c0ae2c102f` |
| `open-desktop-launcher-m1-smoke` | `6e879307a7b0995979773e5965a439229429e224a636ac068dad1b9d54341bc8` |
| `vmhost_isolation_smoke.py` | `56bf30dca3f6cbc404c43740aee23397aacc5c1b45bde2a3983b28e3f727d63d` |

Результаты:

- Arithmetic fragment `48 2b c1 75 fb`, `RAX=3`, `RCX=1`, budget 32: `RAX=0`, `RIP=5`, exit reason 0, 6 instructions.
- Self-loop `75 fe`, STOP через 25 ms: exit reason 2, вернулось 21,427,106 выполненных инструкций.
- `vmhost_isolation_smoke.py`: первый активный helper завершился от внешнего `SIGSEGV`; второй принял STOP и вернул собственное состояние.

Это подтверждает исполнение сгенерированного ARM64 в ad-hoc Hardened Runtime helper на M1 и узкий STOP/isolation сценарий. Это не независимая сборка из исходников на M1, не fault внутри generated code и не тест Developer ID, notarization или packaged application. Исходники остаются источником воспроизведения; переданные бинарники находились во временном каталоге.

## Fault внутри generated code на M3 Max

Добавлен unit test `generated_code_fault_is_confined_to_child_process` в `crates/od-host/src/lib.rs`. Тестовый дочерний процесс компилирует x86 `MOV RAX,1` собственным `od-jit`, исполняет generated ARM64 с намеренно неверным ненулевым указателем состояния и должен завершиться от `SIGSEGV`. Родительский тест проверяет именно сигнал завершения. На основном M3 Max `cargo test --workspace --offline` прошёл: 18 тестов, включая этот сценарий; `cargo fmt --all -- --check` и `cargo check --workspace --all-targets --offline` также прошли.

Проверка подтверждает fault именно при вызове сгенерированного кода и ограничение аварии дочерним процессом. Тест выполнялся на M3 Max; соответствующий бинарник не передавался на M1 Pro.

Сквозной supervisor smoke использует test-only helper [vmhost_generated_fault_helper.swift](../../tests/system/vmhost_generated_fault_helper.swift): он завершает HELLO/READY/RUN по тому же control protocol, генерирует в MAP_JIT `LDR X0,[X0]` + `RET` и исполняет его с адресом `1`. Сценарий [vmhost_generated_fault_smoke.py](../../tests/system/vmhost_generated_fault_smoke.py) подтвердил код выхода launcher `1` и сообщение `VMHost terminated by SIGSEGV`. Это намеренный host fault только в отдельном тестовом helper; production VMHost не изменён. Отдельный [launcher signal smoke](../../tests/system/vmhost_launcher_signal_smoke.py) также проверяет сигнализацию helper, завершённого во время активного цикла.

## Локальная `.app` smoke-упаковка

Сценарий [vmhost_app_bundle_smoke.sh](../../tests/system/vmhost_app_bundle_smoke.sh) создаёт временный `.app` bundle, копирует внутрь launcher и VMHost, ad-hoc подписывает вложенные исполняемые файлы и bundle с Hardened Runtime, проверяет `Info.plist` и подпись, затем запускает arithmetic JIT через бинарники из bundle. На M3 Max `plutil -lint` и `codesign --verify --strict --deep` прошли, fragment вернул `RAX=0`, `RIP=5`, 6 instructions. Каталог удаляется скриптом после smoke.

Это подтверждает только локальную ad-hoc `.app` упаковку smoke-инструментов. Это не конечное Open Desktop приложение, запуск через Launch Services, Developer ID distribution или notarization. Владелец сообщает, что у него есть Developer ID certificate, но на M3 Max не найдено ни действующей code-signing identity, ни Developer ID Application certificate в доступных keychains; на M1 Pro ранее найдено 0 Developer ID Application identities. Поэтому certificate+private key владельца пока не проверены на тестовых Mac. Членство Apple Developer Program не активно; [Apple указывает](https://developer.apple.com/help/account/access/resolving-access-issues), что при истёкшем членстве доступ к Apple notary service прекращается, поэтому notarization credentials не запрашивались и notarization не проверялась. Apple также сообщает, что уже подписанные приложения продолжают запускаться при истечении членства, а для получения нового Developer ID certificate после истечения самого сертификата требуется активное членство ([Developer ID certificates](https://developer.apple.com/help/account/certificates/create-developer-id-certificates)). Подписание нового bundle имеющейся парой certificate+private key на этом host ещё не проверено.

## Локальный smoke launcher ↔ VMHost

22 октября 2026 года на основном M3 Max release `od-host` был связан с Swift VMHost и launcher. Ad-hoc подпись VMHost с Hardened Runtime и `com.apple.security.cs.allow-jit` прошла `codesign --verify`. Повторены:

- Арифметический фрагмент `48 2b c1 75 fb`, `RAX=3`, `RCX=1`, бюджет 32: результат `RAX=0`, `RIP=5`, exit reason `0`, 6 инструкций.
- Бесконечный цикл `75 fe`, STOP через 25 ms: exit reason `2`, состояние вернулось launcher-у.
- Неизвестная версия control protocol: VMHost закрыл соединение и завершился с ошибкой.
- Разрыв socketpair во время бесконечного фрагмента: VMHost запросил cooperative stop и завершился, не оставив работающий worker.
- Два одновременно запущенных VMHost с отдельными socketpair: первый отверг неподдерживаемую версию, второй независимо выполнил RUN и вернул `RAX=1`.
- Через launcher проверены invalid code (`VMHost returned status 2`), несовместимая версия ответа и закрытие канала до RESULT; launcher вернул ошибку и завершил дочерний helper.
- Проверен helper, игнорирующий SIGTERM: launcher завершил его через SIGKILL после 500 ms grace period и собрал exit status.
- Межкомпонентный [isolation smoke](../../tests/system/vmhost_isolation_smoke.py): оба VMHost одновременно исполняли self-loop; после отправки `SIGSEGV` первому второй отдельно принял STOP и вернул exit reason `2`. Это проверка изоляции при fatal process signal, не fault внутри сгенерированного кода.

Воспроизводимые команды сборки и запуска находятся в [apps/vmhost/README.md](../../apps/vmhost/README.md); wire format — в [CONTROL_PROTOCOL_V0.md](../../protocols/control/CONTROL_PROTOCOL_V0.md). Эти проверки подтверждают локальный smoke path и базовую изоляцию процессов, но не полный GUI lifecycle, длительные параллельные VM с измерением JIT arena, Developer ID/нотаризацию или работу на другой модели Mac.

## Исправления control lifecycle по ревью S0

22 октября 2026 года исправлены три воспроизведённые гонки из [ревью S0](../reports/STAGE_0_REVIEW_2026-10-22.md):

- C ABI теперь сохраняет STOP, поступивший после RUN_ACK, но до начала worker execution; запрос сбрасывается после одного run. Rust unit test подтверждает ранний STOP и отсутствие переноса флага на следующий run.
- VMHost читает готовые байты control socket неблокирующе и декодирует полные кадры, продолжая ждать completion pipe при неполном заголовке или payload.
- Launcher ожидает одновременно срок отложенного STOP и входящие ответы. Если фрагмент завершился первым, RESULT принимается как итог; после STOP launcher принимает STOP_ACK и RESULT в согласованном порядке, а при гонке завершения RESULT остаётся финальным ответом.

Сценарий [vmhost_control_race_smoke.py](../../tests/system/vmhost_control_race_smoke.py) прошёл на основном M3 Max после release-сборки Rust и Swift binaries: 100/100 немедленных STOP вернули cooperative-stop reason; воспроизведение естественного завершения до 25 ms таймера вернуло RESULT с reason 0; 20 гонок с нулевой задержкой завершились с допустимым reason 0 или 2; неполные header и payload не задержали RESULT; закрытие канала после RUN_ACK остановило worker. Это smoke-регрессии control lifecycle, не benchmark.

## Инвентаризация S1-00

### Основной host: MacBook Pro с Apple M3 Max

Локальная read-only инвентаризация 22 октября 2026 года: `arm64`, macOS 26.7.1 (build 25G241), Darwin kernel 25.6.0, Apple M3 Max, 36 GiB RAM, размер host page 16,384 bytes. Установлены `rustc 1.99.0` и `cargo 1.99.0`. Подробные данные о модели и памяти не заменяют инвентаризацию физического x86 host.

### Физический x86 host: Mac mini с Ubuntu

Read-only инвентаризация по SSH с включённой проверкой host key выполнена 22 октября 2026 года для `senya@192.168.1.14`. Host key был добавлен в пользовательскую `known_hosts` из локальной консоли; после этого соединение прошло. На машине: Ubuntu 25.10, kernel `6.17.0-41-generic`, `x86_64`, Intel Core i5-3210M, family 6/model 58/stepping 9, microcode `0x21`, 4 логических CPU, page size 4096 bytes. Флаги CPU зафиксированы для этой конфигурации командой `lscpu`; требуемые формы S0-09 используют базовые 64-bit integer инструкции и не требуют AVX или других расширений.

Доступны `cc`/GCC 15.2.0 (`x86_64-linux-gnu`), GNU Make 4.4.1, Git 2.51.0 и Python 3.13.7. По разрешению владельца установлен Rust stable 1.99.1 в пользовательский каталог, checkout синхронизирован в отдельный `~/projects/open_desktop-stage1`. Необходимый native runner собран и выполнен без `sudo`; детали сравнения приведены в [CPU_ORACLE](CPU_ORACLE.md).

Инвентаризация и S1-06 подтверждают физический oracle для ограниченного register-only набора: 10 fixtures исполнены по два раза и совпали с M3 Max interpreter/JIT. Результат не распространяется на прочие модели x86, полную ISA, paging или privileged режим.

## Проверка S1-02 и S1-03 на основном host

22 октября 2026 года финальный `stage1_harness.py --suite all --generated-count 1000 --seed 0x5eedcafe --repeats 2` прошёл на M3 Max и Mac mini. Локальные format/check/test проверки, Linux Cargo tests и сравнение native oracle прошли; четыре положительных fixtures также исполнены через interpreter и ARM64 JIT. Подробные команды, окружения и fingerprints записаны в [отчёте Stage 1](../reports/STAGE_1_REPORT.md).

## Локальная проверка S1-04 supervisor

Создан `od-x86-runner` с watchdog-процессом, ограничением stdout/stderr, обязательным kill/wait, разбором normal exit и signal, а также Linux `setrlimit` для CPU, address space и core dump. На M3 Max прошли 8 supervisor tests; на Ubuntu Mac mini прошли 13 tests, включая фактическую проверку лимитов и завершение потомка, удерживавшего pipe. Ограничения и точные сведения о машинах указаны в [отчёте Stage 1](../reports/STAGE_1_REPORT.md).

## Локальные decode-boundary проверки S1-09

`od-isa` тесты проверяют усечения доступных форм без частичного изменения CPU state, empty input, maximum supported 10-byte MOV, unsupported 15-byte prefix sequence, memory ModR/M, branch target и invalid entry RIP; локальные 9 tests прошли. На Ubuntu Mac mini реальные decode проверки REX prefix и усечённого MOV immediate перед guard page вернули `Truncated` без host fault. Архитектурные границы и полный список проверок приведены в [отчёте Stage 1](../reports/STAGE_1_REPORT.md).

## Детерминированные control protocol сценарии S1-11

Обновлённый `vmhost_control_race_smoke.py` прошёл на M3 Max против уже собранных launcher и настоящего `od-vmhost-smoke`. Помимо существующих 100 early-stop и 20 stop/result races, три фиксированных seed проверили разбиение HELLO/RUN frames; также проверены partial header/payload, disconnect, bad magic/version/kind, oversized lengths и логически неправильный STOP с последующим успешным STOP. Сценарии и генератор зафиксированы в `tests/fixtures/control/control_v0_scenarios.json` и `docs/testing/CONTROL_PROTOCOL_TESTING.md`.
