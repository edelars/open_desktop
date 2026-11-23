//! PERF-22: the checking mode "fast path against the general path" (ADR-0017, section 5).
//!
//! The general path of one step runs for real on the guest memory through [`CheckMemory`] in
//! recording mode: every call of the [`GuestMemory`] boundary (except instruction fetches, which
//! only serve the decoder) is appended to a [`MemoryLog`] with its method, address, access
//! context, width, written bytes or scalars, and its result. The checker then runs the fast path
//! on a *copy* of the CPU state taken before the step, over a [`CheckMemory`] in replay mode:
//! it answers every call from the log (it never touches RAM or a device) and records the calls the
//! fast path actually makes. The two runs must agree on the whole [`LongModeState`], on the
//! architectural definedness mask of RFLAGS, on the step outcome and on the complete sequence of
//! memory calls (reads included, in order). The first disagreement is a [`FastPathDivergence`];
//! the run stops there.
//!
//! Devices therefore see every access exactly once (the general path makes it, the replay does
//! not reach them), so MMIO instructions are compared by the same mechanism. Ports and the
//! profile-owned cells (MSRs, the x87 half of the FP member, CR3 effect cells) are outside the
//! log: a fast path that admits an instruction must not touch them (ADR-0017, section 4).
//!
//! The mode is diagnostic and is selected by a const generic of the run loop: a run without it
//! has no check for it. State of the mode (counters, the last divergence, the fast path to
//! compare) is thread local, because tests run in parallel and one board runs on one thread.

use std::cell::{Cell, RefCell};
use std::fmt::{self, Write as _};

use crate::execute::StepOutcome;
use crate::long_decode::{self, Instruction};
use crate::memory::{AccessContext, FenceKind, GuestMemory, MemoryError};
use crate::{LongModeState, OperandSize};

/// A guest memory method of the [`GuestMemory`] boundary that is logged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryMethod {
    Read,
    Write,
    ReadInteger,
    WriteInteger,
    ReadIntegerForWrite,
    ReadForWriteInteger,
    AtomicExchange,
    AtomicFetchAdd,
    AtomicCompareExchange,
    AtomicRmwInteger,
    AtomicRmwU128,
    PreflightDescriptorAccessedWrite,
    Mfence,
    Fence,
}

/// One logged call. `values` and the data range hold the per-method scalars and bytes:
///
/// - `Read`: data = the bytes read (empty on error); `Write`: data = the bytes offered;
/// - `ReadInteger`, `ReadIntegerForWrite`, `ReadForWriteInteger`: `values[0]` = the value read;
/// - `WriteInteger`: `values[0]` = the value written;
/// - `AtomicExchange`: `[replacement, old, 0]`; `AtomicFetchAdd`: `[addend, old, 0]`;
///   `AtomicCompareExchange`: `[expected, replacement, old]`;
/// - `AtomicRmwInteger`: `[old, stored, 0]` (`stored` is what `update` returned for `old`);
/// - `AtomicRmwU128`: data = the old value then the stored value, little endian, 16 bytes each;
/// - `PreflightDescriptorAccessedWrite`: data = the `(address, length)` pairs, little endian;
/// - `Fence`: `values[0]` = 0 full, 1 store, 2 load; `Mfence`: nothing.
///
/// Results are zero when `error` is set.
#[derive(Clone, Debug)]
pub struct MemoryCall {
    pub method: MemoryMethod,
    pub address: u64,
    pub context: AccessContext,
    /// Width in bytes: the slice length of `Read`/`Write`, the operand size of the integer methods.
    pub size: usize,
    pub values: [u64; 3],
    data_start: usize,
    data_len: usize,
    pub error: Option<MemoryError>,
}

/// The calls of one step (and the instruction bytes the decoder fetched).
#[derive(Clone, Debug, Default)]
pub struct MemoryLog {
    calls: Vec<MemoryCall>,
    data: Vec<u8>,
    fetches: Vec<(u64, Result<u8, MemoryError>)>,
}

impl MemoryLog {
    pub fn calls(&self) -> &[MemoryCall] {
        &self.calls
    }

    /// Instruction fetches, in order. They serve the decoder and are not compared.
    pub fn fetches(&self) -> &[(u64, Result<u8, MemoryError>)] {
        &self.fetches
    }

    pub fn data_of(&self, call: &MemoryCall) -> &[u8] {
        &self.data[call.data_start..call.data_start + call.data_len]
    }

    fn clear(&mut self) {
        self.calls.clear();
        self.data.clear();
        self.fetches.clear();
    }

    fn push(
        &mut self,
        method: MemoryMethod,
        address: u64,
        context: AccessContext,
        size: usize,
        values: [u64; 3],
        data: &[u8],
        error: Option<MemoryError>,
    ) {
        let data_start = self.data.len();
        self.data.extend_from_slice(data);
        self.calls.push(MemoryCall {
            method,
            address,
            context,
            size,
            values,
            data_start,
            data_len: data.len(),
            error,
        });
    }

