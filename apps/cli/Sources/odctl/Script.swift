import Foundation
import ODShared

// The script of `odctl run` (S11-13, docs/specs/CLI.md section 6). The grammar (`parseScript`, `ScriptLine`, the names of the states) is in
// `ODShared/OdctlRules.swift` where the unit tests reach it; this file reads the file and turns the mistakes into exit 2.

/// A failure of the text of a script: a usage failure (exit 2) with the number of the line.
func scriptFailure(_ error: ScriptError) -> CLIFailure { CLIFailure.usage(error.text) }

func readScript(path: String) throws -> [ScriptLine] {
    var info = stat()
    guard stat(path, &info) == 0, (info.st_mode & S_IFMT) == S_IFREG else {
        throw CLIFailure.usage("Файл сценария не найден или не является обычным файлом", details: ["Путь: \(path)"])
    }
    guard info.st_size <= ScriptLimits.maxFileBytes else {
        throw CLIFailure.usage("Файл сценария больше \(ScriptLimits.maxFileBytes / 1024) КиБ", details: ["Путь: \(path)"])
    }
    guard let data = FileManager.default.contents(atPath: path), let text = String(data: data, encoding: .utf8) else {
        throw CLIFailure.usage("Не удалось прочитать файл сценария как UTF-8", details: ["Путь: \(path)"])
    }
    do {
        return try parseScript(text)
    } catch let error as ScriptError {
        throw scriptFailure(error)
    }
}
