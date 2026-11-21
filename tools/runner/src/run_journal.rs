//! Phase journal and periodic screen dumps (S8-11).
//!
//! Both are driven by the machine owner between quanta: the hook of
//! `LoadedFirmwareBoard::run_boot_with_quantum_hook` reads the COM1 transcript, the virtual clock and the
//! display through a shared reference (the hook gets no mutable reference to the machine). The
//! journal calls only reading methods of the machine (`diagnostic_*`, `read_*`); the type would
//! also allow `&self` methods that change display state, which the journal does not call. It does
//! not cut quanta (it sees the boundaries the guest's own I/O creates). Nothing here is read from another
//! thread while a quantum runs.

use od_machine::{DisplayReadError, FrameInfo, LoadedFirmwareBoard, QuantumProgress};
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_MAX_FILES: u64 = 2000;
pub const DEFAULT_MAX_BYTES: u64 = 4 << 30;
/// `fb-<seq>-<virtual ms>ms.raw`: a 64-byte header, then `stride * height` pixel bytes.
pub const DUMP_MAGIC: &[u8; 8] = b"ODFBRAW1";
pub const DUMP_HEADER_BYTES: usize = 64;

/// The header of a raw dump. Little-endian: magic, header size, width, height, stride, format,
/// a reserved word, then the frame generation, the mode-change count, the virtual time (ns)
/// and the retired instructions of the run at the moment of the read.
pub fn dump_header(info: &FrameInfo, virtual_ns: u64, retired: u64) -> [u8; DUMP_HEADER_BYTES] {
    let mut header = [0u8; DUMP_HEADER_BYTES];
    header[..8].copy_from_slice(DUMP_MAGIC);
    let words32 = [
        DUMP_HEADER_BYTES as u32,
        info.mode.width,
        info.mode.height,
        info.mode.stride,
        info.mode.format,
        0,
    ];
    for (index, word) in words32.into_iter().enumerate() {
        header[8 + index * 4..12 + index * 4].copy_from_slice(&word.to_le_bytes());
    }
    for (index, word) in [info.generation, info.mode_change_count, virtual_ns, retired]
        .into_iter()
        .enumerate()
    {
        header[32 + index * 8..40 + index * 8].copy_from_slice(&word.to_le_bytes());
    }
    header
}

pub struct FrameDumpConfig {
    pub dir: PathBuf,
    /// Virtual nanoseconds between two looks at the display.
    pub every_ns: u64,
    pub max_files: u64,
    pub max_bytes: u64,
}

pub struct JournalConfig {
    pub phase_log: Option<PathBuf>,
    pub frames: Option<FrameDumpConfig>,
}

/// What a nonce marker line on COM1 says. COM1 is not an evidence channel (WINDOWS.md 3.3): the
/// journal only notes that the line appeared.
#[derive(Debug, Eq, PartialEq)]
struct Marker<'a> {
    nonce: &'a str,
    phase: &'a str,
    status: &'a str,
    sha256: &'a str,
}

fn is_lower_hex(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn valid_phase(phase: &str) -> bool {
    if matches!(
        phase,
        "firstlogon" | "boot" | "build" | "shutdown" | "update"
    ) {
        return true;
    }
    // app:[a-z0-9][a-z0-9._-]{0,31}
    let Some(name) = phase.strip_prefix("app:") else {
        return false;
    };
    let bytes = name.as_bytes();
    (1..=32).contains(&bytes.len())
        && matches!(bytes[0], b'a'..=b'z' | b'0'..=b'9')
        && bytes[1..]
            .iter()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
}

/// One COM1 line without its `\n` against the marker form of WINDOWS.md 3.1:
/// `OD8 <nonce> <phase> <status> <sha256>`, exactly one terminator (`\n` or `\r\n`).
fn parse_marker(line: &[u8]) -> Option<Marker<'_>> {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let text = std::str::from_utf8(line).ok()?;
    let mut fields = text.split(' ');
    let (magic, nonce, phase, status, sha256) = (
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
        fields.next()?,
    );
    (fields.next().is_none()
        && magic == "OD8"
        && is_lower_hex(nonce, 32)
        && valid_phase(phase)
        && matches!(status, "ok" | "fail")
        && is_lower_hex(sha256, 64))
    .then_some(Marker {
        nonce,
        phase,
        status,
        sha256,
    })
}

