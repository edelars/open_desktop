# Open Desktop

Собственная платформа для запуска x86-64 гостевых ОС на macOS/Apple Silicon. Декодер, интерпретатор, JIT ARM64, виртуальная машина и интеграция разрабатываются самостоятельно.

**Текущее состояние:** реализуются ограниченные вертикальные срезы CPU и системных компонентов Stage 2; это не означает готовность полной VM или поддержку полного x87. В частности, `od-isa` поддерживает bounded x87 compare/pop формы для canonical Normal binary80, включая `FCOM ST(i)`, `FCOMP ST(i)`, `FCOMPP`, `FCOMI` и `FCOMIP`, и сохраняет native x87 execution закрытым. Фактические границы и проверки описаны в [плане этапа 2](docs/plans/STAGE_2.md), [CPU contract](docs/specs/CPU_CONTRACT.md) и [отчёте](docs/reports/STAGE_2_REPORT.md).

## Начало работы

1. Прочитать [правила проекта](AGENTS.md) и [карту каталогов](docs/PROJECT_LAYOUT.md).
2. Ознакомиться с [архитектурой](docs/ARCHITECTURE.md) и [roadmap](docs/ROADMAP.md).
3. Выполнять [план этапа 0](docs/plans/STAGE_0.md) в пределах текущего поручения.

Проверки каркаса из корня проекта:

```sh
python3 tools/check_layout.py
cargo fmt --all -- --check
cargo check --workspace --all-targets --offline
```

Используются Cargo/Rust stable с edition 2024 и системные macOS SDK. Версии компилятора, SDK и macOS нужно записывать вместе с замерами. Точная фиксация toolchain и воспроизводимый app build входят в этап 0; Xcode project и подписанная VMHost-сборка ещё не созданы.

`work/` предназначен для промежуточных файлов, `outputs/` — для готовых deliverables. Долговечная документация хранится в `docs/`.
