import ODShared
import SwiftUI

/// The sheet "Создать машину" (S11-08b). The steps, their checks and the limits are `CreateMachineModel` (S11-08a); the execution of the
/// plan and the rollback are `CreateMachineExecutor`. This file only shows them. "Отмена" at any step before "Создать" creates nothing:
/// the model creates nothing before `confirm()`. While the machine is being created the sheet cannot be closed.
struct CreateMachineWizard: View {
    let library: LibraryModel
    @StateObject private var model: CreateMachineModel
    @State private var executor: CreateMachineExecutor?
    let onClose: () -> Void

    init(library: LibraryModel, onClose: @escaping () -> Void) {
        self.library = library
        self.onClose = onClose
        _model = StateObject(wrappedValue: CreateMachineModel(environment: .standard(defaultDirectory: library.catalog.defaultDirectory)))
    }

    var body: some View {
        Group {
            if let executor {
                CreateMachineProgressView(executor: executor, library: library, onClose: onClose)
            } else {
                editor
            }
        }
        .padding(20)
        .frame(width: 520)
        .interactiveDismissDisabled(executor != nil)
    }

    // MARK: steps

    private var editor: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Создать машину").font(.title3).bold()
            Text("Шаг \(model.currentStep.rawValue + 1) из \(CreateMachineStep.allCases.count): \(title(of: model.currentStep))")
                .font(.caption)
                .foregroundColor(.secondary)
            stepBody
                .frame(maxWidth: .infinity, alignment: .topLeading)
                .frame(minHeight: 200, alignment: .top)
            ForEach(Array(model.issues(for: model.currentStep).enumerated()), id: \.offset) { _, issue in
                Text(issue.message)
                    .font(.callout)
                    .foregroundColor(issue.severity == .error ? .red : .orange)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
            HStack {
                Button("Отмена", role: .cancel) {
                    model.cancel() // nothing was created: there is nothing to remove
                    onClose()
                }
                .keyboardShortcut(.cancelAction)
                Spacer()
                Button("Назад") { model.back() }.disabled(model.currentStep == .name)
                if model.currentStep == .summary {
                    Button("Создать") { confirm() }
                        .keyboardShortcut(.defaultAction)
                        .disabled(!model.canConfirm)
                } else {
                    Button("Далее") { model.advance() }
                        .keyboardShortcut(.defaultAction)
                        .disabled(!model.canAdvance)
                }
            }
        }
    }

    private func title(of step: CreateMachineStep) -> String {
        switch step {
        case .name: return "имя"
        case .profile: return "профиль"
        case .memory: return "память"
        case .disk: return "диск"
        case .summary: return "сводка"
        }
    }

    @ViewBuilder private var stepBody: some View {
        switch model.currentStep {
        case .name: nameStep
        case .profile: profileStep
        case .memory: memoryStep
        case .disk: diskStep
        case .summary: summaryStep
        }
    }

    private var nameStep: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Имя машины").font(.headline)
            TextField("Например, Тест", text: $model.name)
                .textFieldStyle(.roundedBorder)
                .onSubmit { model.advance() }
            Text("Каталог машины: \(model.bundleURL.path)").font(.caption).foregroundColor(.secondary).textSelection(.enabled)
        }
    }

    private var profileStep: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Профиль").font(.headline)
            Picker("Профиль", selection: $model.profile) {
                ForEach(model.profileChoices, id: \.profile) { choice in
                    Text("\(choice.title) (\(choice.detail))").tag(choice.profile)
                }
            }
            .pickerStyle(.radioGroup)
            .labelsHidden()
            Text("Оба профиля тестовые: гостевой системы в них нет, машина показывает тестовую картинку.")
                .font(.caption)
                .foregroundColor(.secondary)
        }
    }

    private var memoryStep: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Память").font(.headline)
            HStack {
                TextField("МиБ", value: $model.memoryMiB, format: .number.grouping(.never))
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 100)
                Text("МиБ")
                Stepper("", value: $model.memoryMiB, step: 256).labelsHidden() // no `in:` range: a value outside is an error, not a silent change
            }
            Text("Допустимо от \(BundleConfigLimits.minMemoryMiB) до \(min(BundleConfigLimits.maxMemoryMiB, model.hostMemoryLimitMiB)) МиБ "
                + "(не больше половины памяти компьютера). По умолчанию \(BundleConfigLimits.defaultMemoryMiB) МиБ.")
                .font(.caption)
                .foregroundColor(.secondary)
        }
    }

    /// The capacity of the disk in whole GiB: no fraction, so the byte count is a multiple of 512 and nothing is rounded silently.
    private var capacityGiB: Binding<Int> {
        Binding(
            get: { Int(clamping: model.diskCapacityBytes >> 30) },
            set: { value in
                let (bytes, overflow) = UInt64(clamping: max(value, 0)).multipliedReportingOverflow(by: 1 << 30)
                // An overflow is shown as the error "больше предела", not corrected.
                model.diskCapacityBytes = overflow ? UInt64.max : bytes
            })
    }

    private var diskStep: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Диск").font(.headline)
            Toggle("Создать RAW-диск", isOn: $model.diskEnabled)
            HStack {
                TextField("ГиБ", value: capacityGiB, format: .number.grouping(.never))
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 100)
                Text("ГиБ")
            }
            .disabled(!model.diskEnabled)
            if let available = model.availableSpaceBytes {
                Text("Свободно на томе каталога машин: \(CreateMachineModel.format(available)).").font(.caption)
            } else {
                Text("Свободное место на томе узнать не удалось.").font(.caption)
            }
            Text("Образ создаёт процесс машины. Предполагается, что файл разреженный и сразу не занимает всё место (гипотеза, не проверена); "
                + "если места не хватает, процесс машины откажет, и диск не будет создан.")
                .font(.caption)
                .foregroundColor(.secondary)
            Text("Машина с диском в этой версии не запускается: запуск отвечает отказом до этапа 9.")
                .font(.caption)
                .foregroundColor(.secondary)
        }
    }

    private var summaryStep: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Сводка").font(.headline)
            Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 4) {
                ForEach(Array(model.summaryLines().enumerated()), id: \.offset) { _, line in
                    GridRow {
                        Text(line.label).foregroundColor(.secondary)
                        Text(line.value).textSelection(.enabled)
                    }
                }
            }
            .font(.callout)
        }
    }

    private func confirm() {
        guard let plan = model.confirm() else { return }
        let executor = CreateMachineExecutor(plan: plan, vmhost: library.vmhost)
        self.executor = executor
        executor.start()
    }
}

