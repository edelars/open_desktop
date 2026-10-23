//! Ptrace accounting helpers for architectural REP instruction boundaries.

/// A successful REP STOSB trap is between elements when it advances RCX but
/// leaves RIP at the repeated instruction. The final element advances RIP.
#[cfg(any(all(target_os = "linux", target_arch = "x86_64"), test))]
pub fn rep_stos_iteration_is_intermediate(
    rip_before: u64,
    rcx_before: u64,
    rip_after: u64,
    rcx_after: u64,
) -> bool {
    rip_before == rip_after && rcx_before > rcx_after
}

#[cfg(test)]
mod tests {
    use super::rep_stos_iteration_is_intermediate;

    #[test]
    fn rep_iteration_traps_do_not_retire_until_rip_advances() {
        assert!(rep_stos_iteration_is_intermediate(0x100000, 3, 0x100000, 2));
        assert!(!rep_stos_iteration_is_intermediate(
            0x100000, 1, 0x100002, 0
        ));
        // A self-branch with unchanged RCX is still a retired instruction.
        assert!(!rep_stos_iteration_is_intermediate(
            0x100000, 3, 0x100000, 3
        ));
    }
}
