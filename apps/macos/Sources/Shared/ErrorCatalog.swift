import Foundation

// The catalog of user-facing errors (S11-09, docs/specs/USER_FACING_ERRORS.md). One place for the words that the application and the
// CLI show for a code of control v2, an exit status or signal of the VMHost, a failure of the client, the library or a bundle.
//
// Rules of the specification (section 2 there):
//  - the title has no code, no digits and no path: codes and paths belong to the details;
//  - a text is a localization key plus a default value in Russian (`Localizable.strings` is the next step: the project has no
//    resources in SwiftPM targets yet, so the Russian values live here);
//  - a code that the catalog does not know is "Неизвестная ошибка" with details, never a guess;
//  - a signal that ended the VMHost is "Процесс машины аварийно завершился", never "an error of the guest";
//  - the action never says "повторите" where a repeat is not safe (`retrySafe == false`): the test of completeness checks the words.
//
// The codes are the enumerations of the protocol (`ControlV2ErrorCode`, `ControlV2AckCode`, `ControlClientExit.Disposition`): they are
// `CaseIterable`, and `ErrorCatalogTests` fails when a case has no entry. No stack trace and no `detail` of the VMHost goes through here.

public enum ErrorSeverity: String, CaseIterable, Equatable {
    /// Not a failure (an `ACK`, a normal end).
    case info
    /// The request was refused; nothing was lost, the machine keeps its state.
    case warning
    /// The operation failed; the user has to act.
    case error
    /// The process of a machine ended abnormally or its state is unknown: check the machine before using it.
    case critical
}

public struct ErrorEntry: Equatable {
    /// Prefix of the localization keys: `<key>.title`, `<key>.explanation`, `<key>.action`.
    public let key: String
    /// Default (Russian) values of the three keys. One or two sentences at most.
    public let title: String
    public let explanation: String
    public let action: String
    public let severity: ErrorSeverity
    /// Whether the user is offered "Подробности" (the code, the request ID, the log, the time).
    public let showsDetails: Bool
    /// Whether the action may tell the user to repeat the same operation. Where the state of the resources is unknown after the failure
    /// it is `false` and the action says "перезапустите приложение" or "проверьте машину".
    public let retrySafe: Bool

    public var titleKey: String { key + ".title" }
    public var explanationKey: String { key + ".explanation" }
    public var actionKey: String { key + ".action" }
}

/// Codes of `ACK` (CONTROL_PROTOCOL_V2.md 6.2, 12.1): not failures, but every code of the wire has a sentence.
public enum ControlV2AckCode: UInt32, CaseIterable {
    case ok = 0
    case inProgress = 1
}

/// Failures of the controlling side that are not a code of the VMHost: the process was not started, the client refused, the connection
/// ended. `ControlClientError` and the launch of `MachineController` map to these cases.
public enum ClientIssue: CaseIterable, Equatable {
    case vmhostMissing
    case spawnFailed
    case notReady
    case requestInFlight
    case connectionClosed
    /// The VMHost sent something that the protocol does not allow (the client closed the connection).
    case protocolViolation
    /// Request IDs are used up (section 5): a new process is needed.
    case requestIDsExhausted
    /// An error of the client itself (`io`, `invalidArgument`): a defect of the application, not of the machine.
    case internalFailure

    public init(_ error: ControlClientError) {
        switch error {
        case .notReady: self = .notReady
        case .requestInFlight: self = .requestInFlight
        case .connectionClosed: self = .connectionClosed
        case .protocolViolation: self = .protocolViolation
        case .requestIDsExhausted: self = .requestIDsExhausted
        // `refused` is told by the code of the VMHost (`entry(forErrorCode:)`); it only lands here when a caller has nothing better.
        case .io, .invalidArgument, .refused: self = .internalFailure
        }
    }
}

