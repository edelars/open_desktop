//! Command table and dispatcher.
//!
//! Order of checks (Part 1 v184, clause 9.2, flow of Figure 2): locality, header (tag, size),
//! implemented command code (`TPM_RC_COMMAND_CODE`), startup state (`TPM_RC_INITIALIZE`) and
//! failure mode (`TPM_RC_FAILURE`), handle area, handle validity, authorization, parameters,
//! command action. A command that fails leaves the TPM state unchanged except the self-test
//! state; the actions run only after every check.
//! NOT VERIFIED against TPM 2.0 Library v185: the position of the startup check relative to the
//! command-code check (the open text lists the header checks and the handle/authorization checks
//! but not where `TPM_RC_INITIALIZE` falls); phase A answers an unknown code before `Startup`
//! with `TPM_RC_COMMAND_CODE`.
//!
//! The command table is the single source for dispatch and for `TPM_CAP_COMMANDS`: a command that
//! is not in [`COMMANDS`] gets `TPM_RC_COMMAND_CODE`, never `TPM_RC_SUCCESS`.
//! `TPM2_PCR_Allocate` (code 0x12B) is deliberately absent, see `docs/specs/VTPM.md`.

use crate::codes::{cc, handle, st};
use crate::command::{HEADER_SIZE, check_password_session, parse_auth_area, parse_header};
use crate::hash::{AuthValue, password_matches};
use crate::marshal::{Reader, limits};
use crate::pcr::resolve_pcr_handle;
use crate::response::{Output, Rc, error_response, success_response};
use crate::state::Tpm;

/// One implemented command.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CommandSpec {
    pub code: u32,
    /// Handles in the command handle area (`cHandles` of `TPMA_CC`).
    pub handles: u8,
    /// Handles that need authorization (the first `auth` handles).
    pub auth: u8,
    /// A handle in the response handle area (`rHandle`).
    pub rhandle: bool,
    /// The command may write NV (`{NV}` in Part 3).
    pub nv: bool,
    /// The sequence handle is flushed on success (`{F}` in Part 3).
    pub flushed: bool,
}

const fn spec(
    code: u32,
    handles: u8,
    auth: u8,
    rhandle: bool,
    nv: bool,
    flushed: bool,
) -> CommandSpec {
    CommandSpec {
        code,
        handles,
        auth,
        rhandle,
        nv,
        flushed,
    }
}

/// Implemented commands in ascending command-index order. Attributes (`nv`, `flushed`,
/// `cHandles`, `rHandle`) are from the command tables of Part 3 v184 (checked 2026-11-07).
pub(crate) static COMMANDS: [CommandSpec; 17] = [
    spec(cc::PCR_EVENT, 1, 1, false, true, false),
    spec(cc::PCR_RESET, 1, 1, false, true, false),
    spec(cc::SEQUENCE_COMPLETE, 1, 1, false, false, true),
    spec(cc::SELF_TEST, 0, 0, false, true, false),
    spec(cc::STARTUP, 0, 0, false, true, false),
    spec(cc::SHUTDOWN, 0, 0, false, true, false),
    spec(cc::STIR_RANDOM, 0, 0, false, true, false),
    spec(cc::SEQUENCE_UPDATE, 1, 1, false, false, false),
    spec(cc::GET_CAPABILITY, 0, 0, false, false, false),
    spec(cc::GET_RANDOM, 0, 0, false, false, false),
    spec(cc::GET_TEST_RESULT, 0, 0, false, false, false),
    spec(cc::HASH, 0, 0, false, false, false),
    spec(cc::PCR_READ, 0, 0, false, false, false),
    spec(cc::READ_CLOCK, 0, 0, false, false, false),
    spec(cc::PCR_EXTEND, 1, 1, false, true, false),
    spec(cc::EVENT_SEQUENCE_COMPLETE, 2, 2, false, true, true),
    spec(cc::HASH_SEQUENCE_START, 0, 0, true, false, false),
];

/// What a handler needs besides the parameter reader.
pub(crate) struct Invocation {
    pub handles: [u32; 2],
}

fn lookup(code: u32) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|c| c.code == code)
}

pub(crate) fn execute(tpm: &mut Tpm, locality: u8, command: &[u8]) -> Vec<u8> {
    match run(tpm, locality, command) {
        Ok(v) if v.len() <= limits::MAX_RESPONSE_SIZE => v,
        Ok(_) => error_response(Rc::FAILURE),
        Err(rc) => error_response(rc),
    }
}

/// Authorization value of the entity behind handle number `i` of `spec`.
/// PCR and `TPM_RH_NULL` use the empty authorization (PTP 1.07, clause 4.7: Empty Auth for all PCR).
fn handle_auth(tpm: &Tpm, code: u32, handles: &[u32; 2], i: usize) -> Result<AuthValue, Rc> {
    let h = handles[i];
    let is_pcr_position = matches!(
        (code, i),
        (cc::PCR_EXTEND | cc::PCR_EVENT | cc::PCR_RESET, 0) | (cc::EVENT_SEQUENCE_COMPLETE, 0)
    );
    if is_pcr_position || h == handle::RH_NULL {
        return Ok(AuthValue::empty());
    }
    let slot = tpm.seq_slot(h, i)?;
    // `AuthValue` is a fixed buffer wiped on drop: no un-wiped copy in a plain `Vec`.
    Ok(tpm.seq_auth(slot))
}

