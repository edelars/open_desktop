import AppKit
import ODShared
import SwiftUI

/// The window of the application (S11-06b): the list of machines, the actions on the selected one, the empty library.
/// Actions are available by the table of CONTROL_PROTOCOL_V2.md 7.2 (`MachineActionTable`), in the context menu, in the toolbar and in
/// the menu "Машина". The wizard that creates a machine is S11-08: the button for it is in place and does not create a bundle.
struct LibraryView: View {
    @ObservedObject var model: LibraryModel

    var body: some View {
        VStack(spacing: 0) {
            if let banner = model.banner {
                HStack(alignment: .firstTextBaseline) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(banner).font(.callout).textSelection(.enabled)
                        if let details = model.bannerDetails {
                            DisclosureGroup("Подробности") {
                                Text(details).font(.caption2).foregroundColor(.secondary).textSelection(.enabled)
                            }
                            .font(.caption)
                        }
                    }
                    Spacer()
                    Button("Скрыть") { model.dismissBanner() }.buttonStyle(.link)
                }
                .padding(10)
                .background(Color.orange.opacity(0.15))
            }
            if model.machines.isEmpty {
                EmptyLibraryView(model: model)
            } else {
                List(selection: $model.selection) {
                    ForEach(model.machines) { machine in
                        MachineRowView(machine: machine, hasIdentityConflict: model.conflictingIDs.contains(machine.id))
                            .tag(machine.id)
                            .contextMenu { MachineActionButtons(machine: machine, model: model) }
                    }
                }
            }
            Divider()
            Text(vmhostDescription)
                .font(.caption2)
                .foregroundColor(.secondary)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(8)
        }
        .frame(minWidth: 520, minHeight: 320)
        .toolbar {
            ToolbarItemGroup {
                MachineActionButtons(machine: model.selected, model: model, includeDelete: false)
                Button { model.refresh() } label: { Label("Обновить", systemImage: "arrow.clockwise") }
                    .help("Перечитать машины и спросить у VMHost запущенной машины его состояние")
            }
        }
        .sheet(item: $model.pendingDelete) { machine in
            DeleteMachineSheet(machine: machine, model: model)
        }
    }

    private var vmhostDescription: String {
        let origin = model.vmhost.source == .argument ? "--vmhost" : "bundle"
        let state = model.vmhost.isExecutable ? "найден" : "не найден"
        return "VMHost (\(origin), \(state)): \(model.vmhost.url.path)"
    }
}

/// The actions of one machine as buttons; the same list serves the context menu and the toolbar. Each one is active only when
/// `MachineActionTable` says so.
struct MachineActionButtons: View {
    let machine: MachineController?
    @ObservedObject var model: LibraryModel
    var includeDelete = true

    var body: some View {
        Button { machine?.start() } label: { Label("Запустить", systemImage: "play.fill") }
            .disabled(!(machine?.allows(.start) ?? false))
        Button { machine?.pause() } label: { Label("Приостановить", systemImage: "pause.fill") }
            .disabled(!(machine?.allows(.pause) ?? false))
        Button { machine?.resume() } label: { Label("Продолжить", systemImage: "playpause.fill") }
            .disabled(!(machine?.allows(.resume) ?? false))
        Button { machine?.stop() } label: { Label("Остановить", systemImage: "stop.fill") }
            .disabled(!(machine?.allows(.stop) ?? false))
        Button { machine?.abort() } label: { Label("Прервать запуск", systemImage: "xmark.circle") }
            .help("Завершить процесс VMHost, который не дошёл до состояния «Запущена»")
            .disabled(!(machine?.allows(.abort) ?? false))
        if includeDelete {
            Divider()
            Button(role: .destructive) { model.pendingDelete = machine } label: { Label("Удалить машину…", systemImage: "trash") }
                .disabled(!(machine?.allows(.delete) ?? false))
        }
    }
}

/// Menu "Машина" of the application.
struct MachineCommands: Commands {
    @ObservedObject var model: LibraryModel

