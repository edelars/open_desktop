//! Explicit caller-driven virtual time and its deterministic TSC conversion.

use std::sync::{Arc, Mutex};

const NANOS_PER_SECOND: u128 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VirtualClockError {
    ClockNotConfigured,
    ClockAlreadyOwned,
    ZeroTscFrequency,
    TimeOverflow,
    TscOverflow,
}

/// A monotonic virtual clock. It advances only when its owner requests it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VirtualClock {
    now_ns: u64,
    tsc_hz: u64,
    tsc: u64,
}

impl VirtualClock {
    pub fn new(tsc_hz: u64) -> Result<Self, VirtualClockError> {
        if tsc_hz == 0 {
            return Err(VirtualClockError::ZeroTscFrequency);
        }
        Ok(Self {
            now_ns: 0,
            tsc_hz,
            tsc: 0,
        })
    }

    pub const fn now_ns(&self) -> u64 {
        self.now_ns
    }

    pub const fn tsc_hz(&self) -> u64 {
        self.tsc_hz
    }

    pub const fn tsc(&self) -> u64 {
        self.tsc
    }

    /// Advance by an explicit amount, committing only if both time and TSC fit.
    pub fn advance_time_ns(&mut self, delta_ns: u64) -> Result<(), VirtualClockError> {
        let now_ns = self
            .now_ns
            .checked_add(delta_ns)
            .ok_or(VirtualClockError::TimeOverflow)?;
        let tsc = u128::from(now_ns) * u128::from(self.tsc_hz) / NANOS_PER_SECOND;
        if tsc > u128::from(u64::MAX) {
            return Err(VirtualClockError::TscOverflow);
        }
        self.now_ns = now_ns;
        self.tsc = tsc as u64;
        Ok(())
    }
}

/// Cloneable observation handle for one virtual time domain.
///
/// The vCPU session remains the only public time advancer. Devices can share
/// this handle so guest-visible timers and RDTSC observe the same clock.
#[derive(Debug)]
struct VirtualClockState {
    clock: VirtualClock,
    advancer_claimed: bool,
    runnable: bool,
}

#[derive(Clone, Debug)]
pub struct VirtualClockHandle(Arc<Mutex<VirtualClockState>>);

/// Unique advancement capability claimed by one vCPU session.
#[derive(Debug)]
pub(crate) struct VirtualClockAdvancer(Arc<Mutex<VirtualClockState>>);

impl PartialEq for VirtualClockHandle {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for VirtualClockHandle {}

impl VirtualClockHandle {
    pub fn new(tsc_hz: u64) -> Result<Self, VirtualClockError> {
        Ok(VirtualClock::new(tsc_hz)?.into())
    }

    pub fn snapshot(&self) -> VirtualClock {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clock
    }

    pub(crate) fn is_runnable(&self) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).runnable
    }

    pub(crate) fn advance_standalone(&self, delta_ns: u64) -> Result<(), VirtualClockError> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.advancer_claimed {
            return Err(VirtualClockError::ClockAlreadyOwned);
        }
        state.clock.advance_time_ns(delta_ns)
    }

    pub(crate) fn claim_advancer(&self) -> Result<VirtualClockAdvancer, VirtualClockError> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.advancer_claimed {
            return Err(VirtualClockError::ClockAlreadyOwned);
        }
        state.advancer_claimed = true;
        state.runnable = true;
        Ok(VirtualClockAdvancer(Arc::clone(&self.0)))
    }
}

impl VirtualClockAdvancer {
    pub(crate) fn snapshot(&self) -> VirtualClock {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clock
    }

    pub(crate) fn set_runnable(&self, runnable: bool) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).runnable = runnable;
    }

    pub(crate) fn advance_time_ns(&self, delta_ns: u64) -> Result<(), VirtualClockError> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clock
            .advance_time_ns(delta_ns)
    }
}

impl From<VirtualClock> for VirtualClockHandle {
    fn from(clock: VirtualClock) -> Self {
        Self(Arc::new(Mutex::new(VirtualClockState {
            clock,
            advancer_claimed: false,
            runnable: true,
        })))
    }
}

impl Drop for VirtualClockAdvancer {
    fn drop(&mut self) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.runnable = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_and_fractional_conversion_round_down() {
        let mut clock = VirtualClock::new(3).unwrap();
        assert_eq!(clock.now_ns(), 0);
        assert_eq!(clock.tsc(), 0);
        clock.advance_time_ns(333_333_333).unwrap();
        assert_eq!(clock.tsc(), 0);
        clock.advance_time_ns(1).unwrap();
        assert_eq!(clock.tsc(), 1);
    }

    #[test]
    fn large_conversion_uses_wide_intermediate() {
        let mut clock = VirtualClock::new(u64::MAX).unwrap();
        clock.advance_time_ns(1_000_000_000).unwrap();
        assert_eq!(clock.tsc(), u64::MAX);
    }

    #[test]
    fn both_overflow_paths_are_atomic() {
        let mut time = VirtualClock::new(1).unwrap();
        time.advance_time_ns(u64::MAX).unwrap();
        assert_eq!(
            time.advance_time_ns(1),
            Err(VirtualClockError::TimeOverflow)
        );
        assert_eq!(time.now_ns(), u64::MAX);

        let mut tsc = VirtualClock::new(u64::MAX).unwrap();
        assert_eq!(
            tsc.advance_time_ns(1_000_000_001),
            Err(VirtualClockError::TscOverflow)
        );
        assert_eq!(tsc.now_ns(), 0);
        assert_eq!(tsc.tsc(), 0);
    }

    #[test]
    fn split_advances_equal_one_advance() {
        let mut split = VirtualClock::new(2_500_000_003).unwrap();
        let mut whole = split;
        split.advance_time_ns(123_456_789).unwrap();
        split.advance_time_ns(987_654_321).unwrap();
        whole.advance_time_ns(1_111_111_110).unwrap();
        assert_eq!(split, whole);
    }

    #[test]
    fn rejects_zero_frequency() {
        assert_eq!(
            VirtualClock::new(0),
            Err(VirtualClockError::ZeroTscFrequency)
        );
    }

    #[test]
    fn only_one_session_can_claim_clock_advancement() {
        let clock = VirtualClockHandle::new(1).unwrap();
        let owner = clock.claim_advancer().unwrap();
        assert_eq!(
            clock.claim_advancer().err(),
            Some(VirtualClockError::ClockAlreadyOwned)
        );
        drop(owner);
        assert_eq!(
            clock.claim_advancer().err(),
            Some(VirtualClockError::ClockAlreadyOwned)
        );
    }
}