/// How the run ended, for the `stop` event.
pub struct StopInfo {
    pub exit: String,
    pub stop_reason: Value,
    /// `None` when the run failed before it could count (a run error).
    pub progress: Option<QuantumProgress>,
}

pub struct RunJournal {
    started: Instant,
    phase_log: Option<(File, PathBuf)>,
    frames: Option<FrameDumper>,
    /// COM1 bytes already examined: every complete line before this offset has been seen.
    com1_scanned: usize,
    first_com1_seen: bool,
    /// Cold restarts of the run. Reserved for S8-23 (not implemented): always 0 until then.
    restart: u32,
    events: u64,
    error: Option<String>,
}

impl RunJournal {
    pub fn new(config: JournalConfig) -> Result<Self, String> {
        // The phase log comes first: if it cannot be created, no dump directory is left behind.
        // If the dump directory is then refused, the phase log just created is removed again.
        let phase_log = config
            .phase_log
            .map(|path| {
                File::create(&path)
                    .map(|file| (file, path.clone()))
                    .map_err(|error| format!("{}: {error}", path.display()))
            })
            .transpose()?;
        let frames = match config.frames.map(FrameDumper::new).transpose() {
            Ok(frames) => frames,
            Err(error) => {
                if let Some((file, path)) = phase_log {
                    drop(file);
                    let _ = fs::remove_file(path);
                }
                return Err(error);
            }
        };
        Ok(Self {
            started: Instant::now(),
            phase_log,
            frames,
            com1_scanned: 0,
            first_com1_seen: false,
            restart: 0,
            events: 0,
            error: None,
        })
    }

    fn emit(
        &mut self,
        event: &str,
        progress: Option<QuantumProgress>,
        virtual_ns: u64,
        extra: Value,
    ) {
        let Some((file, path)) = &mut self.phase_log else {
            return;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as u64);
        let mut line = json!({
            "event": event,
            "host_unix_ms": now,
            "host_elapsed_ns": self.started.elapsed().as_nanos() as u64,
            "instructions": progress.map(|progress| progress.retired),
            "quanta": progress.map(|progress| progress.quanta),
            "virtual_ns": virtual_ns,
            "restart": self.restart,
        });
        if let (Some(target), Value::Object(fields)) = (line.as_object_mut(), extra) {
            target.extend(fields);
        }
        // One write per line, flushed at once, so a killed run keeps the lines so far.
        let written = writeln!(file, "{line}").and_then(|()| file.flush());
        match written {
            Ok(()) => self.events += 1,
            Err(error) => {
                self.error = Some(format!("{}: {error}", path.display()));
                self.phase_log = None;
            }
        }
    }

    /// COM1 events: the first byte, and every completed marker-shaped line.
    fn scan_com1(&mut self, board: &LoadedFirmwareBoard, progress: QuantumProgress) {
        let transcript = board.com1_transcript();
        let virtual_ns = board.clock().now_ns();
        if !self.first_com1_seen && !transcript.is_empty() {
            self.first_com1_seen = true;
            self.emit(
                "first_com1_write",
                Some(progress),
                virtual_ns,
                json!({"com1_bytes": transcript.len()}),
            );
        }
        while let Some(end) = transcript[self.com1_scanned..]
            .iter()
            .position(|&byte| byte == b'\n')
        {
            let line = &transcript[self.com1_scanned..self.com1_scanned + end];
            self.com1_scanned += end + 1;
            if let Some(marker) = parse_marker(line) {
                self.emit(
                    "com1_marker",
                    Some(progress),
                    virtual_ns,
                    json!({
                        "nonce": marker.nonce, "phase": marker.phase,
                        "status": marker.status, "sha256": marker.sha256,
                        "channel": "com1", "evidence": false,
                    }),
                );
            } else if line.starts_with(b"OD8 ") {
                self.emit(
                    "com1_marker_unrecognized",
                    Some(progress),
                    virtual_ns,
                    json!({"line": String::from_utf8_lossy(line)}),
                );
            }
        }
    }