fn run(tpm: &mut Tpm, locality: u8, command: &[u8]) -> Result<Vec<u8>, Rc> {
    // Only locality 0 is served (ADR-0009, decision 4).
    if locality != 0 {
        return Err(Rc::LOCALITY);
    }
    let header = parse_header(command)?;
    let spec = lookup(header.code).ok_or(Rc::COMMAND_CODE)?;

    // Failure mode answers only GetTestResult and GetCapability, and only without sessions.
    let failure_ok = matches!(header.code, cc::GET_TEST_RESULT | cc::GET_CAPABILITY);
    if tpm.failed && (!failure_ok || header.tag != st::NO_SESSIONS) {
        return Err(Rc::FAILURE);
    }
    if !tpm.started && header.code != cc::STARTUP && !(tpm.failed && failure_ok) {
        return Err(Rc::INITIALIZE);
    }
    if tpm.started && header.code == cc::STARTUP {
        return Err(Rc::INITIALIZE);
    }

    let mut r = Reader::new(&command[HEADER_SIZE..]);
    let mut handles = [0u32; 2];
    for slot in handles.iter_mut().take(usize::from(spec.handles)) {
        *slot = r.u32()?;
    }
    check_handles(tpm, spec, &handles)?;

    // Authorization area.
    let with_sessions = header.tag == st::SESSIONS;
    let sessions = if with_sessions {
        parse_auth_area(&mut r)?
    } else {
        Vec::new()
    };
    // NOT VERIFIED against TPM 2.0 Library v185/v184: TPM_RC_AUTHSIZE при authorizationSize < 9
    // для SESSIONS. With `TPM_ST_SESSIONS` and `authorizationSize = 0` a command that needs no
    // authorization (e.g. GetRandom) is served; the behaviour is kept, the spec text was not checked.
    let needed = usize::from(spec.auth);
    if sessions.len() < needed {
        return Err(Rc::AUTH_MISSING);
    }
    if sessions.len() > needed {
        return Err(Rc::AUTHSIZE);
    }
    for (i, s) in sessions.iter().enumerate() {
        check_password_session(s, i + 1)?;
    }
    for (i, s) in sessions.iter().enumerate() {
        let auth = handle_auth(tpm, spec.code, &handles, i)?;
        if !password_matches(&s.hmac, auth.as_slice()) {
            // No dictionary attack protection in phase A: every failure is "without DA
            // implications" (Part 3, clause 5; Part 1, clause 17.6.8).
            return Err(Rc::BAD_AUTH.in_session((i + 1) as u8));
        }
    }

    let inv = Invocation { handles };
    let mut p = Reader::new(&command[command.len() - r.remaining()..]);
    let out: Output = match spec.code {
        cc::STARTUP => tpm.cmd_startup(&mut p)?,
        cc::SHUTDOWN => tpm.cmd_shutdown(&mut p)?,
        cc::SELF_TEST => tpm.cmd_self_test(&mut p)?,
        cc::GET_TEST_RESULT => tpm.cmd_get_test_result(&mut p)?,
        cc::GET_CAPABILITY => tpm.cmd_get_capability(&mut p)?,
        cc::GET_RANDOM => tpm.cmd_get_random(&mut p)?,
        cc::STIR_RANDOM => tpm.cmd_stir_random(&mut p)?,
        cc::PCR_READ => tpm.cmd_pcr_read(&mut p)?,
        cc::PCR_EXTEND => tpm.cmd_pcr_extend(&inv, &mut p)?,
        cc::PCR_EVENT => tpm.cmd_pcr_event(&inv, &mut p)?,
        cc::PCR_RESET => tpm.cmd_pcr_reset(&inv, &mut p)?,
        cc::HASH => tpm.cmd_hash(&mut p)?,
        cc::HASH_SEQUENCE_START => tpm.cmd_hash_sequence_start(&mut p)?,
        cc::SEQUENCE_UPDATE => tpm.cmd_sequence_update(&inv, &mut p)?,
        cc::SEQUENCE_COMPLETE => tpm.cmd_sequence_complete(&inv, &mut p)?,
        cc::EVENT_SEQUENCE_COMPLETE => tpm.cmd_event_sequence_complete(&inv, &mut p)?,
        cc::READ_CLOCK => tpm.cmd_read_clock(&mut p)?,
        // Unreachable while the table and this match agree; a missing arm is the safe answer.
        _ => return Err(Rc::COMMAND_CODE),
    };
    let attrs: Vec<u8> = sessions.iter().map(|s| s.attributes).collect();
    Ok(success_response(&out, &attrs, with_sessions))
}

/// Handle validity (type, loaded) before authorization (Part 1, clause 9.2).
fn check_handles(tpm: &Tpm, spec: &CommandSpec, handles: &[u32; 2]) -> Result<(), Rc> {
    match spec.code {
        cc::PCR_EXTEND | cc::PCR_EVENT => {
            resolve_pcr_handle(handles[0], true, 1)?;
        }
        cc::PCR_RESET => {
            resolve_pcr_handle(handles[0], false, 1)?;
        }
        cc::SEQUENCE_UPDATE | cc::SEQUENCE_COMPLETE => {
            tpm.seq_slot(handles[0], 0)?;
        }
        cc::EVENT_SEQUENCE_COMPLETE => {
            resolve_pcr_handle(handles[0], true, 1)?;
            tpm.seq_slot(handles[1], 1)?;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_unique_and_in_profile() {
        let mut prev = 0;
        for c in &COMMANDS {
            assert!(c.code > prev, "{:#x} out of order", c.code);
            prev = c.code;
            assert!(c.auth <= c.handles);
        }
        // PCR_Allocate (0x12B) and PCR_SetAuthPolicy/Value are not implemented.
        for code in [0x12Bu32, 0x12C, 0x183] {
            assert!(lookup(code).is_none());
        }
        assert_eq!(COMMANDS.len(), 17);
    }
}
