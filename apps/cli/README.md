# odctl

Командная строка Open Desktop (задача S11-13 [плана этапа 11](../../docs/plans/STAGE_11.md)): создать машину, посмотреть её, прогнать жизненный цикл и удалить без окна приложения. Test-only: машины без гостевой ОС; машины с дисками не запускаются до S9-06 (`unsupported_durability`).

`odctl` не демон и не слушает endpoint: команда `run` сама запускает `od-vmhost`, ведёт его по control v2 и завершается вместе с ним. Подробности: [docs/specs/CLI.md](../../docs/specs/CLI.md) (команды, коды завершения, JSON, формат сценария), решения [ADR-0006](../../docs/decisions/ADR-0006-desktop-app-shell.md) (в) и (г).

## Сборка и запуск

```sh
cargo build -p od-host --offline      # из корня репозитория: статическая библиотека движка для od-vmhost
cd apps && swift build                # собирает od-vmhost и odctl; odctl не линкует libod_host.a
.build/debug/odctl --help
```

`odctl` ищет процесс машины в `--vmhost PATH`, затем в `$ODCTL_VMHOST`, затем рядом с собой (`.build/debug/od-vmhost`).

## Команды

Список совпадает с `odctl --help` и разделом 2 `docs/specs/CLI.md`.

| Команда | Что делает | Процесс машины |
|---|---|---|
| `odctl list [--json] [--library DIR]` | список машин библиотеки | нет |
| `odctl inspect <имя\|путь> [--json] [--library DIR]` | конфигурация и состояние одной машины | нет |
| `odctl delete <имя\|путь> --confirm <имя> [--files] [--json] [--library DIR]` | убрать остановленную машину из индекса; с `--files` удалить каталог | нет |
| `odctl create --name N --profile P --memory-mib M [--disk-gib G] [--library DIR] [--vmhost PATH] [--json]` | создать машину | только с `--disk-gib`, на время создания образа |
| `odctl run <имя\|путь> --script FILE [--json-events] [--library DIR] [--vmhost PATH]` | выполнить сценарий (`start`, `wait`, `pause`, `resume`, `sleep`, `query`, `stop`) | на время команды |

Коды завершения: 0 успех, 1 ошибка операции, 2 использование, 3 `busy` (машина занята приложением или другим `odctl`), 4 `unsupported_durability` (машина с диском: `run` со `start` или `stop`, `delete --files`; `run` без `start` на такой машине — код 0). Тексты ошибок — из каталога ошибок приложения; код — в `--json`.

## Пример

```sh
LIB=$(mktemp -d)
odctl create --name demo --profile display-test-pattern --memory-mib 256 --library "$LIB"
printf 'start\nwait running\npause\nwait paused\nresume\nwait running\nstop\n' > "$LIB/s.txt"
odctl run demo --script "$LIB/s.txt" --library "$LIB"
odctl inspect demo --json --library "$LIB"
odctl delete demo --confirm demo --files --library "$LIB"
```

С `--library DIR` каталог машин — `DIR`, индекс — `DIR/Library.plist`; без него — каталоги приложения (`~/Documents/Open Desktop/`, `~/Library/Application Support/OpenDesktop/`). Для проверок всегда задавайте `--library`.

## Проверка

```sh
python3 tests/system/odctl_lifecycle_smoke.py   # из корня репозитория, после swift build; печатает ok
```
