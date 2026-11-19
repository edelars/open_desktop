//! Host samples at publication of the two guest compute marker lines on COM1.

use od_machine::LoadedFirmwareBoard;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const MAX_LINE_BYTES: usize = 256;

struct Sample {
    wall_before_ns: u64,
    wall_after_ns: u64,
    process_cpu_ns: u64,
}

impl Sample {
    fn midpoint_ns(&self) -> u64 {
        self.wall_before_ns + (self.wall_after_ns - self.wall_before_ns) / 2
    }
}

#[derive(Default)]
enum State {
    #[default]
    Incomplete,
    Started(Sample),
    Finished {
        begin: Sample,
        end: Sample,
    },
    Invalid(String),
}

pub(super) struct Capture(Arc<Mutex<State>>);

impl Capture {
    pub(super) fn install(board: &mut LoadedFirmwareBoard, nonce: &str, compute: &str) -> Self {
        let capture = Self(Arc::new(Mutex::new(State::Incomplete)));
        let mut reader = Reader::new(nonce, compute, Arc::clone(&capture.0));
        let origin = Instant::now();
        board.set_com1_observer(move |byte| reader.observe(byte, || host_sample(origin)));
        capture
    }

    pub(super) fn report(&self) -> Value {
        let state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut report = match &*state {
            State::Incomplete | State::Started(_) => json!({"status": "incomplete"}),
            State::Invalid(detail) => json!({"status": "invalid", "detail": detail}),
            State::Finished { begin, end } => json!({
                "status": "sampled",
                "host_elapsed_ns": end.midpoint_ns() - begin.midpoint_ns(),
                "host_process_cpu_ns": end.process_cpu_ns - begin.process_cpu_ns,
                "begin_wall_bounds_ns": [begin.wall_before_ns, begin.wall_after_ns],
                "end_wall_bounds_ns": [end.wall_before_ns, end.wall_after_ns],
            }),
        };
        report["boundary"] = json!("com1_line_publication");
        report
    }
}

struct Reader {
    begin_line: String,
    end_line: String,
    line: Vec<u8>,
    overflow: bool,
    state: Arc<Mutex<State>>,
}

impl Reader {
    fn new(nonce: &str, compute: &str, state: Arc<Mutex<State>>) -> Self {
        let mut fields = compute.split(',');
        let workers = fields.next().expect("validated worker count");
        let blocks = fields.next().expect("validated block count");
        Self {
            begin_line: format!("OD_S6_38_BEGIN nonce={nonce} workers={workers} blocks={blocks}"),
            end_line: format!("OD_S6_38_END nonce={nonce}"),
            line: Vec::with_capacity(MAX_LINE_BYTES),
            overflow: false,
            state,
        }
    }

