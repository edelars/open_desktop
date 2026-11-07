import Foundation

// Command line parsing of `odctl` (S11-13). No external package: a command has a fixed set of options with a value and of flags;
// anything else is a usage failure (exit 2).

struct CommandSpec {
    var name: String
    /// What `--help` prints for the command: the synopsis line and one sentence.
    var synopsis: String
    var summary: String
    /// Options that take a value (`--name N`).
    var valueOptions: Set<String>
    /// Options without a value (`--json`).
    var flagOptions: Set<String>
    /// Number of positional arguments (`<имя|путь>`).
    var positionalCount: Int
}

let commandSpecs: [CommandSpec] = [
    CommandSpec(
        name: "list", synopsis: "list [--json] [--library DIR]",
        summary: "Показать машины библиотеки. Процесс машины не запускается.",
        valueOptions: ["--library"], flagOptions: ["--json"], positionalCount: 0),
    CommandSpec(
        name: "inspect", synopsis: "inspect <имя|путь> [--json] [--library DIR]",
        summary: "Показать конфигурацию и состояние каталога одной машины. Процесс машины не запускается.",
        valueOptions: ["--library"], flagOptions: ["--json"], positionalCount: 1),
    CommandSpec(
        name: "delete", synopsis: "delete <имя|путь> --confirm <имя> [--files] [--json] [--library DIR]",
        summary: "Убрать остановленную машину из индекса; с --files удалить и каталог машины. Процесс машины не запускается.",
        valueOptions: ["--confirm", "--library"], flagOptions: ["--files", "--json"], positionalCount: 1),
    CommandSpec(
        name: "create",
        synopsis: "create --name N --profile P --memory-mib M [--disk-gib G] [--library DIR] [--vmhost PATH] [--json]",
        summary: "Создать машину. Без --disk-gib процесс машины не запускается; с ним запускается на время создания образа.",
        valueOptions: ["--name", "--profile", "--memory-mib", "--disk-gib", "--library", "--vmhost"], flagOptions: ["--json"],
        positionalCount: 0),
    CommandSpec(
        name: "run", synopsis: "run <имя|путь> --script FILE [--json-events] [--library DIR] [--vmhost PATH]",
        summary: "Запустить процесс машины, выполнить сценарий и завершиться вместе с процессом.",
        valueOptions: ["--script", "--library", "--vmhost"], flagOptions: ["--json-events"], positionalCount: 1),
]

let helpText: String = {
    var lines = [
        "odctl — управление машинами Open Desktop без окна (test-only, без гостя).",
        "",
        "Использование: odctl <команда> [параметры]",
        "",
        "Команды:",
    ]
    for spec in commandSpecs {
        lines.append("  odctl " + spec.synopsis)
        lines.append("      " + spec.summary)
    }
    lines += [
        "",
        "Общие параметры:",
        "  --library DIR   каталог библиотеки вместо ~/Documents/Open Desktop/ (индекс — DIR/Library.plist)",
        "  --vmhost PATH   процесс машины od-vmhost (по умолчанию — рядом с odctl или $ODCTL_VMHOST)",
        "  --json          результат одним JSON-объектом (\"schema\": 1); кроме run: у него --json-events (по объекту на строку)",
        "  --help          эта справка",
        "",
        "Коды завершения: 0 успех; 1 ошибка операции; 2 ошибка использования; 3 busy (машина занята); 4 unsupported_durability.",
        "Описание сценария и формата вывода: docs/specs/CLI.md.",
    ]
    return lines.joined(separator: "\n")
}()

struct ParsedArguments {
    var spec: CommandSpec
    var positional: [String] = []
    var values: [String: String] = [:]
    var flags: Set<String> = []

    func value(_ option: String) -> String? { values[option] }
    func flag(_ option: String) -> Bool { flags.contains(option) }
}

enum ParseOutcome {
    case help
    case command(ParsedArguments)
}

func parseArguments(_ arguments: [String]) throws -> ParseOutcome {
    guard let first = arguments.first else { throw CLIFailure.usage("Не указана команда") }
    if first == "--help" || first == "-h" || first == "help" { return .help }
    guard let spec = commandSpecs.first(where: { $0.name == first }) else {
        throw CLIFailure.usage("Неизвестная команда «\(first)»", details: ["Команды: " + commandSpecs.map(\.name).joined(separator: ", ")])
    }
    var parsed = ParsedArguments(spec: spec)
    var index = 1
    while index < arguments.count {
        let argument = arguments[index]
        index += 1
        if argument == "--help" || argument == "-h" { return .help }
        if argument.hasPrefix("--") {
            if spec.flagOptions.contains(argument) {
                parsed.flags.insert(argument)
            } else if spec.valueOptions.contains(argument) {
                guard index < arguments.count else { throw CLIFailure.usage("У параметра \(argument) нет значения") }
                guard parsed.values[argument] == nil else { throw CLIFailure.usage("Параметр \(argument) указан дважды") }
                parsed.values[argument] = arguments[index]
                index += 1
            } else {
                throw CLIFailure.usage("Команда \(spec.name) не принимает параметр \(argument)")
            }
        } else {
            parsed.positional.append(argument)
        }
    }
    if parsed.positional.count != spec.positionalCount {
        let expected = spec.positionalCount == 0 ? "не принимает позиционных аргументов" : "ожидает <имя|путь>"
        throw CLIFailure.usage("Команда \(spec.name) \(expected)")
    }
    return .command(parsed)
}

/// A required option of a command.
func require(_ parsed: ParsedArguments, _ option: String) throws -> String {
    guard let value = parsed.value(option) else { throw CLIFailure.usage("Не указан обязательный параметр \(option)") }
    return value
}

/// A decimal integer in `range`; anything else is a usage failure.
func integerOption(_ text: String, _ option: String, range: ClosedRange<Int>) throws -> Int {
    guard let value = Int(text), text.allSatisfy({ $0.isASCII && $0.isNumber }) else {
        throw CLIFailure.usage("Параметр \(option): ожидается целое число, получено «\(text)»")
    }
    guard range.contains(value) else {
        throw CLIFailure.usage("Параметр \(option): значение \(value) вне допустимого диапазона от \(range.lowerBound) до \(range.upperBound)")
    }
    return value
}
