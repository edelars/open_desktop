import SwiftUI

/// One row of the library (S11-06b): name, profile, state, memory. The state is the one `MachineController` keeps from the events; the
/// row never infers it. Test-only profiles say so: there is no guest.
struct MachineRowView: View {
    @ObservedObject var machine: MachineController
    let hasIdentityConflict: Bool

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Circle()
                .fill(stateColor)
                .frame(width: 10, height: 10)
                .padding(.top, 5)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline) {
                    Text(machine.displayName).font(.headline)
                    Spacer()
                    Text(stateLabel)
                        .font(.subheadline)
                        .foregroundColor(stateColor)
                        .accessibilityIdentifier("machine-state")
                }
                if let config = machine.config {
                    Text("\(MachinePresentation.profileText(config.machineProfile)) · \(MachinePresentation.memoryText(mib: config.memoryMiB))")
                        .font(.caption)
                        .foregroundColor(.secondary)
                } else if case .config(let state) = machine.bundleStatus, let issue = state.issue {
                    Text(issue.description).font(.caption).foregroundColor(.secondary).textSelection(.enabled)
                }
                if hasIdentityConflict {
                    Text("Тот же идентификатор, что у другой машины в списке (копия каталога). Они не объединены.")
                        .font(.caption)
                        .foregroundColor(.orange)
                }
                if let notice = machine.notice {
                    Text(notice.text).font(.caption).foregroundColor(.red).textSelection(.enabled)
                        .accessibilityIdentifier("machine-notice")
                    if let details = notice.details {
                        DisclosureGroup("Подробности") {
                            Text(details).font(.caption2).foregroundColor(.secondary).textSelection(.enabled)
                        }
                        .font(.caption)
                    }
                }
                if let query = machine.lastQueryText {
                    Text(query).font(.caption2).foregroundColor(.secondary)
                }
            }
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .contain)
    }

    /// A bundle that cannot be read is shown by its class (incompatible, damaged, missing); otherwise by the event-driven state.
    private var stateLabel: String {
        MachinePresentation.bundleText(machine.bundleStatus) ?? MachinePresentation.stateText(machine.runState)
    }

    private var stateColor: Color {
        if MachinePresentation.bundleText(machine.bundleStatus) != nil { return .orange }
        switch machine.runState {
        case .running: return .green
        case .paused: return .yellow
        case .starting, .shuttingDown: return .blue
        case .failed: return .red
        case .stopped: return .gray
        }
    }
}