    /// Called at every quantum boundary at which the run continues.
    pub fn at_boundary(&mut self, board: &LoadedFirmwareBoard, progress: QuantumProgress) {
        self.scan_com1(board, progress);
        if let Some(frames) = &mut self.frames {
            frames.poll(board, progress.retired, false);
        }
    }

    /// The run is over: the last COM1 lines, the last frame (if it changed) and the `stop` event.
    /// Returns the summary the run report carries.
    pub fn finish(&mut self, board: &LoadedFirmwareBoard, stop: StopInfo) -> Value {
        let progress = stop.progress;
        if let Some(progress) = progress {
            self.scan_com1(board, progress);
        }
        if let Some(frames) = &mut self.frames {
            frames.poll(board, progress.map_or(0, |progress| progress.retired), true);
        }
        self.emit(
            "stop",
            progress,
            board.clock().now_ns(),
            json!({"exit": stop.exit, "stop_reason": stop.stop_reason}),
        );
        json!({
            "phase_log": self.phase_log.as_ref().map(|(_, path)| path.display().to_string()),
            "phase_events": self.events,
            "phase_log_error": self.error,
            "restart": self.restart,
            "framebuffer_dumps": self.frames.as_ref().map(FrameDumper::summary),
        })
    }
}

struct FrameDumper {
    config: FrameDumpConfig,
    next_due_ns: u64,
    /// (generation, mode_change_count) of the last frame written.
    last: Option<(u64, u64)>,
    files: u64,
    bytes: u64,
    skipped_unchanged: u64,
    skipped_limit: u64,
    error: Option<String>,
}

impl FrameDumper {
    /// The directory is created if absent and must be empty: dumps of an earlier run are never
    /// overwritten or mixed in.
    fn new(config: FrameDumpConfig) -> Result<Self, String> {
        let dir = &config.dir;
        fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        let empty = fs::read_dir(dir)
            .map_err(|error| format!("{}: {error}", dir.display()))?
            .next()
            .is_none();
        if !empty {
            return Err(format!(
                "{}: the framebuffer directory is not empty",
                dir.display()
            ));
        }
        Ok(Self {
            next_due_ns: config.every_ns,
            config,
            last: None,
            files: 0,
            bytes: 0,
            skipped_unchanged: 0,
            skipped_limit: 0,
            error: None,
        })
    }

    /// Looks at the display when `force` or the period is due, and writes the frame only if its
    /// generation (or mode) changed since the last written one.
    fn poll(&mut self, board: &LoadedFirmwareBoard, retired: u64, force: bool) {
        let now = board.clock().now_ns();
        if self.error.is_some() || (!force && now < self.next_due_ns) {
            return;
        }
        let every = self.config.every_ns;
        self.next_due_ns = (now / every).saturating_add(1).saturating_mul(every);
        let display = board.display();
        if !display.has_current_frame() {
            return;
        }
        let key = (display.frame_generation(), display.mode_change_count());
        if self.last == Some(key) {
            self.skipped_unchanged += 1;
            return;
        }
        let frame_bytes = display.current_mode().frame_bytes();
        let file_bytes = DUMP_HEADER_BYTES as u64 + frame_bytes;
        if self.files >= self.config.max_files || self.bytes + file_bytes > self.config.max_bytes {
            self.skipped_limit += 1;
            return;
        }
        let mut buffer = vec![0u8; file_bytes as usize];
        let info = match board.read_display_frame_into(&mut buffer[DUMP_HEADER_BYTES..]) {
            Ok(info) => info,
            Err(DisplayReadError::NoActiveMode | DisplayReadError::NoFrame) => return,
            Err(error) => {
                self.error = Some(format!("{error:?}"));
                return;
            }
        };
        buffer[..DUMP_HEADER_BYTES].copy_from_slice(&dump_header(&info, now, retired));
        let path = self.config.dir.join(format!(
            "fb-{:06}-{}ms.raw",
            self.files + 1,
            now / 1_000_000
        ));
        match write_new(&path, &buffer) {
            Ok(()) => {
                self.files += 1;
                self.bytes += file_bytes;
                self.last = Some((info.generation, info.mode_change_count));
            }
            Err(error) => self.error = Some(format!("{}: {error}", path.display())),
        }
    }