    fn observe(&mut self, byte: u8, sample: impl FnOnce() -> Result<Sample, String>) {
        if byte != b'\n' {
            if self.line.len() < MAX_LINE_BYTES {
                self.line.push(byte);
            } else {
                self.overflow = true;
            }
            return;
        }
        let line = self.line.strip_suffix(b"\r").unwrap_or(&self.line);
        let begin = !self.overflow && line == self.begin_line.as_bytes();
        let end = !self.overflow && line == self.end_line.as_bytes();
        self.line.clear();
        self.overflow = false;
        if !begin && !end {
            return;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *state = match (begin, std::mem::take(&mut *state)) {
            (true, State::Incomplete) => match sample() {
                Ok(mark) => State::Started(mark),
                Err(detail) => State::Invalid(detail),
            },
            (false, State::Started(begin)) => match sample() {
                Ok(end)
                    if end.wall_before_ns >= begin.wall_after_ns
                        && end.process_cpu_ns >= begin.process_cpu_ns =>
                {
                    State::Finished { begin, end }
                }
                Ok(_) => State::Invalid("host clocks moved backwards".into()),
                Err(detail) => State::Invalid(detail),
            },
            (_, invalid @ State::Invalid(_)) => invalid,
            _ => State::Invalid("compute timing markers repeated or out of order".into()),
        };
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn host_sample(origin: Instant) -> Result<Sample, String> {
    use std::time::Duration;
    let ns = |duration: Duration| {
        u64::try_from(duration.as_nanos()).map_err(|_| "host clock out of range".to_string())
    };
    let wall_before_ns = ns(origin.elapsed())?;
    let cpu = od_host::process_clock::process_cpu_time();
    let wall_after_ns = ns(origin.elapsed())?;
    let process_cpu_ns = ns(cpu.map_err(|error| error.to_string())?)?;
    Ok(Sample {
        wall_before_ns,
        wall_after_ns,
        process_cpu_ns,
    })
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn host_sample(_origin: Instant) -> Result<Sample, String> {
    Err("process CPU timing requires macOS arm64".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn process_clock_samples_are_monotonic() {
        let origin = Instant::now();
        let first = host_sample(origin).unwrap();
        let second = host_sample(origin).unwrap();
        assert!(first.wall_before_ns <= first.wall_after_ns);
        assert!(first.wall_after_ns <= second.wall_before_ns);
        assert!(second.wall_before_ns <= second.wall_after_ns);
        assert!(first.process_cpu_ns <= second.process_cpu_ns);
    }

    fn reader() -> (Reader, Capture) {
        let capture = Capture(Arc::new(Mutex::new(State::Incomplete)));
        let reader = Reader::new("test", "2,4097,ec4675d8578a4e14", Arc::clone(&capture.0));
        (reader, capture)
    }

    #[test]
    fn complete_markers_sample_exactly_twice_across_serial_chunks() {
        let (mut reader, capture) = reader();
        let mut samples = [
            Sample {
                wall_before_ns: 100,
                wall_after_ns: 104,
                process_cpu_ns: 45,
            },
            Sample {
                wall_before_ns: 600,
                wall_after_ns: 608,
                process_cpu_ns: 950,
            },
        ]
        .into_iter();
        for chunk in [
            "OD_S6_38_END nonce=other\nOD_S6_38_BE",
            "GIN nonce=test workers=2 blocks=4097\r",
            "\nnoise\nOD_S6_38_END nonce=test",
            "\r\n",
        ] {
            for byte in chunk.bytes() {
                reader.observe(byte, || Ok(samples.next().expect("at most two samples")));
            }
        }
        assert!(samples.next().is_none());
        let report = capture.report();
        assert_eq!(report["status"], "sampled");
        assert_eq!(report["host_elapsed_ns"], 502);
        assert_eq!(report["host_process_cpu_ns"], 905);
        assert_eq!(report["begin_wall_bounds_ns"], json!([100, 104]));
        assert_eq!(report["end_wall_bounds_ns"], json!([600, 608]));
    }

    #[test]
    fn rejects_out_of_order_and_overlong_or_incomplete_lines() {
        let (mut reader, capture) = reader();
        let mut overlong = vec![b'x'; MAX_LINE_BYTES];
        overlong.extend(b"OD_S6_38_END nonce=test\n");
        for byte in overlong
            .into_iter()
            .chain(b"OD_S6_38_END nonce=test".iter().copied())
        {
            reader.observe(byte, || panic!("invalid line cannot sample a clock"));
        }
        assert_eq!(capture.report()["status"], "incomplete");
        reader.observe(b'\n', || panic!("END without BEGIN cannot sample a clock"));
        assert_eq!(capture.report()["status"], "invalid");
    }

    #[test]
    fn clock_errors_and_duplicate_markers_do_not_supply_a_duration() {
        let start = b"OD_S6_38_BEGIN nonce=test workers=2 blocks=4097\n";
        let stop = b"OD_S6_38_END nonce=test\n";
        let (mut input, capture) = reader();
        for byte in start {
            input.observe(*byte, || Err("process CPU clock unavailable".into()));
        }
        assert_eq!(capture.report()["detail"], "process CPU clock unavailable");
        assert!(capture.report().get("host_elapsed_ns").is_none());

        let (mut input, capture) = reader();
        let mut ticks = 0;
        for byte in start.iter().chain(stop).chain(stop) {
            input.observe(*byte, || {
                ticks += 1;
                Ok(Sample {
                    wall_before_ns: ticks,
                    wall_after_ns: ticks,
                    process_cpu_ns: ticks,
                })
            });
        }
        assert_eq!(ticks, 2);
        assert_eq!(capture.report()["status"], "invalid");
        assert!(capture.report().get("host_process_cpu_ns").is_none());
    }
}