/// Failures and warnings of the library of machines and of a bundle (the catalog, `Library.plist`, `config.plist`, deletion).
public enum LibraryIssue: CaseIterable, Equatable {
    // The catalog (`CatalogError`, `CatalogWarning`).
    case notADirectory
    case notABundle
    case entryLimitReached
    case indexTooLarge
    case entryNotFound
    case indexWriteFailed
    case indexNotSaved
    case indexCorrupt
    case indexNewerVersion
    case scanLimitReached
    // The status of a bundle (`CatalogBundleStatus`).
    case bundleDirectoryMissing
    case bundleIncompatible
    case bundleInvalid
    // Deletion and the file system (`BundleRemovalError`).
    case removalNotStopped
    case removalUnsupportedDurability
    case removalInUse
    case permissionDenied
    case fileBusy
    case readOnlyVolume
    case noSpace
    case fileSystemError

    public init(_ error: CatalogError) {
        switch error {
        case .notADirectory: self = .notADirectory
        case .limitReached: self = .entryLimitReached
        case .indexTooLarge: self = .indexTooLarge
        case .notFound: self = .entryNotFound
        case .io: self = .indexWriteFailed
        }
    }

    public init(_ warning: CatalogWarning) {
        switch warning {
        case .indexCorrupt: self = .indexCorrupt
        case .indexNewerVersion: self = .indexNewerVersion
        case .limitReached: self = .scanLimitReached
        }
    }

    /// The issue of an `errno` of the file system; any other code is a plain "ошибка файловой системы".
    public init(errno code: Int32) {
        switch code {
        case EACCES, EPERM: self = .permissionDenied
        case EBUSY, ETXTBSY: self = .fileBusy
        case EROFS: self = .readOnlyVolume
        case ENOSPC: self = .noSpace
        default: self = .fileSystemError
        }
    }

    /// `nil` for a bundle that is readable (`valid`).
    public init?(_ status: CatalogBundleStatus) {
        switch status {
        case .directoryMissing: self = .bundleDirectoryMissing
        case .notADirectory: self = .notADirectory
        case .config(let state):
            switch state.kind {
            case .valid: return nil
            case .incompatible: self = .bundleIncompatible
            case .invalid: self = .bundleInvalid
            }
        }
    }
}

extension ControlV2ErrorCode {
    /// The name of the code in the specification (12.1), as it is written in the details and in the log.
    public var wireName: String {
        switch self {
        case .invalidState: return "invalid_state"
        case .invalidConfig: return "invalid_config"
        case .notConfigured: return "not_configured"
        case .unsupported: return "unsupported"
        case .unsupportedDurability: return "unsupported_durability"
        case .unsupportedNoAgent: return "unsupported_no_agent"
        case .busy: return "busy"
        case .lockChanged: return "lock_changed"
        case .lockInvalid: return "lock_invalid"
        case .pauseTimeout: return "pause_timeout"
        case .shutdownTimeout: return "shutdown_timeout"
        case .hostMemoryExceeded: return "host_memory_exceeded"
        case .diskImageUnavailable: return "disk_image_unavailable"
        case .invalidName: return "invalid_name"
        case .diskExists: return "disk_exists"
        case .insufficientSpace: return "insufficient_space"
        case .invalidResource: return "invalid_resource"
        case .resourceConflict: return "resource_conflict"
        case .resourceLimit: return "resource_limit"
        case .invalidArgument: return "invalid_argument"
        case .operationInProgress: return "operation_in_progress"
        case .requestIDConflict: return "request_id_conflict"
        case .requestIDStale: return "request_id_stale"
        case .ioError: return "io_error"
        case .hostFailure: return "host_failure"
        }
    }
}

public enum ErrorCatalog {
    // MARK: lookup

    /// Words for a code of `ERROR`. A code that is not in the closed list of 12.1 is the unknown error (6.7).
    public static func entry(forErrorCode raw: UInt32) -> ErrorEntry {
        guard let code = ControlV2ErrorCode(rawValue: raw) else { return unknown }
        return entry(for: code)
    }

    /// Words for a code of `ACK`; a code outside the list is the unknown error.
    public static func entry(forAckCode raw: UInt32) -> ErrorEntry {
        guard let code = ControlV2AckCode(rawValue: raw) else { return unknown }
        return entry(for: code)
    }

    /// What the end of a process says. A refused start is told by the code of the refusal, not by the exit status 3.
    public static func entry(for exit: ControlClientExit) -> ErrorEntry {
        if let rejection = exit.startRejection { return entry(for: rejection) }
        return entry(for: exit.disposition)
    }