/// The creation in progress, its result and the way out of a failure: "Повторить" (only after `busy` at the write of `config.plist`),
/// "Удалить частично созданную машину…" (after a confirmation that names what goes and what stays) and "Закрыть".
struct CreateMachineProgressView: View {
    @ObservedObject var executor: CreateMachineExecutor
    let library: LibraryModel
    let onClose: () -> Void
    @State private var confirmingRollback = false
    @State private var opened = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Создание машины").font(.title3).bold()
            content
        }
        .onChange(of: executor.phase) { _, phase in finish(phase) }
        .onAppear { finish(executor.phase) }
    }

    @ViewBuilder private var content: some View {
        switch executor.phase {
        case .idle, .running:
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text(stageText)
            }
            if let progress = executor.progress, progress.total > 0 {
                ProgressView(value: Double(min(progress.completed, progress.total)), total: Double(progress.total))
                Text("\(CreateMachineModel.format(progress.completed)) из \(CreateMachineModel.format(progress.total))").font(.caption)
            }
        case .succeeded:
            Text("Машина создана и добавлена в список.")
            HStack { Spacer(); Button("Закрыть") { onClose() }.keyboardShortcut(.defaultAction) }
        case .failed(let failure):
            failureView(failure)
        }
    }

    private var stageText: String {
        switch executor.phase {
        case .running(.directories), .idle: return "Создаются каталоги машины…"
        case .running(.startingHost): return "Запускается процесс машины…"
        case .running(.configuring): return "Передаётся конфигурация…"
        case .running(.preparingDisk): return "Создаётся образ диска…"
        case .running(.stopping): return "Процесс машины останавливается…"
        case .running(.writingConfig): return "Записывается конфигурация машины…"
        default: return ""
        }
    }

    private func failureView(_ failure: CreateMachineFailure) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(failure.headline).foregroundColor(.red).textSelection(.enabled)
            Text(failure.hint).font(.callout).foregroundColor(.secondary).textSelection(.enabled)
            if let details = failure.details {
                DisclosureGroup("Подробности") {
                    Text(details).font(.caption2).foregroundColor(.secondary).textSelection(.enabled)
                }
                .font(.caption)
            }
            if let outcome = executor.rollbackOutcome {
                Text(Self.rollbackText(outcome)).font(.callout).textSelection(.enabled)
            }
            HStack {
                Spacer()
                if failure.canRetry {
                    Button("Повторить") { executor.retryWriteConfig() }
                }
                if failure.canRollback, executor.rollbackOutcome == nil {
                    Button("Удалить частично созданную машину…", role: .destructive) { confirmingRollback = true }
                }
                Button("Закрыть") { onClose() }.keyboardShortcut(.defaultAction)
            }
        }
        .alert("Удалить частично созданную машину?", isPresented: $confirmingRollback) {
            Button("Удалить", role: .destructive) { executor.rollback() }
            Button("Отмена", role: .cancel) {}
        } message: {
            Text(executor.rollbackDescription)
        }
    }

    static func rollbackText(_ outcome: CreateMachineRollbackOutcome) -> String {
        switch outcome {
        case .notAllowed:
            return "Удалять нечего."
        case .refused:
            return "Блокировку каталога машины взять не удалось, поэтому ничего не удалено. Каталог можно убрать вручную в Finder."
        case .done(let removed, let remaining):
            var text = removed.isEmpty ? "Ничего не удалено." : "Удалено: \(removed.joined(separator: ", "))."
            if !remaining.isEmpty {
                text += " Остались: \(remaining.joined(separator: ", ")). Приложение не удаляет файл блокировки и образ диска; уберите каталог вручную в Finder."
            }
            return text
        }
    }

    /// Success: the bundle goes to the library (once) and the machine is selected.
    private func finish(_ phase: CreateMachineExecutor.Phase) {
        guard phase == .succeeded, !opened else { return }
        opened = true
        library.open(bundle: executor.bundleURL)
    }
}
