use od_machine::{DiagnosticExit, DiagnosticTerminalExit};

fn accepts_all_terminal_causes(value: DiagnosticTerminalExit) {
    match value {
        DiagnosticTerminalExit::Halted => {}
        DiagnosticTerminalExit::Fault(_) => {}
        DiagnosticTerminalExit::Unsupported { .. } => {}
        DiagnosticTerminalExit::BackendError(_) => {}
        DiagnosticTerminalExit::StopAddress => {}
    }
}

#[test]
fn terminal_exit_is_available_to_external_callers() {
    let cause = DiagnosticTerminalExit::Halted;
    accepts_all_terminal_causes(cause.clone());
    assert_eq!(
        DiagnosticExit::Terminal(cause),
        DiagnosticExit::Terminal(DiagnosticTerminalExit::Halted)
    );
}