    /// One call as text, for a divergence report.
    pub fn describe(&self, index: usize) -> String {
        let Some(call) = self.calls.get(index) else {
            return format!("#{index} <no call>");
        };
        let mut text = format!(
            "#{index} {:?} address={:#x} size={} context={:?}/{:?}",
            call.method, call.address, call.size, call.context.privilege, call.context.origin
        );
        let data = self.data_of(call);
        if !data.is_empty() {
            let _ = write!(text, " data={}", hex(data));
        }
        if call.values != [0; 3] {
            let _ = write!(
                text,
                " values=[{:#x}, {:#x}, {:#x}]",
                call.values[0], call.values[1], call.values[2]
            );
        }
        if let Some(error) = &call.error {
            let _ = write!(text, " error={error:?}");
        }
        text
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

fn same_call(left: &MemoryLog, a: &MemoryCall, right: &MemoryLog, b: &MemoryCall) -> bool {
    a.method == b.method
        && a.address == b.address
        && a.context == b.context
        && a.size == b.size
        && a.values == b.values
        && a.error == b.error
        && left.data_of(a) == right.data_of(b)
}

/// The first call at which two logs differ, as a report line.
fn first_log_difference(general: &MemoryLog, fast: &MemoryLog) -> Option<String> {
    let longest = general.calls.len().max(fast.calls.len());
    for index in 0..longest {
        match (general.calls.get(index), fast.calls.get(index)) {
            (Some(a), Some(b)) if same_call(general, a, fast, b) => {}
            (Some(_), Some(_)) => {
                return Some(format!(
                    "memory call {}: general path {} | fast path {}",
                    index,
                    general.describe(index),
                    fast.describe(index)
                ));
            }
            (Some(_), None) => {
                return Some(format!(
                    "memory call {}: general path {} | fast path made no call (it made {} of {})",
                    index,
                    general.describe(index),
                    fast.calls.len(),
                    general.calls.len()
                ));
            }
            (None, Some(_)) => {
                return Some(format!(
                    "memory call {}: general path made no call (it made {}) | fast path {}",
                    index,
                    general.calls.len(),
                    fast.describe(index)
                ));
            }
            (None, None) => unreachable!("index is below the longer log"),
        }
    }
    None
}

fn unmatched_call() -> MemoryError {
    MemoryError::Backend(
        "fast-path check: the call does not match the next recorded call of the general path"
            .into(),
    )
}

/// A [`GuestMemory`] for the check. In recording mode it forwards every call to the wrapped
/// memory and logs it; in replay mode it answers from a recorded log, touches nothing and logs the
/// calls it receives. A call that is not the next recorded call (another method, address, context
/// or width) gets an error back and is still logged, so the later comparison names it.
///
/// `cached_instruction` and `remember_instruction` are not forwarded (like every wrapper): the
/// decoder reads the bytes through `fetch`, which the log keeps for the replayed decode.
pub struct CheckMemory<'a> {
    inner: Option<&'a mut dyn GuestMemory>,
    expected: MemoryLog,
    produced: MemoryLog,
    cursor: usize,
}

impl<'a> CheckMemory<'a> {
    /// Recording mode over `inner`; `buffer` is a cleared log to fill (reused between steps).
    pub fn record(inner: &'a mut dyn GuestMemory, mut buffer: MemoryLog) -> Self {
        buffer.clear();
        Self {
            inner: Some(inner),
            expected: MemoryLog::default(),
            produced: buffer,
            cursor: 0,
        }
    }

    /// Replay mode: answers from `expected`, logs what it is asked into `buffer`.
    pub fn replay(expected: MemoryLog, mut buffer: MemoryLog) -> CheckMemory<'static> {
        buffer.clear();
        CheckMemory {
            inner: None,
            expected,
            produced: buffer,
            cursor: 0,
        }
    }

    /// What this memory logged (the recording of the general path, or the calls of the replay).
    pub fn log(&self) -> &MemoryLog {
        &self.produced
    }

    /// Recording mode: the finished log.
    pub fn into_log(self) -> MemoryLog {
        self.produced
    }

    /// Replay mode: the recorded log and the log of the calls the replay received.
    pub fn into_parts(self) -> (MemoryLog, MemoryLog) {
        (self.expected, self.produced)
    }

    /// Replay: the index of the next recorded call when it has this identity.
    fn replayed(
        &mut self,
        method: MemoryMethod,
        address: u64,
        context: AccessContext,
        size: usize,
    ) -> Option<usize> {
        let call = self.expected.calls.get(self.cursor)?;
        if call.method == method
            && call.address == address
            && call.context == context
            && call.size == size
        {
            self.cursor += 1;
            Some(self.cursor - 1)
        } else {
            None
        }
    }

    fn recorded_error(&self, index: usize) -> Option<MemoryError> {
        self.expected.calls[index].error.clone()
    }

    fn integer_read(
        &mut self,
        method: MemoryMethod,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        live: impl FnOnce(&mut dyn GuestMemory) -> Result<u64, MemoryError>,
    ) -> Result<u64, MemoryError> {
        let result = match self.inner.as_deref_mut() {
            Some(inner) => live(inner),
            None => match self.replayed(method, address, context, size.bytes()) {
                Some(index) => match self.recorded_error(index) {
                    Some(error) => Err(error),
                    None => Ok(self.expected.calls[index].values[0]),
                },
                None => Err(unmatched_call()),
            },
        };
        let (value, error) = split(&result);
        self.produced.push(
            method,
            address,
            context,
            size.bytes(),
            [value, 0, 0],
            &[],
            error,
        );
        result
    }

    /// The three atomic specializations: `inputs` come first in `values`, the old value after them.
    fn atomic_call(
        &mut self,
        method: MemoryMethod,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        inputs: &[u64],
        live: impl FnOnce(&mut dyn GuestMemory) -> Result<u64, MemoryError>,
    ) -> Result<u64, MemoryError> {
        let result = match self.inner.as_deref_mut() {
            Some(inner) => live(inner),
            None => match self.replayed(method, address, context, size.bytes()) {
                Some(index) => match self.recorded_error(index) {
                    Some(error) => Err(error),
                    None => Ok(self.expected.calls[index].values[inputs.len()]),
                },
                None => Err(unmatched_call()),
            },
        };
        let (old, error) = split(&result);
        let mut values = [0; 3];
        values[..inputs.len()].copy_from_slice(inputs);
        values[inputs.len()] = old;
        self.produced
            .push(method, address, context, size.bytes(), values, &[], error);
        result
    }
}