    /// The VMHost answered `ERROR` instead of `READY` (section 4 item 2 of the protocol): the words of that code.
    public static func entry(for rejection: StartRejection) -> ErrorEntry {
        switch rejection {
        case .busy: return entry(for: ControlV2ErrorCode.busy)
        case .lockChanged: return entry(for: ControlV2ErrorCode.lockChanged)
        case .lockInvalid: return entry(for: ControlV2ErrorCode.lockInvalid)
        case .ioError: return entry(for: ControlV2ErrorCode.ioError)
        case .other(let code): return entry(forErrorCode: code)
        }
    }

    /// A refusal that the controlling process applies before it spawns a VMHost (VM_BUNDLE 3.5): the words of the same code.
    public static func entry(for refusal: LaunchRefusal) -> ErrorEntry {
        switch refusal {
        case .unsupportedDurability: return entry(for: ControlV2ErrorCode.unsupportedDurability)
        case .unsupported: return entry(for: ControlV2ErrorCode.unsupported)
        case .hostMemoryExceeded: return entry(for: ControlV2ErrorCode.hostMemoryExceeded)
        }
    }

    /// The entry for a failure or a warning of the library.
    public static func entry(for issue: LibraryIssue) -> ErrorEntry { library(issue) }

    // MARK: the unknown error

    public static let unknown = ErrorEntry(
        key: "error.unknown",
        title: "Неизвестная ошибка",
        explanation: "Процесс машины сообщил об ошибке, которой нет в этой версии приложения.",
        action: "Сохраните подробности и сообщите о проблеме. Если машина не отвечает, перезапустите приложение и проверьте машину.",
        severity: .error, showsDetails: true, retrySafe: false)

    // MARK: ERROR codes (CONTROL_PROTOCOL_V2.md 12.1)