    fn summary(&self) -> Value {
        json!({
            "dir": self.config.dir.display().to_string(),
            "every_virtual_ns": self.config.every_ns,
            "files": self.files,
            "bytes": self.bytes,
            "max_files": self.config.max_files,
            "max_bytes": self.config.max_bytes,
            "skipped_unchanged": self.skipped_unchanged,
            "skipped_by_limit": self.skipped_limit,
            "error": self.error,
        })
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stop_snapshot::tests::rom;
    use od_machine::{
        AccessWidth, DISPLAY_FRAMEBUFFER_BASE, DISPLAY_REGISTERS_BASE, FirmwareBoardConfig,
        GuestPhysicalAddress, load_firmware_image,
    };

    /// Unique directory removed on drop, also after a panic.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("{tag}-{}-{nanos}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const NONCE: &str = "0123456789abcdef0123456789abcdef";
    const SHA: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    #[test]
    fn marker_lines_follow_the_documented_form_exactly() {
        let good = format!("OD8 {NONCE} firstlogon ok {SHA}");
        let marker = parse_marker(good.as_bytes()).unwrap();
        assert_eq!(
            (marker.nonce, marker.phase, marker.status, marker.sha256),
            (NONCE, "firstlogon", "ok", SHA)
        );
        // "\r\n" ends the line with one \r; the terminator \n is already cut off.
        assert!(parse_marker(format!("{good}\r").as_bytes()).is_some());
        assert!(parse_marker(format!("{good}\r\r").as_bytes()).is_none());
        assert!(parse_marker(format!("OD8 {NONCE} app:calc-1.x fail {SHA}").as_bytes()).is_some());
        for bad in [
            format!("OD8 {} boot ok {SHA}", NONCE.to_uppercase()),
            format!("OD8 {NONCE} boot ok {SHA} "),
            format!("OD8 {NONCE}  boot ok {SHA}"),
            format!("OD8 {NONCE} reboot ok {SHA}"),
            format!("OD8 {NONCE} boot maybe {SHA}"),
            format!("OD8 {NONCE} app: ok {SHA}"),
            format!("OD8 {NONCE} app:-x ok {SHA}"),
            format!("OD8 {NONCE} app:{} ok {SHA}", "a".repeat(33)),
            format!("OD8 {} boot ok {SHA}", &NONCE[1..]),
            format!("OD8 {NONCE} boot ok {}", &SHA[1..]),
            format!("OD9 {NONCE} boot ok {SHA}"),
            String::new(),
        ] {
            assert!(parse_marker(bad.as_bytes()).is_none(), "{bad:?}");
        }
        assert!(parse_marker(b"OD8 \xff boot ok x").is_none());
    }

    #[test]
    fn the_dump_header_carries_the_mode_and_the_run_position() {
        let info = FrameInfo {
            mode: od_machine::DisplayMode {
                width: 640,
                height: 480,
                stride: 2560,
                format: 1,
            },
            generation: 7,
            mode_change_count: 2,
        };
        let header = dump_header(&info, 1_500_000_000, 99);
        assert_eq!(&header[..8], b"ODFBRAW1");
        let word = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());
        let wide = |at: usize| u64::from_le_bytes(header[at..at + 8].try_into().unwrap());
        assert_eq!(
            [word(8), word(12), word(16), word(20), word(24), word(28)],
            [64, 640, 480, 2560, 1, 0]
        );
        assert_eq!(
            [wide(32), wide(40), wide(48), wide(56)],
            [7, 2, 1_500_000_000, 99]
        );
    }

    fn write_dword(board: &mut od_machine::LoadedFirmwareBoard, address: u64, value: u32) {
        board
            .bus_mut()
            .write_physical(
                GuestPhysicalAddress::new(address),
                AccessWidth::Dword,
                u64::from(value),
                |_| {},
            )
            .unwrap();
    }