fn split<T: Copy + Default>(result: &Result<T, MemoryError>) -> (T, Option<MemoryError>) {
    match result {
        Ok(value) => (*value, None),
        Err(error) => (T::default(), Some(error.clone())),
    }
}

impl GuestMemory for CheckMemory<'_> {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        match self.inner.as_deref_mut() {
            Some(inner) => {
                let result = inner.fetch(address, context);
                self.produced.fetches.push((address, result.clone()));
                result
            }
            None => self
                .expected
                .fetches
                .iter()
                .find(|(fetched, _)| *fetched == address)
                .map(|(_, result)| result.clone())
                .unwrap_or_else(|| Err(unmatched_call())),
        }
    }

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        let size = destination.len();
        let result = match self.inner.as_deref_mut() {
            Some(inner) => inner.read(address, context, destination),
            None => match self.replayed(MemoryMethod::Read, address, context, size) {
                Some(index) => match self.recorded_error(index) {
                    Some(error) => Err(error),
                    None => {
                        let call = &self.expected.calls[index];
                        destination.copy_from_slice(self.expected.data_of(call));
                        Ok(())
                    }
                },
                None => Err(unmatched_call()),
            },
        };
        let data: &[u8] = if result.is_ok() { destination } else { &[] };
        self.produced.push(
            MemoryMethod::Read,
            address,
            context,
            size,
            [0; 3],
            data,
            result.as_ref().err().cloned(),
        );
        result
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        source: &[u8],
    ) -> Result<(), MemoryError> {
        let result = match self.inner.as_deref_mut() {
            Some(inner) => inner.write(address, context, source),
            None => match self.replayed(MemoryMethod::Write, address, context, source.len()) {
                Some(index) => self.recorded_error(index).map_or(Ok(()), Err),
                None => Err(unmatched_call()),
            },
        };
        self.produced.push(
            MemoryMethod::Write,
            address,
            context,
            source.len(),
            [0; 3],
            source,
            result.as_ref().err().cloned(),
        );
        result
    }

    fn read_integer_for_write(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
    ) -> Result<u64, MemoryError> {
        self.integer_read(
            MemoryMethod::ReadIntegerForWrite,
            address,
            context,
            size,
            |inner| inner.read_integer_for_write(address, context, size),
        )
    }

    fn preflight_descriptor_accessed_write(
        &mut self,
        descriptor_access_address: u64,
        descriptor_context: AccessContext,
        stack_writes: &[(u64, usize)],
    ) -> Result<(), MemoryError> {
        let mut pairs = Vec::with_capacity(stack_writes.len() * 16);
        for (address, length) in stack_writes {
            pairs.extend_from_slice(&address.to_le_bytes());
            pairs.extend_from_slice(&(*length as u64).to_le_bytes());
        }
        let result = match self.inner.as_deref_mut() {
            Some(inner) => inner.preflight_descriptor_accessed_write(
                descriptor_access_address,
                descriptor_context,
                stack_writes,
            ),
            None => match self.replayed(
                MemoryMethod::PreflightDescriptorAccessedWrite,
                descriptor_access_address,
                descriptor_context,
                stack_writes.len(),
            ) {
                Some(index) => self.recorded_error(index).map_or(Ok(()), Err),
                None => Err(unmatched_call()),
            },
        };
        self.produced.push(
            MemoryMethod::PreflightDescriptorAccessedWrite,
            descriptor_access_address,
            descriptor_context,
            stack_writes.len(),
            [0; 3],
            &pairs,
            result.as_ref().err().cloned(),
        );
        result
    }

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        self.atomic_call(
            MemoryMethod::AtomicExchange,
            address,
            context,
            size,
            &[replacement],
            |inner| inner.atomic_exchange_integer(address, context, size, replacement),
        )
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        addend: u64,
    ) -> Result<u64, MemoryError> {
        self.atomic_call(
            MemoryMethod::AtomicFetchAdd,
            address,
            context,
            size,
            &[addend],
            |inner| inner.atomic_fetch_add_integer(address, context, size, addend),
        )
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        expected: u64,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        self.atomic_call(
            MemoryMethod::AtomicCompareExchange,
            address,
            context,
            size,
            &[expected, replacement],
            |inner| {
                inner.atomic_compare_exchange_integer(address, context, size, expected, replacement)
            },
        )
    }

    fn atomic_rmw_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        update: &mut dyn FnMut(u64) -> u64,
    ) -> Result<u64, MemoryError> {
        let mut stored = 0;
        let result = match self.inner.as_deref_mut() {
            Some(inner) => inner.atomic_rmw_integer(address, context, size, &mut |old| {
                stored = update(old);
                stored
            }),
            None => match self.replayed(
                MemoryMethod::AtomicRmwInteger,
                address,
                context,
                size.bytes(),
            ) {
                Some(index) => match self.recorded_error(index) {
                    Some(error) => Err(error),
                    None => {
                        let old = self.expected.calls[index].values[0];
                        stored = update(old);
                        Ok(old)
                    }
                },
                None => Err(unmatched_call()),
            },
        };
        let (old, error) = split(&result);
        let stored = if error.is_some() { 0 } else { stored };
        self.produced.push(
            MemoryMethod::AtomicRmwInteger,
            address,
            context,
            size.bytes(),
            [old, stored, 0],
            &[],
            error,
        );
        result
    }

    fn atomic_rmw_u128(
        &mut self,
        address: u64,
        context: AccessContext,
        update: &mut dyn FnMut(u128) -> u128,
    ) -> Result<u128, MemoryError> {
        let mut stored = 0u128;
        let result = match self.inner.as_deref_mut() {
            Some(inner) => inner.atomic_rmw_u128(address, context, &mut |old| {
                stored = update(old);
                stored
            }),
            None => match self.replayed(MemoryMethod::AtomicRmwU128, address, context, 16) {
                Some(index) => match self.recorded_error(index) {
                    Some(error) => Err(error),
                    None => {
                        let call = &self.expected.calls[index];
                        let mut old = [0u8; 16];
                        old.copy_from_slice(&self.expected.data_of(call)[..16]);
                        let old = u128::from_le_bytes(old);
                        stored = update(old);
                        Ok(old)
                    }
                },
                None => Err(unmatched_call()),
            },
        };
        let (old, error) = split(&result);
        let mut data = [0u8; 32];
        if error.is_none() {
            data[..16].copy_from_slice(&old.to_le_bytes());
            data[16..].copy_from_slice(&stored.to_le_bytes());
        }
        self.produced.push(
            MemoryMethod::AtomicRmwU128,
            address,
            context,
            16,
            [0; 3],
            if error.is_none() { &data } else { &[] },
            error,
        );
        result
    }

    fn mfence(&mut self, context: AccessContext) -> Result<(), MemoryError> {
        let result = match self.inner.as_deref_mut() {
            Some(inner) => inner.mfence(context),
            None => match self.replayed(MemoryMethod::Mfence, 0, context, 0) {
                Some(index) => self.recorded_error(index).map_or(Ok(()), Err),
                None => Err(unmatched_call()),
            },
        };
        self.produced.push(
            MemoryMethod::Mfence,
            0,
            context,
            0,
            [0; 3],
            &[],
            result.as_ref().err().cloned(),
        );
        result
    }

    fn fence(&mut self, kind: FenceKind) -> Result<(), MemoryError> {
        let context = AccessContext::default();
        let code = match kind {
            FenceKind::Full => 0,
            FenceKind::Store => 1,
            FenceKind::Load => 2,
        };
        let result = match self.inner.as_deref_mut() {
            Some(inner) => inner.fence(kind),
            None => match self.replayed(MemoryMethod::Fence, 0, context, 0) {
                Some(index) if self.expected.calls[index].values[0] == code => {
                    self.recorded_error(index).map_or(Ok(()), Err)
                }
                _ => Err(unmatched_call()),
            },
        };
        self.produced.push(
            MemoryMethod::Fence,
            0,
            context,
            0,
            [code, 0, 0],
            &[],
            result.as_ref().err().cloned(),
        );
        result
    }

    fn read_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
    ) -> Result<u64, MemoryError> {
        self.integer_read(MemoryMethod::ReadInteger, address, context, size, |inner| {
            inner.read_integer(address, context, size)
        })
    }

    fn read_for_write_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
    ) -> Result<u64, MemoryError> {
        self.integer_read(
            MemoryMethod::ReadForWriteInteger,
            address,
            context,
            size,
            |inner| inner.read_for_write_integer(address, context, size),
        )
    }

    fn write_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        value: u64,
    ) -> Result<(), MemoryError> {
        let result = match self.inner.as_deref_mut() {
            Some(inner) => inner.write_integer(address, context, size, value),
            None => match self.replayed(MemoryMethod::WriteInteger, address, context, size.bytes())
            {
                Some(index) => self.recorded_error(index).map_or(Ok(()), Err),
                None => Err(unmatched_call()),
            },
        };
        self.produced.push(
            MemoryMethod::WriteInteger,
            address,
            context,
            size.bytes(),
            [value, 0, 0],
            &[],
            result.as_ref().err().cloned(),
        );
        result
    }
}