    public static func entry(for code: ControlV2ErrorCode) -> ErrorEntry {
        let key = "error.control." + code.wireName
        func make(_ title: String, _ explanation: String, _ action: String, _ severity: ErrorSeverity, details: Bool = true, retry: Bool = false) -> ErrorEntry {
            ErrorEntry(key: key, title: title, explanation: explanation, action: action, severity: severity, showsDetails: details, retrySafe: retry)
        }
        switch code {
        case .invalidState:
            return make("Действие сейчас недоступно",
                        "Машина находится в состоянии, в котором эта операция не выполняется.",
                        "Дождитесь окончания текущей операции или обновите состояние и выберите доступное действие.", .warning)
        case .invalidConfig:
            return make("Настройки машины не приняты",
                        "Процесс машины отверг одно из значений конфигурации. Машина не запущена.",
                        "Проверьте значения в конфигурации машины и исправьте их.", .error)
        case .notConfigured:
            return make("Машина не настроена",
                        "Процесс машины получил команду раньше, чем настройки. Это ошибка приложения, а не машины.",
                        "Перезапустите приложение и сообщите об ошибке, приложив подробности.", .error)
        case .unsupported:
            return make("Эта возможность пока не поддерживается",
                        "Конфигурация требует устройства или функции, которых нет у выбранного профиля машины в этой версии.",
                        "Уберите это устройство из конфигурации (сеть, звук, общие папки, буфер обмена, загрузочный носитель) или выберите другой профиль.",
                        .warning)
        case .unsupportedDurability:
            return make("Работа с дисками пока недоступна",
                        "В этой версии нельзя гарантировать сохранность данных на дисках машины, поэтому машина с дисками не запускается и не останавливается запросом. Это не временная ошибка.",
                        "Для проверки используйте машину без дисков. Ограничение снимется, когда надёжность хранения данных будет подтверждена.",
                        .warning)
        case .unsupportedNoAgent:
            return make("Агент в гостевой системе не найден",
                        "Для корректного завершения нужен агент внутри гостевой системы, а он не отвечает или не установлен. Машина продолжает работать.",
                        "Завершите работу изнутри гостевой системы или остановите машину принудительно: несохранённые данные гостевой системы при этом могут быть потеряны.",
                        .warning)
        case .busy:
            return make("Машина уже запущена другим процессом",
                        "Файлы машины заняты: её запустил другой процесс, либо блокировка была занята на мгновение.",
                        "Закройте другое окно или процесс с этой машиной. Если других запусков нет, запустите машину ещё раз.",
                        .warning, retry: true)
        case .lockChanged:
            return make("Файл блокировки машины изменился",
                        "Файл блокировки был заменён или удалён во время запуска, поэтому процесс не стал запускать машину.",
                        "Убедитесь, что другая программа не работает с каталогом машины, и запустите машину ещё раз.",
                        .error, retry: true)
        case .lockInvalid:
            return make("Каталог машины повреждён",
                        "Файл блокировки в каталоге машины не является обычным файлом.",
                        "Сохраните копию каталога машины и замените файл блокировки пустым обычным файлом. Пока этого не сделано, машина не запустится.",
                        .error)
        case .pauseTimeout:
            return make("Машина не успела приостановиться",
                        "Машина не дошла до безопасной точки за отведённое время и продолжает работать.",
                        "Подождите немного и нажмите «Приостановить» ещё раз или остановите машину.",
                        .warning, retry: true)
        case .shutdownTimeout:
            return make("Гостевая система не завершила работу",
                        "Гостевая система не подтвердила завершение за отведённое время. Машина продолжает работать; это не успешное завершение.",
                        "Подождите, завершите работу изнутри гостевой системы или остановите машину принудительно.",
                        .warning)
        case .hostMemoryExceeded:
            return make("Машине выделено слишком много памяти",
                        "Объём памяти машины больше половины памяти компьютера.",
                        "Уменьшите объём памяти машины в конфигурации.", .error)
        case .diskImageUnavailable:
            return make("Образ диска не найден",
                        "Файл образа диска отсутствует или не является обычным файлом.",
                        "Проверьте, что файл образа на месте и не заменён ссылкой, либо восстановите его из копии.", .error)
        case .invalidName:
            return make("Недопустимое имя диска",
                        "Имя образа нарушает правила: оно пустое, слишком длинное или содержит недопустимые символы.",
                        "Выберите другое имя.", .error)
        case .diskExists:
            return make("Диск с таким именем уже есть",
                        "Образ с этим именем уже существует; существующий файл не изменён.",
                        "Выберите другое имя или удалите старый образ вручную, если он больше не нужен.", .warning)
        case .insufficientSpace:
            return make("Не хватает места на диске",
                        "На томе не хватает свободного места для образа диска.",
                        "Освободите место на томе или уменьшите ёмкость диска и повторите действие.", .error, retry: true)
        case .invalidResource:
            return make("Файл не подходит для машины",
                        "Процесс машины отверг переданный файл или папку: не подходят роль, тип или режим доступа.",
                        "Выберите подходящий файл и повторите действие.", .error, retry: true)
        case .resourceConflict:
            return make("Файл уже подключён к машине",
                        "Это место в машине уже занято другим файлом. Это ошибка приложения, а не машины.",
                        "Перезапустите приложение и сообщите об ошибке, приложив подробности.", .error)
        case .resourceLimit:
            return make("Подключено слишком много файлов",
                        "Превышено число файлов, которые процесс машины может удерживать одновременно.",
                        "Остановите машину и запустите её заново: так подключённые файлы освободятся.", .warning)
        case .invalidArgument:
            return make("Недопустимое значение",
                        "Приложение передало процессу машины значение вне допустимого диапазона.",
                        "Исправьте значение. Если вы его не вводили, перезапустите приложение и сообщите об ошибке.", .error)
        case .operationInProgress:
            return make("Предыдущая операция ещё выполняется",
                        "Процесс машины обрабатывает другой запрос и не принимает новый.",
                        "Дождитесь завершения текущей операции и повторите действие.", .warning, details: false, retry: true)
        case .requestIDConflict:
            return make("Сбой обмена с процессом машины",
                        "Приложение и процесс машины рассинхронизировались: один и тот же запрос получил разный смысл. Это ошибка приложения, а не машины.",
                        "Перезапустите приложение и сообщите об ошибке, приложив подробности.", .error)
        case .requestIDStale:
            return make("Устаревший запрос к процессу машины",
                        "Ответ на запрос уже нельзя восстановить: он вышел за пределы хранимых. Это ошибка приложения, а не машины.",
                        "Перезапустите приложение и сообщите об ошибке, приложив подробности.", .error)
        case .ioError:
            return make("Ошибка работы с файлами машины",
                        "Операционная система вернула ошибку при работе с файлами машины; состояние этих файлов может быть неизвестно.",
                        "Проверьте права доступа, свободное место и целостность каталога машины перед следующим запуском.", .error)
        case .hostFailure:
            return make("Сбой процесса машины",
                        "Процесс машины не может установить состояние ресурсов машины и завершается. Это не ошибка гостевой системы.",
                        "Перезапустите приложение и проверьте машину перед следующим запуском; сохраните подробности для отчёта.", .critical)
        }
    }

