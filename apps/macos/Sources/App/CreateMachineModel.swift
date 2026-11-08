import Combine
import Darwin
import Foundation
import ODShared

// Model of the "Create machine" wizard (S11-08a, docs/plans/STAGE_11.md section 7): the steps, their checks, the limits and the
// cancellation. No UI framework and no side effect: this file creates no directory, no file and no process; `confirm()` returns the
// plan that the window slice (S11-08b) executes. Main queue only.
//
// Steps: name, profile, memory, disk, summary. The step "boot media" of the plan is NOT here: an ISO needs `NSOpenPanel` and a bookmark
// (VM_BUNDLE section 4), and a path stub is not allowed; a machine with `boot_media` answers `unsupported` on START of a test-only
// profile anyway (CONTROL_PROTOCOL_V2.md 7.3). It arrives with the window slice or later; the discrepancy is recorded in the S11-08a report section (docs/reports/STAGE_11_REPORT.md).
//
// Rules and where they come from:
//  - machine name: `name` of `config.plist`, VM_BUNDLE 3.2: 1...128 bytes of UTF-8, no control characters (U+0000-U+001F, U+007F),
//    not only spaces. `/` and `:` are allowed in the name: the directory name replaces them by `_` (VM_BUNDLE 2 item 4). The limit of
//    249 bytes belongs to the disk image name (3.3), not to the machine name.
//  - the bundle directory is `<name derived by VM_BUNDLE 2 item 4>.odvm` in the default directory. A directory that exists is an error
//    of the step (the plan), although VM_BUNDLE 2 item 4 would add a numeric suffix: recorded in the S11-08a report section.
//  - memory: `BundleConfigLimits` 256...65536 MiB and at most half of `hw.memsize` (VM_BUNDLE 3.5 item 4, the same arithmetic as
//    `BundleConfig.launchRefusal`). A value outside is an error of the step; it is never corrected silently.
//  - disk: RAW, a multiple of 512 bytes, not zero, not above Int64.max (STORAGE_HOST_ABI_V1.md). The image is created by VMHost
//    (`PREPARE_DISK`), never by the application.

enum CreateMachineStep: Int, CaseIterable, Equatable {
    case name, profile, memory, disk, summary
}

struct CreateMachineIssue: Equatable {
    enum Severity: Equatable { case error, warning }

    var step: CreateMachineStep
    var severity: Severity
    var message: String
}

/// One line of the profile list (closed list of S11-02). Both profiles are test-only.
struct CreateMachineProfileChoice: Equatable {
    var profile: MachineProfile
    var title: String
    var detail: String
}

/// What the model reads from the system; the tests replace it.
struct CreateMachineEnvironment {
    var hostMemoryBytes: UInt64
    /// The directory of new bundles (`Catalog.defaultDirectory`). It may not exist yet.
    var defaultDirectory: URL
    /// Free bytes of the volume that holds the given URL (the nearest existing ancestor); nil when it cannot be told.
    var availableSpace: (URL) -> UInt64?

    static func standard(defaultDirectory: URL) -> CreateMachineEnvironment {
        CreateMachineEnvironment(hostMemoryBytes: ProcessInfo.processInfo.physicalMemory, defaultDirectory: defaultDirectory,
                                 availableSpace: CreateMachineModel.volumeAvailableBytes(at:))
    }
}

/// The disk to create in the plan.
struct CreateMachineDiskPlan: Equatable {
    var name: String
    var capacityBytes: UInt64
}

/// The result of the confirmation. Executing it is S11-08b: the order of VM_BUNDLE 5.1 (directories, `lock`, VMHost with `CONFIGURE`
/// without `disks`, `PREPARE_DISK`, `STOP`, then `config.plist` last).
struct CreateMachinePlan: Equatable {
    var bundleURL: URL
    /// The final `config.plist`: with the disk (`disks/<name>`) when one is planned.
    var config: BundleConfig
    /// The `CONFIGURE` of the creation session: the same machine WITHOUT `disks` (the image does not exist yet; VM_BUNDLE 5.1 step 3).
    var creationConfigure: ControlV2Configure
    var disk: CreateMachineDiskPlan?
}

final class CreateMachineModel: ObservableObject {
    enum Phase: Equatable { case editing, cancelled, confirmed }

    /// File name of the image of the first disk inside `disks/` (a valid name of VM_BUNDLE 3.3). The name is chosen here, not asked.
    static let diskImageName = "disk0.img"
    static let defaultDiskCapacityBytes: UInt64 = 32 << 30
    static let diskBlockBytes: UInt64 = 512
    static let bytesPerMiB: UInt64 = 1 << 20

    let environment: CreateMachineEnvironment

    @Published var name: String
    @Published var profile: MachineProfile
    @Published var memoryMiB: Int
    /// A machine with a disk does not start in this version (ISSUE-055); the default is a machine without one.
    @Published var diskEnabled: Bool
    @Published var diskCapacityBytes: UInt64
    @Published private(set) var currentStep: CreateMachineStep = .name
    @Published private(set) var phase: Phase = .editing

