//! Atomic state transitions for the single Local APIC and INTIN4 route.
use crate::{DeviceError, local_apic::State};

pub(crate) const MASK: u32 = 1 << 16;
pub(crate) const LEVEL: u32 = 1 << 15;
pub(crate) const REMOTE_IRR: u32 = 1 << 14;

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct Route {
    pub selector: u8,
    pub low: u32,
    pub high_line: bool,
    pub remote_irr: bool,
}
impl Default for Route {
    fn default() -> Self {
        Self {
            selector: 0,
            low: MASK,
            high_line: false,
            remote_irr: false,
        }
    }
}
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub(crate) struct PairState {
    pub local: State,
    pub route: Route,
}
impl PairState {
    pub fn reset_local(&mut self) -> Result<(), DeviceError> {
        if self.route.remote_irr {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        self.local = State::default();
        Ok(())
    }

    pub fn reset_io(&mut self) -> Result<(), DeviceError> {
        if self.route.remote_irr {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        self.route = Route::default();
        Ok(())
    }

    fn bit(vector: u8) -> (usize, u32) {
        (usize::from(vector / 32), 1 << (vector % 32))
    }
    fn busy(&self, vector: u8) -> bool {
        let (i, bit) = Self::bit(vector);
        (self.local.irr[i] | self.local.isr[i]) & bit != 0
    }
    pub fn set_enabled(&mut self, enabled: bool) -> Result<(), DeviceError> {
        if self.local.global_enabled != enabled {
            if !enabled {
                if self.route.remote_irr {
                    return Err(DeviceError::UnsupportedConfiguration);
                }
                self.local = State::default();
            }
            self.local.global_enabled = enabled;
        }
        Ok(())
    }
    pub fn edge(&mut self, vector: u8) -> Result<(), DeviceError> {
        if vector < 16
            || !self.local.global_enabled
            || (self.route.low & LEVEL != 0 && self.route.low as u8 == vector)
        {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        let (i, bit) = Self::bit(vector);
        self.local.irr[i] |= bit;
        self.local.tmr[i] &= !bit;
        Ok(())
    }
    pub fn level(&mut self) -> Result<(), DeviceError> {
        let vector = self.route.low as u8;
        if !self.local.global_enabled || self.local.svr & 0x100 == 0 || self.busy(vector) {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        let (i, bit) = Self::bit(vector);
        self.local.irr[i] |= bit;
        self.local.tmr[i] |= bit;
        self.route.remote_irr = true;
        Ok(())
    }
    pub fn set_line(&mut self, high: bool) -> Result<(), DeviceError> {
        if high && self.route.low & MASK == 0 {
            if self.route.low & LEVEL != 0 {
                if !self.route.remote_irr {
                    self.level()?;
                }
            } else if !self.route.high_line {
                self.edge(self.route.low as u8)?;
            }
        }
        self.route.high_line = high;
        Ok(())
    }
    pub fn set_low(&mut self, value: u32) -> Result<(), DeviceError> {
        let old = self.route.low;
        let changed =
            (old ^ value) & LEVEL != 0 || (old & LEVEL != 0 && (old as u8 != value as u8));
        if changed
            && (self.route.high_line
                || self.route.remote_irr
                || self.busy(old as u8)
                || self.busy(value as u8))
        {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        // Validate delivery before publishing any configuration change.
        let needs_delivery =
            value & (LEVEL | MASK) == LEVEL && self.route.high_line && !self.route.remote_irr;
        if needs_delivery
            && (!self.local.global_enabled || self.local.svr & 0x100 == 0 || self.busy(value as u8))
        {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        self.route.low = value;
        if needs_delivery {
            self.level()?;
        }
        Ok(())
    }
    pub fn eoi(&mut self) -> Result<(), DeviceError> {
        let Some(v) = State::highest(&self.local.isr) else {
            return Ok(());
        };
        let (i, bit) = Self::bit(v);
        let matching =
            self.local.tmr[i] & bit != 0 && self.route.remote_irr && self.route.low as u8 == v;
        let retry = matching && self.route.high_line && self.route.low & MASK == 0;
        if retry
            && (!self.local.global_enabled
                || self.local.svr & 0x100 == 0
                || self.local.irr[i] & bit != 0)
        {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        self.local.isr[i] &= !bit;
        if matching {
            self.route.remote_irr = false;
            if retry {
                self.level()?;
            }
        }
        Ok(())
    }
}