    // MARK: ACK codes (6.2)

    public static func entry(for code: ControlV2AckCode) -> ErrorEntry {
        switch code {
        case .ok:
            return ErrorEntry(key: "ack.ok", title: "Готово", explanation: "Запрос выполнен.", action: "Действий не требуется.",
                              severity: .info, showsDetails: false, retrySafe: false)
        case .inProgress:
            return ErrorEntry(key: "ack.in_progress", title: "Операция выполняется",
                              explanation: "Процесс машины уже получил этот запрос и выполняет его.",
                              action: "Дождитесь результата.", severity: .info, showsDetails: false, retrySafe: false)
        }
    }

    // MARK: the end of the VMHost process (12.3 and signals)

    public static func entry(for disposition: ControlClientExit.Disposition) -> ErrorEntry {
        func make(_ key: String, _ title: String, _ explanation: String, _ action: String, _ severity: ErrorSeverity, details: Bool = true, retry: Bool = false) -> ErrorEntry {
            ErrorEntry(key: "exit." + key, title: title, explanation: explanation, action: action, severity: severity, showsDetails: details, retrySafe: retry)
        }
        switch disposition {
        case .stopped:
            return make("stopped", "Машина остановлена", "Процесс машины завершился штатно.", "Действий не требуется.", .info, details: false)
        case .usageError:
            return make("usage_error", "Процесс машины не запустился",
                        "Процесс получил неверные аргументы запуска. Это ошибка приложения, а не машины.",
                        "Перезапустите приложение. Если ошибка остаётся, сообщите о ней, приложив подробности.", .error)
        case .startRejected:
            return make("start_rejected", "Машину не удалось запустить",
                        "Процесс машины отказался запускаться; причина указана в подробностях.",
                        "Устраните причину из подробностей и запустите машину ещё раз.", .error, retry: true)
        case .protocolError:
            return make("protocol_error", "Нарушен обмен с процессом машины",
                        "Приложение и процесс машины получили данные не по протоколу и закрыли соединение. Это не ошибка гостевой системы.",
                        "Перезапустите приложение и проверьте машину перед следующим запуском; сообщите об ошибке, приложив подробности.", .error)
        case .transportLost:
            return make("transport_lost", "Связь с процессом машины потеряна",
                        "Соединение между приложением и процессом машины оборвалось, процесс завершился. Несохранённые данные гостевой системы могли быть потеряны.",
                        "Проверьте машину перед следующим запуском.", .error)
        case .hostFailed:
            return make("host_failed", "Сбой процесса машины",
                        "Процесс машины не смог установить состояние ресурсов машины и завершился. Это не ошибка гостевой системы.",
                        "Перезапустите приложение и проверьте машину перед следующим запуском; сохраните подробности для отчёта.", .critical)
        case .killedBySignal:
            return make("signal", "Процесс машины аварийно завершился",
                        "Процесс был прерван сигналом операционной системы или другой программой. Это не ошибка гостевой системы; несохранённые данные гостевой системы могли быть потеряны.",
                        "Проверьте машину перед следующим запуском; подробности помогут найти причину.", .critical)
        case .unexpected:
            return make("unexpected", "Процесс машины завершился неожиданно",
                        "Процесс завершился с кодом, которого нет в описании протокола.",
                        "Проверьте машину перед следующим запуском и сообщите об ошибке, приложив подробности.", .error)
        case .unavailable:
            return make("unavailable", "Состояние процесса машины неизвестно",
                        "У приложения нет процесса, статус завершения которого можно узнать.",
                        "Перезапустите приложение и проверьте машину.", .error)
        }
    }