    init(environment: CreateMachineEnvironment, name: String = "") {
        self.environment = environment
        self.name = name
        self.profile = .displayTestPattern
        self.memoryMiB = BundleConfigLimits.defaultMemoryMiB
        self.diskEnabled = false
        self.diskCapacityBytes = Self.defaultDiskCapacityBytes
    }

    // MARK: lists and derived values

    /// The closed list of profiles: both are labeled as test-only (the plan: "тестовый, без гостя").
    var profileChoices: [CreateMachineProfileChoice] {
        MachineProfile.allCases.map { profile in
            switch profile {
            case .displayTestPattern:
                return CreateMachineProfileChoice(profile: profile, title: "Тестовая картинка, один экран",
                                                  detail: "тестовый, без гостя")
            case .displayTwoScreensTest:
                return CreateMachineProfileChoice(profile: profile, title: "Тестовая картинка, два экрана",
                                                  detail: "тестовый, без гостя")
            }
        }
    }

    /// The memory limit relative to the host in MiB (half of `hw.memsize`, rounded down).
    var hostMemoryLimitMiB: Int { Int(environment.hostMemoryBytes / Self.bytesPerMiB / 2) }

    /// Name of the directory of the bundle: VM_BUNDLE 2 item 4 without the numeric suffix.
    var bundleDirectoryName: String { Self.bundleDirectoryName(for: name) }

    var bundleURL: URL { environment.defaultDirectory.appendingPathComponent(bundleDirectoryName, isDirectory: true) }

    /// Free bytes of the volume of the default directory (the "disk" step shows it); nil when unknown.
    var availableSpaceBytes: UInt64? { environment.availableSpace(environment.defaultDirectory) }

    static func bundleDirectoryName(for name: String) -> String {
        var scalars = String.UnicodeScalarView()
        for scalar in name.unicodeScalars {
            if scalar == "/" || scalar == ":" || scalar.value < 0x20 || scalar.value == 0x7F {
                scalars.append("_")
            } else {
                scalars.append(scalar)
            }
        }
        var text = String(scalars)
        if text.hasPrefix(".") { text = "_" + text.dropFirst() }
        if text.allSatisfy({ $0 == "_" || $0 == " " }) { text = "Machine" }
        return text + CatalogLimits.bundleSuffix
    }

    // MARK: checks

    /// The issues of one step, errors first. Computed from the current values and the file system each time: nothing is cached.
    func issues(for step: CreateMachineStep) -> [CreateMachineIssue] {
        switch step {
        case .name: return nameIssues()
        case .profile: return []  // `profile` is a `MachineProfile`: the closed list is the type
        case .memory: return memoryIssues()
        case .disk: return diskIssues()
        case .summary: return summaryIssues()
        }
    }

    func errors(for step: CreateMachineStep) -> [CreateMachineIssue] { issues(for: step).filter { $0.severity == .error } }

    func warnings(for step: CreateMachineStep) -> [CreateMachineIssue] { issues(for: step).filter { $0.severity == .warning } }

    /// Whether the current step has no error (warnings do not block).
    var canAdvance: Bool { phase == .editing && currentStep != .summary && errors(for: currentStep).isEmpty }

    /// Whether every step is valid and the wizard stands on the summary.
    var canConfirm: Bool {
        phase == .editing && currentStep == .summary && CreateMachineStep.allCases.allSatisfy { errors(for: $0).isEmpty }
    }

    private func error(_ step: CreateMachineStep, _ message: String) -> CreateMachineIssue {
        CreateMachineIssue(step: step, severity: .error, message: message)
    }

    private func warning(_ step: CreateMachineStep, _ message: String) -> CreateMachineIssue {
        CreateMachineIssue(step: step, severity: .warning, message: message)
    }

    private func nameIssues() -> [CreateMachineIssue] {
        let bytes = name.utf8.count
        if bytes == 0 { return [error(.name, "Введите имя машины.")] }
        if bytes > BundleConfigLimits.maxNameBytes {
            return [error(.name, "Имя слишком длинное: \(bytes) байт в UTF-8, допустимо не больше \(BundleConfigLimits.maxNameBytes).")]
        }
        if name.unicodeScalars.contains(where: { $0.value < 0x20 || $0.value == 0x7F }) {
            return [error(.name, "В имени есть управляющий символ. Уберите его.")]
        }
        if name.trimmingCharacters(in: .whitespaces).isEmpty {
            return [error(.name, "Имя состоит только из пробелов. Введите имя машины.")]
        }
        if entryExists(at: bundleURL) {
            return [error(.name, "Машина с таким именем уже есть: в каталоге «\(environment.defaultDirectory.path)» занято «\(bundleDirectoryName)». Выберите другое имя.")]
        }
        return []
    }

