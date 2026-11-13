#!/bin/sh
set -eu

nonce=${1-}
case "$nonce" in
  ''|*[!A-Za-z0-9._-]*) printf 'OD_S5_A_FAIL nonce=%s step=nonce rc=64\n' invalid; exit 64 ;;
esac
step=preflight
on_exit() {
  rc=$?
  if [ "$rc" -ne 0 ]; then
    printf 'OD_S5_A_FAIL nonce=%s step=%s rc=%s\n' "$nonce" "$step" "$rc"
  fi
}
trap on_exit 0

# Do not power off a physical Linux test host. A future runner must deliver
# this nonce-bound opt-in through the guest kernel command line, not a host
# environment variable. These test-only keys are preliminary, not a protocol.
guest_vm=0
if [ -r /proc/cpuinfo ] && grep -qi hypervisor /proc/cpuinfo; then
  guest_vm=1
elif [ -r /sys/class/dmi/id/product_name ]; then
  product=$(cat /sys/class/dmi/id/product_name)
  case "$product" in
    *QEMU*|*KVM*|*VirtualBox*|*VMware*|*Virtual*|*OpenDesktop*) guest_vm=1 ;;
  esac
fi
cmdline=$(cat /proc/cmdline 2>/dev/null || true)
case " $cmdline " in *" od_s5_allow_poweroff=1 "*) allow_poweroff=1 ;; *) allow_poweroff=0 ;; esac
case " $cmdline " in *" od_s5_guest_run=1 "*) guest_run=1 ;; *) guest_run=0 ;; esac
case " $cmdline " in *" od_s5_nonce=$nonce "*) nonce_matches=1 ;; *) nonce_matches=0 ;; esac
if [ "$allow_poweroff" -ne 1 ] || [ "$guest_run" -ne 1 ] ||
   [ "$nonce_matches" -ne 1 ] || [ "$guest_vm" -ne 1 ]; then
  trap - 0
  printf 'OD_S5_A_NOT_RUN nonce=%s reason=guest-poweroff-not-authorized\n' "$nonce"
  exit 78
fi

base=/run/open-desktop-stage5/A-$nonce
mkdir -p /run/open-desktop-stage5
mkdir "$base"
marker=$base/boot.txt
step=marker
printf 'boot nonce=%s\n' "$nonce" > "$marker"
line=$(sha256sum "$marker")
printf 'OD_S5_A_FILE nonce=%s path=%s sha256=%s\n' "$nonce" "$marker" "${line%% *}"
sync
step=shutdown
printf 'OD_S5_A_SHUTDOWN_REQUEST nonce=%s method=poweroff\n' "$nonce"
poweroff -f
# Returning means the guest did not stop; only the runner's observed
# MachineStopReason::Shutdown can complete the scenario gate.
trap - 0
printf 'OD_S5_A_FAIL nonce=%s step=poweroff-returned rc=1\n' "$nonce"
exit 1
