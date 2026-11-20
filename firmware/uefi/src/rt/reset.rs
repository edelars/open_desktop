//! ResetSystem commands for the board's ACPI fixed hardware.

use crate::acpi::manifest::AcpiPmEntry;
use crate::uefi::runtime::{RESET_SHUTDOWN, ResetType};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResetCommand {
    Reset { port: u16, value: u8 },
    Shutdown { port: u16, sleep_bits: u16 },
}

pub fn command(hardware: AcpiPmEntry, reset_type: ResetType) -> ResetCommand {
    if reset_type == RESET_SHUTDOWN || hardware.reset_reg == 0 {
        ResetCommand::Shutdown {
            port: hardware.pm1a_cnt_blk,
            sleep_bits: (u16::from(hardware.slp_typ_s5) << 10) | (1 << 13),
        }
    } else {
        // The board implements cold reset and defines no platform reset GUIDs.
        ResetCommand::Reset {
            port: hardware.reset_reg,
            value: hardware.reset_value,
        }
    }
}

#[cfg(any(target_os = "none", target_os = "uefi"))]
pub fn perform(hardware: Option<AcpiPmEntry>, reset_type: ResetType) -> ! {
    use crate::{
        console::serial_term,
        stop::{STOP_RESET_UNAVAILABLE, format_stop_line},
    };
    use core::arch::asm;

    let (reason, detail): (&[u8], u64) = if let Some(hardware) = hardware {
        match command(hardware, reset_type) {
            ResetCommand::Reset { port, value } => unsafe {
                asm!("out dx, al", in("dx") port, in("al") value, options(nostack, preserves_flags))
            },
            ResetCommand::Shutdown { port, sleep_bits } => unsafe {
                let previous: u16;
                asm!("in ax, dx", in("dx") port, out("ax") previous, options(nostack, preserves_flags));
                let value = (previous & !(0b111 << 10)) | sleep_bits;
                asm!("out dx, ax", in("dx") port, in("ax") value, options(nostack, preserves_flags));
            },
        }
        (b"OD-FW ResetSystem hardware returned\r\n", 2)
    } else {
        (b"OD-FW ResetSystem has no fixed hardware\r\n", 1)
    };
    serial_term::init();
    serial_term::write(reason);
    serial_term::write(&format_stop_line(STOP_RESET_UNAVAILABLE, detail));
    unsafe { asm!("cli", "2: hlt", "jmp 2b", options(noreturn, nostack)) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uefi::runtime::{RESET_COLD, RESET_PLATFORM_SPECIFIC, RESET_WARM};

    #[test]
    fn uses_fixed_reset_register_and_s5_sleep_type() {
        let acpi = AcpiPmEntry {
            base_port: 0x600,
            port_count: 16,
            pm1a_evt_blk: 0x600,
            pm1a_cnt_blk: 0x604,
            pm_tmr_blk: 0x608,
            reset_reg: 0x60c,
            reset_value: 6,
            slp_typ_s5: 5,
            sci_gsi: 9,
            sci_level: true,
            sci_active_low: true,
        };
        for reset_type in [RESET_COLD, RESET_WARM, RESET_PLATFORM_SPECIFIC] {
            assert_eq!(
                command(acpi, reset_type),
                ResetCommand::Reset {
                    port: 0x60c,
                    value: 6
                }
            );
        }
        assert_eq!(
            command(acpi, RESET_SHUTDOWN),
            ResetCommand::Shutdown {
                port: 0x604,
                sleep_bits: 0x3400
            }
        );
        assert_eq!(
            command(
                AcpiPmEntry {
                    reset_reg: 0,
                    reset_value: 0,
                    ..acpi
                },
                RESET_COLD,
            ),
            command(acpi, RESET_SHUTDOWN)
        );
    }
}