    private func memoryIssues() -> [CreateMachineIssue] {
        let minimum = BundleConfigLimits.minMemoryMiB
        let maximum = BundleConfigLimits.maxMemoryMiB
        if memoryMiB < minimum || memoryMiB > maximum {
            return [error(.memory, "Память \(memoryMiB) МиБ вне допустимого диапазона: от \(minimum) до \(maximum) МиБ.")]
        }
        let limit = hostMemoryLimitMiB
        if memoryMiB > limit {
            return [error(.memory, "Память \(memoryMiB) МиБ больше половины памяти компьютера: допустимо не больше \(limit) МиБ.")]
        }
        return []
    }

    private func diskIssues() -> [CreateMachineIssue] {
        guard diskEnabled else { return [] }
        let capacity = diskCapacityBytes
        if capacity == 0 { return [error(.disk, "Ёмкость диска не может быть нулевой.")] }
        if capacity % Self.diskBlockBytes != 0 {
            return [error(.disk, "Ёмкость диска должна быть кратна \(Self.diskBlockBytes) байтам.")]
        }
        if capacity > UInt64(Int64.max) { return [error(.disk, "Ёмкость диска больше предела в \(Int64.max) байт.")] }
        if let available = availableSpaceBytes, capacity > available {
            return [warning(.disk, "Ёмкость диска (\(Self.format(capacity))) больше свободного места на томе (\(Self.format(available))). "
                + "Образ — разреженный файл и, по-видимому, не резервирует место на диске (гипотеза, не проверена), но процесс машины "
                + "отклонит создание, если свободного места меньше ёмкости.")]
        }
        return []
    }

    private func summaryIssues() -> [CreateMachineIssue] {
        var result: [CreateMachineIssue] = []
        // Errors of the earlier steps: the summary cannot be confirmed while one stands.
        for step in CreateMachineStep.allCases where step != .summary { result += errors(for: step) }
        if diskEnabled {
            result.append(warning(.summary, "Машина с диском в этой версии не запускается до завершения работ по надёжности хранения "
                + "(этап 9): при запуске будет отказ «unsupported_durability». Диск будет создан, но использовать машину пока нельзя."))
            result += warnings(for: .disk)
        }
        return result
    }

    /// `lstat`: a dangling symlink and a file with this name count as taken, nothing is followed.
    private func entryExists(at url: URL) -> Bool {
        var info = stat()
        return lstat(url.path, &info) == 0
    }

    // MARK: navigation

    /// Moves to the next step when the current one has no error. A refusal changes nothing: the issues are read with `issues(for:)`.
    @discardableResult
    func advance() -> Bool {
        guard canAdvance, let next = CreateMachineStep(rawValue: currentStep.rawValue + 1) else { return false }
        currentStep = next
        return true
    }

    /// Moves back one step; the values stay as they are.
    func back() {
        guard phase == .editing, let previous = CreateMachineStep(rawValue: currentStep.rawValue - 1) else { return }
        currentStep = previous
    }

    /// Cancellation at any step: nothing exists to remove, because this model creates nothing.
    func cancel() {
        guard phase == .editing else { return }
        phase = .cancelled
    }

    /// The confirmation: checks every step again (the directory may have appeared meanwhile) and returns the plan; creates nothing.
    func confirm() -> CreateMachinePlan? {
        guard canConfirm else { return nil }
        var config = BundleConfig(name: name, machineProfile: profile)
        config.memoryMiB = memoryMiB
        let creation = config.controlConfigure() // before the disk is added: PREPARE_DISK runs in a session without `disks`
        var disk: CreateMachineDiskPlan?
        if diskEnabled {
            config.disks = [BundleDisk(location: .inBundle(name: Self.diskImageName))]
            disk = CreateMachineDiskPlan(name: Self.diskImageName, capacityBytes: diskCapacityBytes)
        }
        phase = .confirmed
        return CreateMachinePlan(bundleURL: bundleURL, config: config, creationConfigure: creation, disk: disk)
    }

    // MARK: summary text

    /// Lines of the summary step: label and value.
    func summaryLines() -> [(label: String, value: String)] {
        var lines: [(label: String, value: String)] = [
            ("Имя", name),
            ("Профиль", profileChoices.first { $0.profile == profile }.map { "\($0.title) (\($0.detail))" } ?? profile.rawValue),
            ("Память", "\(memoryMiB) МиБ"),
            ("Диск", diskEnabled ? "RAW, \(Self.format(diskCapacityBytes))" : "нет"),
            ("Каталог", bundleURL.path),
        ]
        if diskEnabled { lines.insert(("Файл диска", "disks/\(Self.diskImageName)"), at: 4) }
        return lines
    }

    // MARK: system

    static func format(_ bytes: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(min(bytes, UInt64(Int64.max))), countStyle: .binary)
    }

    /// `volumeAvailableCapacityForImportantUsageKey` of the nearest existing ancestor (the default directory is created lazily).
    static func volumeAvailableBytes(at url: URL) -> UInt64? {
        var probe = url
        var info = stat()
        while lstat(probe.path, &info) != 0 {
            let parent = probe.deletingLastPathComponent()
            if parent.path == probe.path { return nil }
            probe = parent
        }
        guard let values = try? probe.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey]),
              let available = values.volumeAvailableCapacityForImportantUsage, available >= 0 else { return nil }
        return UInt64(available)
    }
}