    /// Commits a 4x4 mode, as the guest driver would through the registers.
    fn commit_4x4(board: &mut od_machine::LoadedFirmwareBoard) {
        for (register, value) in [(0x40, 4), (0x44, 4), (0x48, 1), (0x4c, 1)] {
            write_dword(board, DISPLAY_REGISTERS_BASE + register, value);
        }
    }

    fn doorbell(board: &mut od_machine::LoadedFirmwareBoard) {
        write_dword(board, DISPLAY_REGISTERS_BASE + 0x80, 0);
    }

    fn dumps(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut files: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().into_string().unwrap(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect();
        files.sort();
        files
    }

    #[test]
    fn frames_are_dumped_only_when_the_generation_changes_and_within_the_limits() {
        let dir = TempDir::new("od-frame-dumps");
        let mut board =
            load_firmware_image(&rom(0x40, &[]), &FirmwareBoardConfig::default()).unwrap();
        let mut dumper = FrameDumper::new(FrameDumpConfig {
            dir: dir.0.clone(),
            every_ns: 100,
            max_files: 2,
            max_bytes: 1 << 20,
        })
        .unwrap();

        // No mode, then a mode but no doorbell: there is no frame to dump.
        dumper.poll(&board, 0, true);
        commit_4x4(&mut board);
        dumper.poll(&board, 0, true);
        assert!(dumps(&dir.0).is_empty());

        // The first frame: the dump is the 64-byte header and the frame as the display reports it.
        for index in 0..16u64 {
            write_dword(
                &mut board,
                DISPLAY_FRAMEBUFFER_BASE + index * 4,
                0x0100_0000 + index as u32,
            );
        }
        doorbell(&mut board);
        board.advance_time_ns(250).unwrap();
        dumper.poll(&board, 77, false);
        let files = dumps(&dir.0);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "fb-000001-0ms.raw");
        let mut frame = vec![0u8; 64];
        let info = board.read_display_frame_into(&mut frame).unwrap();
        assert_eq!(files[0].1.len(), DUMP_HEADER_BYTES + 64);
        assert_eq!(files[0].1[..DUMP_HEADER_BYTES], dump_header(&info, 250, 77));
        assert_eq!(files[0].1[DUMP_HEADER_BYTES..], frame[..]);
        assert_eq!(frame[..4], [0, 0, 0, 1], "pixel 0 as the guest wrote it");

        // Not due yet, and unchanged when due: no new file, and the counters say why.
        dumper.poll(&board, 78, false);
        board.advance_time_ns(1000).unwrap();
        dumper.poll(&board, 79, false);
        dumper.poll(&board, 79, true);
        assert_eq!(dumps(&dir.0).len(), 1);
        assert_eq!(dumper.summary()["skipped_unchanged"], 2);

        // A new generation is dumped; the third one hits the file limit and is counted.
        doorbell(&mut board);
        dumper.poll(&board, 80, true);
        assert_eq!(dumps(&dir.0).len(), 2);
        doorbell(&mut board);
        dumper.poll(&board, 81, true);
        let summary = dumper.summary();
        assert_eq!(dumps(&dir.0).len(), 2);
        assert_eq!(summary["files"], 2);
        assert_eq!(summary["skipped_by_limit"], 1);
        assert_eq!(summary["error"], Value::Null);

        // An occupied directory is refused: dumps of two runs are never mixed.
        assert!(
            FrameDumper::new(FrameDumpConfig {
                dir: dir.0.clone(),
                every_ns: 1,
                max_files: 1,
                max_bytes: 1,
            })
            .is_err()
        );
    }

    #[test]
    fn the_byte_limit_stops_dumping_before_the_file_that_would_exceed_it() {
        let dir = TempDir::new("od-frame-bytes");
        let mut board =
            load_firmware_image(&rom(0x40, &[]), &FirmwareBoardConfig::default()).unwrap();
        commit_4x4(&mut board);
        doorbell(&mut board);
        let mut dumper = FrameDumper::new(FrameDumpConfig {
            dir: dir.0.clone(),
            every_ns: 1,
            max_files: 10,
            max_bytes: (DUMP_HEADER_BYTES + 64) as u64 - 1,
        })
        .unwrap();
        dumper.poll(&board, 0, true);
        assert!(dumps(&dir.0).is_empty());
        assert_eq!(dumper.summary()["skipped_by_limit"], 1);
    }

