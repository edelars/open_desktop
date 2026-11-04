use std::process::ExitCode;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod fxsave64;
mod native_fp;
mod rep_trace;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod native;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod native_v2;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod supervisor;

fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("--help") {
        println!(
            "od-x86-runner: isolated x86-64 oracle runner\nUsage: od-x86-runner --fixture CASE.json --json\n       od-x86-runner --reduce-native-xorpd CASE.json [--limit N] [--output-dir DIR]\n       od-x86-runner --reduce-native-mov-memory CASE.json [--limit N] [--output-dir DIR]\n       od-x86-runner --reduce-native-push-pop CASE.json [--limit N] [--output-dir DIR]\n       od-x86-runner --fxsave64-oracle (Linux x86_64 only)"
        );
        return ExitCode::SUCCESS;
    }
    if std::env::args().nth(1).as_deref() == Some("--fxsave64-oracle")
        && !cfg!(all(target_os = "linux", target_arch = "x86_64"))
    {
        eprintln!("FXSAVE64 oracle unavailable/not_run: requires Linux x86_64 CPL3 execution");
        return ExitCode::from(4);
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