    // MARK: client

    public static func entry(for issue: ClientIssue) -> ErrorEntry {
        func make(_ key: String, _ title: String, _ explanation: String, _ action: String, _ severity: ErrorSeverity, details: Bool = true, retry: Bool = false) -> ErrorEntry {
            ErrorEntry(key: "client." + key, title: title, explanation: explanation, action: action, severity: severity, showsDetails: details, retrySafe: retry)
        }
        switch issue {
        case .vmhostMissing:
            return make("vmhost_missing", "Процесс машины не найден",
                        "В составе приложения нет исполняемого файла процесса машины, поэтому машина не запускалась.",
                        "Переустановите или обновите приложение.", .error)
        case .spawnFailed:
            return make("spawn_failed", "Не получилось создать процесс машины",
                        "Операционная система не смогла создать процесс машины; машина не запускалась.",
                        "Проверьте права и свободные ресурсы системы и запустите машину ещё раз.", .error, retry: true)
        case .notReady:
            return make("not_ready", "Процесс машины ещё не готов",
                        "Процесс машины ещё не сообщил о готовности принимать команды.",
                        "Подождите и повторите действие.", .warning, details: false, retry: true)
        case .requestInFlight:
            return make("request_in_flight", "Предыдущая операция ещё выполняется",
                        "Приложение ждёт ответа на предыдущую команду и не отправляет новую.",
                        "Дождитесь завершения текущей операции и повторите действие.", .warning, details: false, retry: true)
        case .connectionClosed:
            return make("connection_closed", "Связь с процессом машины закрыта",
                        "Соединение с процессом машины закрыто; причину покажет статус его завершения.",
                        "Проверьте состояние машины в списке.", .warning)
        case .protocolViolation:
            return make("protocol_violation", "Нарушен обмен с процессом машины",
                        "Процесс машины прислал данные не по протоколу, соединение закрыто. Это не ошибка гостевой системы.",
                        "Перезапустите приложение и проверьте машину перед следующим запуском; сообщите об ошибке, приложив подробности.", .error)
        case .requestIDsExhausted:
            return make("request_ids_exhausted", "Исчерпан счётчик запросов",
                        "К этому процессу машины отправлено максимально возможное число запросов.",
                        "Остановите машину и запустите её заново.", .warning)
        case .internalFailure:
            return make("internal", "Внутренняя ошибка приложения",
                        "Приложение не смогло выполнить обмен с процессом машины. Это не ошибка гостевой системы.",
                        "Перезапустите приложение и проверьте машину перед следующим запуском; сообщите об ошибке, приложив подробности.", .error)
        }
    }

    // MARK: library and bundle