/// The signature of a fast path under check: the state copy, the replaying memory, the access
/// context, `rflags_defined`, the decoded instruction and whether the CPL0 control policy is
/// present (ADR-0017, section 3). `None` is "not handled".
pub type FastStepResult = Option<Result<StepOutcome, MemoryError>>;

/// A draft fast path for the [`FastPathMode::Custom`] mode (tests, the mutation check).
pub type DraftFastStep = fn(
    &mut LongModeState,
    &mut CheckMemory<'_>,
    AccessContext,
    &mut u64,
    &Instruction,
    bool,
) -> FastStepResult;

/// Which fast path the checker compares the general path with.
#[derive(Clone, Copy, Debug, Default)]
pub enum FastPathMode {
    /// `execute::try_fast_step`, the fast path of the tree (empty until PERF-23).
    #[default]
    Tree,
    /// Self-test of the instrument: the general step itself plays the fast path (ADR-0017,
    /// section 5, item 7). Valid only where a step has no effect outside the CPU state and the
    /// memory log (no ports, no profile cells that a second execution would change, no RDTSC
    /// source): the corpora of the plain and CPL0 sets without such members.
    GeneralPath,
    /// A draft handler under test.
    Custom(DraftFastStep),
}

/// What one checked run (or a thread, since the last [`reset`]) did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CheckCounters {
    /// Instructions the fast path took and whose results were compared.
    pub compared: u64,
    /// Decoded instructions the fast path refused (`None`).
    pub refused_by_admission: u64,
    /// Steps that fetched nothing: they ended before decoding (halted, refused by a precondition).
    pub before_decode: u64,
    /// Steps whose recorded bytes did not decode again (a decode error of the general path).
    pub not_decoded: u64,
    /// Memory calls compared in the compared instructions.
    pub memory_calls_compared: u64,
    /// Divergences found (the run stops at the first).
    pub divergences: u64,
}

/// The first disagreement of the two paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FastPathDivergence {
    pub rip: u64,
    /// The bytes of the instruction (up to its decoded length), from the fetches of the step.
    pub bytes: Vec<u8>,
    /// The decoded instruction (`Debug`).
    pub instruction: String,
    /// One line per differing item: outcome, `rflags_defined`, state fields, the first memory call.
    pub differences: Vec<String>,
}

