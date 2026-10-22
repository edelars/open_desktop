use std::process::ExitCode;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod native;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod supervisor;

fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("--help") {
        println!(
            "od-x86-runner: isolated physical x86-64 fixture runner\nUsage: od-x86-runner --fixture CASE.json --json"
        );
        return ExitCode::SUCCESS;
    }
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        eprintln!("unsupported platform: od-x86-runner requires Linux x86_64");
        return ExitCode::from(4);
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        return native::run_cli();
    }
    #[allow(unreachable_code)]
    ExitCode::from(4)
}