    #[test]
    fn a_failed_phase_log_leaves_no_dump_directory_and_a_refused_dump_directory_no_phase_log() {
        let dir = TempDir::new("od-journal-order");
        let frames_config = |path: PathBuf| FrameDumpConfig {
            dir: path,
            every_ns: 1_000_000,
            max_files: 10,
            max_bytes: 1 << 20,
        };
        // The phase log's parent is missing: the dump directory must not have been created.
        let dumps = dir.0.join("dumps");
        let result = RunJournal::new(JournalConfig {
            phase_log: Some(dir.0.join("missing-parent").join("phases.jsonl")),
            frames: Some(frames_config(dumps.clone())),
        });
        assert!(result.is_err());
        assert!(!dumps.exists());
        // The dump directory is not empty: the phase log created before it is removed again.
        let busy = dir.0.join("busy");
        fs::create_dir(&busy).unwrap();
        fs::write(busy.join("old.raw"), b"x").unwrap();
        let log = dir.0.join("phases.jsonl");
        let result = RunJournal::new(JournalConfig {
            phase_log: Some(log.clone()),
            frames: Some(frames_config(busy)),
        });
        assert!(result.is_err());
        assert!(!log.exists());
    }

    #[test]
    fn the_phase_log_records_com1_events_with_counters_and_a_reserved_restart() {
        // MOV DX,0x3F8, then OUT each byte of a marker line and a malformed OD8 line; HLT.
        let line = format!("OD8 {NONCE} boot ok {SHA}\nOD8 broken\nplain firmware text\n");
        let mut code = vec![0x66, 0xba, 0xf8, 0x03];
        for byte in line.bytes() {
            code.extend_from_slice(&[0xb0, byte, 0xee]);
        }
        code.push(0xf4);
        let dir = TempDir::new("od-phase-log");
        let log = dir.0.join("phases.jsonl");
        let mut journal = RunJournal::new(JournalConfig {
            phase_log: Some(log.clone()),
            frames: None,
        })
        .unwrap();
        let mut board = load_firmware_image(
            &rom(0x40, &[(0x40, &code)]),
            &FirmwareBoardConfig::default(),
        )
        .unwrap();
        let report = board
            .run_boot_with_quantum_hook(100_000, |board, progress| {
                journal.at_boundary(board, progress)
            })
            .unwrap();
        let summary = journal.finish(
            &board,
            StopInfo {
                exit: "halted".to_owned(),
                stop_reason: Value::Null,
                progress: Some(QuantumProgress {
                    retired: report.retired,
                    quanta: report.quanta,
                }),
            },
        );
        assert_eq!(summary["phase_log_error"], Value::Null);
        let events: Vec<Value> = fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let names: Vec<_> = events
            .iter()
            .map(|event| event["event"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "first_com1_write",
                "com1_marker",
                "com1_marker_unrecognized",
                "stop"
            ]
        );
        assert_eq!(events[1]["nonce"], NONCE);
        assert_eq!(events[1]["phase"], "boot");
        assert_eq!(events[1]["status"], "ok");
        assert_eq!(events[1]["sha256"], SHA);
        assert_eq!(events[1]["evidence"], false);
        assert_eq!(events[2]["line"], "OD8 broken");
        assert_eq!(events[3]["exit"], "halted");
        assert_eq!(events[3]["instructions"], report.retired);
        for event in &events {
            assert_eq!(event["restart"], 0);
            assert!(event["host_unix_ms"].as_u64().unwrap() > 0);
            assert!(event["host_elapsed_ns"].is_u64());
            assert_eq!(event["virtual_ns"].as_u64().is_some(), true);
        }
        // Counters never go backwards, and the first byte is seen before the run's end.
        let instructions: Vec<u64> = events
            .iter()
            .map(|e| e["instructions"].as_u64().unwrap())
            .collect();
        assert!(
            instructions.windows(2).all(|pair| pair[0] <= pair[1]),
            "{instructions:?}"
        );
        assert!(instructions[0] < report.retired);
    }
}