impl fmt::Display for FastPathDivergence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "fast-path check divergence at rip={:#x} bytes={} instruction={}",
            self.rip,
            hex(&self.bytes),
            self.instruction
        )?;
        for line in &self.differences {
            write!(formatter, "; {line}")?;
        }
        Ok(())
    }
}

thread_local! {
    static MODE: Cell<FastPathMode> = const { Cell::new(FastPathMode::Tree) };
    static TOTALS: Cell<CheckCounters> = const { Cell::new(CheckCounters {
        compared: 0,
        refused_by_admission: 0,
        before_decode: 0,
        not_decoded: 0,
        memory_calls_compared: 0,
        divergences: 0,
    }) };
    static LAST_DIVERGENCE: RefCell<Option<FastPathDivergence>> = const { RefCell::new(None) };
    static BUFFERS: RefCell<Vec<MemoryLog>> = const { RefCell::new(Vec::new()) };
}

#[cfg(debug_assertions)]
thread_local! {
    static FAST_STEPS_TAKEN: Cell<u64> = const { Cell::new(0) };
}

/// PERF-23: whether [`fast_steps_taken`] counts. The count is kept in debug builds only, so the
/// step of a release build carries no counter.
pub const FAST_STEPS_COUNTED: bool = cfg!(debug_assertions);

/// Debug builds: one more step of this thread was taken by the fast path of the step
/// (`Cpl0ProfileSet::with_fast_path`); the replay of the check does not count.
#[cfg(debug_assertions)]
pub(crate) fn note_fast_step_taken() {
    FAST_STEPS_TAKEN.with(|cell| cell.set(cell.get() + 1));
}

/// PERF-23: the steps of this thread taken by the fast path of the step since the last
/// [`reset_fast_steps_taken`]; always 0 when [`FAST_STEPS_COUNTED`] is false (release builds).
pub fn fast_steps_taken() -> u64 {
    #[cfg(debug_assertions)]
    {
        FAST_STEPS_TAKEN.with(Cell::get)
    }
    #[cfg(not(debug_assertions))]
    {
        0
    }
}

/// Clears [`fast_steps_taken`] of this thread.
pub fn reset_fast_steps_taken() {
    #[cfg(debug_assertions)]
    FAST_STEPS_TAKEN.with(|cell| cell.set(0));
}

/// Selects the fast path for the checks of this thread.
pub fn set_mode(mode: FastPathMode) {
    MODE.with(|cell| cell.set(mode));
}

/// The mode of this thread.
pub fn mode() -> FastPathMode {
    MODE.with(Cell::get)
}

/// Clears the counters and the stored divergence of this thread.
pub fn reset() {
    TOTALS.with(|cell| cell.set(CheckCounters::default()));
    LAST_DIVERGENCE.with(|cell| *cell.borrow_mut() = None);
}

/// The counters of this thread, summed over its finished checked runs.
pub fn counters() -> CheckCounters {
    TOTALS.with(Cell::get)
}

/// Takes the first divergence found on this thread.
pub fn take_divergence() -> Option<FastPathDivergence> {
    LAST_DIVERGENCE.with(|cell| cell.borrow_mut().take())
}

/// The state of one checked run: counters, the buffers reused between steps, the divergence.
pub struct FastPathChecker {
    mode: FastPathMode,
    record_buffer: MemoryLog,
    replay_buffer: MemoryLog,
    counters: CheckCounters,
    divergence: Option<FastPathDivergence>,
    reported: Option<String>,
}

impl Default for FastPathChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl FastPathChecker {
    pub fn new() -> Self {
        let (record_buffer, replay_buffer) = BUFFERS.with(|pool| {
            let mut pool = pool.borrow_mut();
            (
                pool.pop().unwrap_or_default(),
                pool.pop().unwrap_or_default(),
            )
        });
        Self {
            mode: mode(),
            record_buffer,
            replay_buffer,
            counters: CheckCounters::default(),
            divergence: None,
            reported: None,
        }
    }

    pub fn mode(&self) -> FastPathMode {
        self.mode
    }

    pub fn counters(&self) -> CheckCounters {
        self.counters
    }

    /// The text of the divergence this run found, if any.
    pub fn failure(&self) -> Option<&str> {
        self.reported.as_deref()
    }

