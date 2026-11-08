import Darwin
import Foundation

// odctl (S11-13, ADR-0006 (в) and (г)): a command-line tool on the same contracts as the application (control v2, bundle, catalog, error
// catalog of `ODShared`). It is not a daemon and listens to nothing: a command ends together with the VMHost it started. Test-only, no guest.

func execute(_ arguments: [String]) -> Int32 {
    var json = false
    var command = arguments.first ?? ""
    do {
        let outcome = try parseArguments(arguments)
        guard case .command(let parsed) = outcome else {
            Output.stdout(helpText)
            return ExitCode.success.rawValue
        }
        command = parsed.spec.name
        json = parsed.flag("--json") || parsed.flag("--json-events")
        switch parsed.spec.name {
        case "list": return try runList(parsed)
        case "inspect": return try runInspect(parsed)
        case "delete": return try runDelete(parsed)
        case "create": return try runCreate(parsed)
        default:
            let code = try runRun(parsed)
            if json { Output.json(["event": "result", "command": "run", "ok": true, "exit_code": Int(code)]) }
            return code
        }
    } catch let failure as CLIFailure {
        if command == "run", json {
            Output.json(["event": "result", "command": "run", "ok": false, "exit_code": Int(failure.exit.rawValue), "error": failure.jsonObject])
        } else {
            failure.report(json: json, command: command)
        }
        return failure.exit.rawValue
    } catch {
        Output.stderr("odctl: внутренняя ошибка: \(error)")
        return ExitCode.failure.rawValue
    }
}

exit(execute(Array(CommandLine.arguments.dropFirst())))