    var body: some Commands {
        CommandMenu("Машина") {
            Button("Открыть…") { model.presentOpenPanel() }.keyboardShortcut("o")
            Divider()
            MachineActionButtons(machine: model.selected, model: model)
            Divider()
            Button("Обновить") { model.refresh() }.keyboardShortcut("r")
        }
    }
}

extension LibraryModel {
    /// The one panel of "Открыть…" (the menu and the empty library): a machine directory `.odvm`.
    func presentOpenPanel() {
        let panel = NSOpenPanel()
        panel.title = "Открыть машину"
        panel.message = "Выберите каталог машины (.odvm)"
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        if panel.runModal() == .OK, let url = panel.url { open(bundle: url) }
    }
}

/// The empty library: a hint and the place of the action "Создать машину…". The wizard is S11-08; until then the button is disabled and
/// creates nothing (the plan wants a default bundle here, but creating bundles is S11-08).
struct EmptyLibraryView: View {
    @ObservedObject var model: LibraryModel

    var body: some View {
        VStack(spacing: 12) {
            Text("Машин пока нет").font(.title2)
            Text("Создание машин — следующий этап (S11-08). Готовую машину можно открыть кнопкой «Открыть…» или из меню «Машина» → «Открыть…».")
                .font(.callout)
                .foregroundColor(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 420)
            HStack {
                // Placeholder of the wizard (S11-08): disabled, no bundle is created.
                Button("Создать машину…") {}.disabled(true).help("Создание машин — следующий этап (S11-08)")
                Button("Открыть…") { model.presentOpenPanel() }
            }
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityIdentifier("empty-library")
    }
}

/// "Удалить машину…": the name of the bundle and the switch "удалить файлы с диска" (off by default: only from the library, the bundle
/// directory stays on the disk). A machine with `disks` gets the message about `unsupported_durability` and its files stay (before S9-06).
struct DeleteMachineSheet: View {
    @ObservedObject var machine: MachineController
    @ObservedObject var model: LibraryModel
    @State private var deleteFiles = false
    @State private var failure: LibraryFailure?

    private var canDeleteFiles: Bool { LibraryModel.canDeleteFiles(of: machine) }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Удалить машину «\(machine.displayName)»?").font(.headline)
            Text("Каталог: \(machine.entry.bundleURL.lastPathComponent)").font(.callout).textSelection(.enabled)
            Toggle("Удалить файлы с диска", isOn: $deleteFiles)
                .disabled(!canDeleteFiles)
            if !canDeleteFiles {
                Text(ErrorCatalog.entry(for: .removalUnsupportedDurability).explanation + " " + ErrorCatalog.entry(for: .removalUnsupportedDurability).action)
                    .font(.caption)
                    .foregroundColor(.orange)
            }
            Text(deleteFiles
                 ? "Каталог \(machine.entry.bundleURL.lastPathComponent) со всеми файлами будет удалён с диска. Это нельзя отменить."
                 : "Машина исчезнет из списка. Каталог \(machine.entry.bundleURL.lastPathComponent) останется на диске, его можно открыть снова.")
                .font(.caption)
                .foregroundColor(.secondary)
            if let failure {
                Text("Не удалось удалить: \(failure.text).").font(.caption).foregroundColor(.red).textSelection(.enabled)
                Text(failure.hint).font(.caption2).foregroundColor(.secondary).textSelection(.enabled)
                if let details = failure.details {
                    Text(details).font(.caption2).foregroundColor(.secondary).textSelection(.enabled)
                }
            }
            HStack {
                Spacer()
                Button("Отмена", role: .cancel) { model.pendingDelete = nil }.keyboardShortcut(.cancelAction)
                Button(deleteFiles ? "Удалить с диска" : "Убрать из списка", role: .destructive) { confirm() }
                    .disabled(!machine.allows(.delete))
            }
        }
        .padding(20)
        .frame(width: 440)
    }

    private func confirm() {
        do {
            try model.delete(machine, deleteFiles: deleteFiles)
        } catch {
            failure = LibraryFailure(error)
        }
    }
}