    /// Starts the recording of the general path over `inner`.
    pub fn record<'a>(&mut self, inner: &'a mut dyn GuestMemory) -> CheckMemory<'a> {
        CheckMemory::record(inner, std::mem::take(&mut self.record_buffer))
    }

    /// Compares the fast path with the step that just ran.
    ///
    /// `recording` is the memory of the general step; `before` and `defined_before` are the CPU
    /// state and the definedness mask at its start (the fast path runs on them, so they are
    /// consumed); `after`, `defined_after` and `outcome` are what the general path left.
    /// `fast` receives the state copy, a replaying memory, the context, the mask and the decoded
    /// instruction. Nothing is compared for a step that fetched nothing (it ended before decoding)
    /// or whose bytes do not decode, nor for an instruction the fast path refuses (`None`).
    #[allow(clippy::too_many_arguments)]
    pub fn finish(
        &mut self,
        recording: CheckMemory<'_>,
        before: LongModeState,
        defined_before: u64,
        context: AccessContext,
        after: &LongModeState,
        defined_after: u64,
        outcome: &Result<StepOutcome, MemoryError>,
        fast: impl FnOnce(
            &mut LongModeState,
            &mut CheckMemory<'_>,
            AccessContext,
            &mut u64,
            &Instruction,
        ) -> FastStepResult,
    ) {
        let expected = recording.into_log();
        if self.divergence.is_some() || expected.fetches.is_empty() {
            if expected.fetches.is_empty() {
                self.counters.before_decode += 1;
            }
            self.record_buffer = expected;
            return;
        }
        let rip = before.rip;
        // The decode of the general path again, from the bytes it fetched.
        let mut decoder = CheckMemory::replay(expected, std::mem::take(&mut self.replay_buffer));
        let decoded = long_decode::decode(&mut decoder, rip, context);
        let (expected, spare) = decoder.into_parts();
        self.replay_buffer = spare;
        let instruction = match decoded {
            Ok(instruction) => instruction,
            Err(_) => {
                self.counters.not_decoded += 1;
                self.record_buffer = expected;
                return;
            }
        };
        let mut fast_state = before.clone();
        let mut fast_defined = defined_before;
        let mut replay = CheckMemory::replay(expected, std::mem::take(&mut self.replay_buffer));
        let fast_result = fast(
            &mut fast_state,
            &mut replay,
            context,
            &mut fast_defined,
            &instruction,
        );
        let (expected, fast_log) = replay.into_parts();
        let mut differences = Vec::new();
        match fast_result {
            None => {
                // "Not handled" must leave no trace: the state, the mask and the memory calls
                // stay as they were (ADR-0017, section 3: refuse before the first side effect).
                if fast_state == before
                    && fast_defined == defined_before
                    && fast_log.calls().is_empty()
                {
                    self.counters.refused_by_admission += 1;
                    self.record_buffer = expected;
                    self.replay_buffer = fast_log;
                    return;
                }
                let first_call = fast_log
                    .calls()
                    .first()
                    .map_or_else(|| "none".to_owned(), |call| format!("{call:?}"));
                differences.push(format!(
                    "admission: None after a side effect (state changed: {}, rflags_defined {defined_before:#x} -> {fast_defined:#x}, {} memory calls, first call: {first_call})",
                    fast_state != before,
                    fast_log.calls().len()
                ));
                differences.extend(state_differences(&before, &fast_state));
            }
            Some(fast_outcome) => {
                if *outcome != fast_outcome {
                    differences.push(format!(
                        "outcome: general path {outcome:?} | fast path {fast_outcome:?}"
                    ));
                }
                if defined_after != fast_defined {
                    differences.push(format!(
                        "rflags_defined: general path {defined_after:#x} | fast path {fast_defined:#x}"
                    ));
                }
                if *after != fast_state {
                    differences.extend(state_differences(after, &fast_state));
                }
                if let Some(line) = first_log_difference(&expected, &fast_log) {
                    differences.push(line);
                }
            }
        }
        if differences.is_empty() {
            self.counters.compared += 1;
            self.counters.memory_calls_compared += expected.calls.len() as u64;
        } else {
            self.counters.divergences += 1;
            let length = usize::from(instruction.length);
            let bytes = (0..length as u64)
                .map_while(|offset| {
                    expected
                        .fetches
                        .iter()
                        .find(|(address, _)| *address == rip.wrapping_add(offset))
                        .and_then(|(_, byte)| byte.as_ref().ok().copied())
                })
                .collect();
            let divergence = FastPathDivergence {
                rip,
                bytes,
                instruction: format!("{instruction:?}"),
                differences,
            };
            self.reported = Some(divergence.to_string());
            self.divergence = Some(divergence);
        }
        self.record_buffer = expected;
        self.replay_buffer = fast_log;
    }
}

impl Drop for FastPathChecker {
    fn drop(&mut self) {
        TOTALS.with(|cell| {
            let mut totals = cell.get();
            totals.compared += self.counters.compared;
            totals.refused_by_admission += self.counters.refused_by_admission;
            totals.before_decode += self.counters.before_decode;
            totals.not_decoded += self.counters.not_decoded;
            totals.memory_calls_compared += self.counters.memory_calls_compared;
            totals.divergences += self.counters.divergences;
            cell.set(totals);
        });
        if let Some(divergence) = self.divergence.take() {
            LAST_DIVERGENCE.with(|cell| {
                let mut slot = cell.borrow_mut();
                if slot.is_none() {
                    *slot = Some(divergence);
                }
            });
        }
        BUFFERS.with(|pool| {
            let mut pool = pool.borrow_mut();
            if pool.is_empty() {
                pool.push(std::mem::take(&mut self.record_buffer));
                pool.push(std::mem::take(&mut self.replay_buffer));
            }
        });
    }
}