    private static func library(_ issue: LibraryIssue) -> ErrorEntry {
        func make(_ key: String, _ title: String, _ explanation: String, _ action: String, _ severity: ErrorSeverity, details: Bool = true, retry: Bool = false) -> ErrorEntry {
            ErrorEntry(key: "library." + key, title: title, explanation: explanation, action: action, severity: severity, showsDetails: details, retrySafe: retry)
        }
        switch issue {
        case .notADirectory:
            return make("not_a_directory", "Это не каталог",
                        "Выбранный путь не является каталогом (или это ссылка на каталог).",
                        "Выберите каталог машины.", .warning)
        case .notABundle:
            return make("not_a_bundle", "Каталог не похож на машину",
                        "В каталоге нет суффикса машины или обычного файла конфигурации. Приложение ничего в нём не меняет.",
                        "Выберите каталог машины, созданный этим приложением.", .warning)
        case .entryLimitReached:
            return make("entry_limit_reached", "В списке слишком много машин",
                        "В списке уже максимальное число машин (1000).",
                        "Уберите ненужные машины из списка.", .warning, details: false)
        case .indexTooLarge:
            return make("index_too_large", "Список машин слишком велик",
                        "Список машин занял бы больше допустимого размера, поэтому он не записан.",
                        "Уберите ненужные машины из списка.", .warning, details: false)
        case .entryNotFound:
            return make("entry_not_found", "Записи нет в списке",
                        "Этой машины уже нет в списке; менять ничего не нужно.",
                        "Обновите список.", .info)
        case .indexWriteFailed:
            return make("index_write_failed", "Не удалось записать список машин",
                        "Файл списка машин не записан. Каталоги машин не затронуты.",
                        "Проверьте права доступа и свободное место на диске; при следующем запуске список будет собран заново по каталогу машин.", .warning)
        case .indexNotSaved:
            return make("index_not_saved", "Список машин не сохранён",
                        "Машина показана только до закрытия приложения.",
                        "Проверьте права доступа и свободное место на диске, затем откройте машину заново.", .warning)
        case .indexCorrupt:
            return make("index_corrupt", "Список машин был повреждён",
                        "Он собран заново по каталогу машин; копия повреждённого файла сохранена рядом.",
                        "Проверьте, что все нужные машины на месте; недостающие откройте вручную.", .warning)
        case .indexNewerVersion:
            return make("index_newer_version", "Список машин записан более новой версией приложения",
                        "Изменения в списке в этой версии не сохраняются.",
                        "Обновите приложение.", .warning, details: false)
        case .scanLimitReached:
            return make("scan_limit_reached", "Показаны не все машины",
                        "В каталоге больше машин, чем приложение может показать (1000): показаны первые.",
                        "Уберите ненужные машины из каталога.", .warning, details: false)
        case .bundleDirectoryMissing:
            return make("bundle_directory_missing", "Каталог машины не найден",
                        "Каталога, на который ссылается список, больше нет на диске.",
                        "Верните каталог на место или уберите машину из списка.", .warning, details: false)
        case .bundleIncompatible:
            return make("bundle_incompatible", "Машина несовместима с этой версией",
                        "Конфигурация записана более новой версией приложения или содержит неизвестное значение. Файл не изменён.",
                        "Обновите приложение. Машину можно убрать из списка.", .warning)
        case .bundleInvalid:
            return make("bundle_invalid", "Конфигурация машины повреждена",
                        "Файл конфигурации отсутствует или не соответствует правилам. Он не изменён и не исправляется автоматически.",
                        "Восстановите конфигурацию из копии или уберите машину из списка.", .error)
        case .removalNotStopped:
            return make("removal_not_stopped", "Машина не остановлена",
                        "Удалять можно только остановленную машину, у которой нет процесса.",
                        "Остановите машину и затем удалите её.", .warning, details: false)
        case .removalUnsupportedDurability:
            return make("removal_unsupported_durability", "Файлы машины с дисками не удаляются",
                        "В этой версии нельзя гарантировать сохранность данных на дисках, поэтому файлы такой машины не удаляются. Это не временная ошибка.",
                        "Уберите машину из списка без удаления файлов; каталог можно удалить вручную.", .warning)
        case .removalInUse:
            return make("removal_in_use", "Машина запущена другим процессом",
                        "Файлы машины заняты другим процессом, поэтому они не удалены.",
                        "Остановите машину в другом окне и затем удалите её.", .warning, retry: true)
        case .permissionDenied:
            return make("permission_denied", "Нет прав на изменение файлов машины",
                        "Операционная система не разрешила изменить файлы машины.",
                        "Проверьте права доступа к каталогу машины.", .error)
        case .fileBusy:
            return make("file_busy", "Файл занят другой программой",
                        "Файл машины сейчас использует другая программа.",
                        "Закройте программу, которая использует файл, и повторите действие.", .warning, retry: true)
        case .readOnlyVolume:
            return make("read_only_volume", "Диск доступен только для чтения",
                        "Том, на котором лежат файлы машины, не позволяет их менять.",
                        "Используйте том, доступный для записи.", .error)
        case .noSpace:
            return make("no_space", "На диске нет места",
                        "На томе не хватает свободного места для записи.",
                        "Освободите место на томе и повторите действие.", .error, retry: true)
        case .fileSystemError:
            return make("file_system_error", "Ошибка файловой системы",
                        "Операционная система вернула ошибку при работе с файлами; причина указана в подробностях.",
                        "Проверьте каталог машины и права доступа к нему.", .error)
        }
    }
}