/// The differing fields of two states, one line each, found through the pretty `Debug` text (the
/// layout of both is the same; a field of another shape is reported as a whole).
fn state_differences(general: &LongModeState, fast: &LongModeState) -> Vec<String> {
    const LIMIT: usize = 24;
    let general_text = format!("{general:#?}");
    let fast_text = format!("{fast:#?}");
    let mut field = String::from("?");
    let mut lines = Vec::new();
    let mut fast_lines = fast_text.lines();
    for general_line in general_text.lines() {
        let fast_line = fast_lines.next().unwrap_or("<missing>");
        let indent = general_line.len() - general_line.trim_start().len();
        if indent == 4 {
            if let Some((name, _)) = general_line.trim_start().split_once(':') {
                field = name.to_owned();
            }
        }
        if general_line != fast_line {
            lines.push(format!(
                "state.{field}: general path `{}` | fast path `{}`",
                general_line.trim(),
                fast_line.trim()
            ));
        }
    }
    if fast_lines.next().is_some() {
        lines.push("state: the fast path state prints more lines than the general one".into());
    }
    if lines.is_empty() {
        lines.push("state: the states differ but print the same".into());
    }
    if lines.len() > LIMIT {
        let more = lines.len() - LIMIT;
        lines.truncate(LIMIT);
        lines.push(format!("... and {more} more differing lines"));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::AccessKind;

    /// A memory that counts what reaches it (the "device").
    #[derive(Default)]
    struct Device {
        seen: Vec<String>,
        ram: Vec<u8>,
    }

    impl Device {
        fn new() -> Self {
            Self {
                seen: Vec::new(),
                ram: (0..64).collect(),
            }
        }

        fn slot(&self, address: u64, len: usize, access: AccessKind) -> Result<usize, MemoryError> {
            match usize::try_from(address) {
                Ok(start) if start + len <= self.ram.len() => Ok(start),
                _ => Err(MemoryError::Unmapped { address, access }),
            }
        }
    }

    impl GuestMemory for Device {
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            self.seen.push(format!("fetch {address:#x}"));
            Ok(0x90)
        }

        fn read(
            &mut self,
            address: u64,
            _: AccessContext,
            destination: &mut [u8],
        ) -> Result<(), MemoryError> {
            self.seen
                .push(format!("read {address:#x} {}", destination.len()));
            let start = self.slot(address, destination.len(), AccessKind::Read)?;
            destination.copy_from_slice(&self.ram[start..start + destination.len()]);
            Ok(())
        }

        fn write(
            &mut self,
            address: u64,
            _: AccessContext,
            source: &[u8],
        ) -> Result<(), MemoryError> {
            self.seen
                .push(format!("write {address:#x} {}", source.len()));
            let start = self.slot(address, source.len(), AccessKind::Write)?;
            self.ram[start..start + source.len()].copy_from_slice(source);
            Ok(())
        }

        fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
            self.seen.push("mfence".into());
            Ok(())
        }

        fn fence(&mut self, kind: FenceKind) -> Result<(), MemoryError> {
            self.seen.push(format!("fence {kind:?}"));
            Ok(())
        }

        fn read_integer(
            &mut self,
            address: u64,
            _: AccessContext,
            size: OperandSize,
        ) -> Result<u64, MemoryError> {
            self.seen
                .push(format!("read_integer {address:#x} {}", size.bytes()));
            let start = self.slot(address, size.bytes(), AccessKind::Read)?;
            let mut bytes = [0u8; 8];
            bytes[..size.bytes()].copy_from_slice(&self.ram[start..start + size.bytes()]);
            Ok(u64::from_le_bytes(bytes))
        }

        fn write_integer(
            &mut self,
            address: u64,
            _: AccessContext,
            size: OperandSize,
            value: u64,
        ) -> Result<(), MemoryError> {
            self.seen
                .push(format!("write_integer {address:#x} {}", size.bytes()));
            let start = self.slot(address, size.bytes(), AccessKind::Write)?;
            self.ram[start..start + size.bytes()]
                .copy_from_slice(&value.to_le_bytes()[..size.bytes()]);
            Ok(())
        }

        fn atomic_fetch_add_integer(
            &mut self,
            address: u64,
            _: AccessContext,
            size: OperandSize,
            addend: u64,
        ) -> Result<u64, MemoryError> {
            self.seen
                .push(format!("atomic_fetch_add {address:#x} {}", size.bytes()));
            let start = self.slot(address, size.bytes(), AccessKind::Write)?;
            let mut bytes = [0u8; 8];
            bytes[..size.bytes()].copy_from_slice(&self.ram[start..start + size.bytes()]);
            let old = u64::from_le_bytes(bytes);
            let new = old.wrapping_add(addend);
            self.ram[start..start + size.bytes()]
                .copy_from_slice(&new.to_le_bytes()[..size.bytes()]);
            Ok(old)
        }
    }

    fn context() -> AccessContext {
        AccessContext::default()
    }

    /// A fixed sequence of calls of every logged kind.
    fn exercise(memory: &mut impl GuestMemory) -> Vec<String> {
        let mut seen = Vec::new();
        let mut buffer = [0u8; 4];
        seen.push(format!("{:?}", memory.read(8, context(), &mut buffer)));
        seen.push(format!("{buffer:?}"));
        seen.push(format!("{:?}", memory.write(16, context(), &[1, 2, 3])));
        seen.push(format!(
            "{:?}",
            memory.read_integer(24, context(), OperandSize::Dword)
        ));
        seen.push(format!(
            "{:?}",
            memory.write_integer(32, context(), OperandSize::Word, 0xbeef)
        ));
        seen.push(format!(
            "{:?}",
            memory.atomic_fetch_add_integer(40, context(), OperandSize::Qword, 5)
        ));
        seen.push(format!("{:?}", memory.mfence(context())));
        seen.push(format!("{:?}", memory.fence(FenceKind::Store)));
        // An error result is part of the log too.
        seen.push(format!("{:?}", memory.read(4096, context(), &mut buffer)));
        seen.push(format!(
            "{:?}",
            memory.write_integer(4096, context(), OperandSize::Byte, 1)
        ));
        seen
    }

    #[test]
    fn recording_forwards_each_call_once_and_logs_it_in_order() {
        let mut device = Device::new();
        let mut direct = Device::new();
        let direct_results = exercise(&mut direct);
        let mut recording = CheckMemory::record(&mut device, MemoryLog::default());
        let recorded_results = exercise(&mut recording);
        let log = recording.into_log();
        assert_eq!(recorded_results, direct_results);
        // The wrapped memory saw exactly what a direct run shows: each call once.
        assert_eq!(device.seen, direct.seen);
        assert_eq!(device.seen.len(), 9);
        let methods: Vec<MemoryMethod> = log.calls().iter().map(|call| call.method).collect();
        assert_eq!(
            methods,
            vec![
                MemoryMethod::Read,
                MemoryMethod::Write,
                MemoryMethod::ReadInteger,
                MemoryMethod::WriteInteger,
                MemoryMethod::AtomicFetchAdd,
                MemoryMethod::Mfence,
                MemoryMethod::Fence,
                MemoryMethod::Read,
                MemoryMethod::WriteInteger,
            ]
        );
        assert_eq!(log.data_of(&log.calls()[0]), [8, 9, 10, 11]);
        assert_eq!(log.data_of(&log.calls()[1]), [1, 2, 3]);
        assert_eq!(log.calls()[2].values[0], 0x1b1a_1918);
        assert_eq!(log.calls()[3].values[0], 0xbeef);
        assert_eq!(log.calls()[4].values, [5, 0x2f2e_2d2c_2b2a_2928, 0]);
        assert_eq!(log.calls()[6].values[0], 1);
        assert!(log.calls()[7].error.is_some() && log.data_of(&log.calls()[7]).is_empty());
        assert!(log.calls()[8].error.is_some());
    }

    #[test]
    fn replay_answers_from_the_log_and_never_reaches_the_memory() {
        let mut device = Device::new();
        let mut recording = CheckMemory::record(&mut device, MemoryLog::default());
        let recorded_results = exercise(&mut recording);
        let log = recording.into_log();
        let seen_after_recording = device.seen.clone();

        let mut replay = CheckMemory::replay(log, MemoryLog::default());
        let replayed_results = exercise(&mut replay);
        assert_eq!(replayed_results, recorded_results);
        let (expected, produced) = replay.into_parts();
        assert_eq!(first_log_difference(&expected, &produced), None);
        // Replay has no memory to reach; the device saw only the recording's calls.
        assert_eq!(device.seen, seen_after_recording);
    }

    fn recorded_sequence() -> MemoryLog {
        let mut device = Device::new();
        let mut recording = CheckMemory::record(&mut device, MemoryLog::default());
        let _ = recording.read_integer(24, context(), OperandSize::Dword);
        let _ = recording.write(16, context(), &[1, 2, 3]);
        recording.into_log()
    }

    #[test]
    fn replay_names_the_first_differing_call() {
        // Another address: the replay refuses the call and the logs differ at call 0.
        let mut replay = CheckMemory::replay(recorded_sequence(), MemoryLog::default());
        assert!(
            replay
                .read_integer(28, context(), OperandSize::Dword)
                .is_err()
        );
        let (expected, produced) = replay.into_parts();
        let report = first_log_difference(&expected, &produced).expect("a difference");
        assert!(report.starts_with("memory call 0:"), "{report}");
        assert!(
            report.contains("address=0x18") && report.contains("address=0x1c"),
            "{report}"
        );

        // Another width.
        let mut replay = CheckMemory::replay(recorded_sequence(), MemoryLog::default());
        assert!(
            replay
                .read_integer(24, context(), OperandSize::Qword)
                .is_err()
        );
        let (expected, produced) = replay.into_parts();
        assert!(first_log_difference(&expected, &produced).is_some());

        // Right calls, other bytes written.
        let mut replay = CheckMemory::replay(recorded_sequence(), MemoryLog::default());
        assert!(
            replay
                .read_integer(24, context(), OperandSize::Dword)
                .is_ok()
        );
        assert!(replay.write(16, context(), &[1, 2, 4]).is_ok());
        let (expected, produced) = replay.into_parts();
        let report = first_log_difference(&expected, &produced).expect("a difference");
        assert!(report.starts_with("memory call 1:"), "{report}");

        // A missing call.
        let mut replay = CheckMemory::replay(recorded_sequence(), MemoryLog::default());
        assert!(
            replay
                .read_integer(24, context(), OperandSize::Dword)
                .is_ok()
        );
        let (expected, produced) = replay.into_parts();
        let report = first_log_difference(&expected, &produced).expect("a difference");
        assert!(report.contains("fast path made no call"), "{report}");

        // An extra call.
        let mut replay = CheckMemory::replay(recorded_sequence(), MemoryLog::default());
        assert!(
            replay
                .read_integer(24, context(), OperandSize::Dword)
                .is_ok()
        );
        assert!(replay.write(16, context(), &[1, 2, 3]).is_ok());
        assert!(replay.mfence(context()).is_err());
        let (expected, produced) = replay.into_parts();
        let report = first_log_difference(&expected, &produced).expect("a difference");
        assert!(report.contains("general path made no call"), "{report}");

        // The same sequence: no difference.
        let mut replay = CheckMemory::replay(recorded_sequence(), MemoryLog::default());
        assert!(
            replay
                .read_integer(24, context(), OperandSize::Dword)
                .is_ok()
        );
        assert!(replay.write(16, context(), &[1, 2, 3]).is_ok());
        let (expected, produced) = replay.into_parts();
        assert_eq!(first_log_difference(&expected, &produced), None);
    }

    #[test]
    fn replayed_fetches_serve_the_decoder_and_are_not_compared() {
        let mut device = Device::new();
        let mut recording = CheckMemory::record(&mut device, MemoryLog::default());
        assert_eq!(recording.fetch(0x10, context()), Ok(0x90));
        let log = recording.into_log();
        assert_eq!(log.fetches().len(), 1);
        assert!(log.calls().is_empty());
        let mut replay = CheckMemory::replay(log, MemoryLog::default());
        assert_eq!(replay.fetch(0x10, context()), Ok(0x90));
        assert!(replay.fetch(0x11, context()).is_err());
        let (expected, produced) = replay.into_parts();
        assert_eq!(first_log_difference(&expected, &produced), None);
    }

    #[test]
    fn state_differences_name_the_field_and_both_values() {
        let general = LongModeState {
            rip: 0x1003,
            rflags: 0x246,
            ..LongModeState::default()
        };
        let mut fast = general.clone();
        fast.rflags = 0x2c6;
        fast.gpr[3] = 7;
        let lines = state_differences(&general, &fast);
        assert!(
            lines.iter().any(|line| line.starts_with("state.rflags:")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.starts_with("state.gpr:")),
            "{lines:?}"
        );
        assert!(lines.iter().all(|line| !line.contains("rip")), "{lines:?}");
    }
}
