#!/bin/sh
set -eu

nonce=${1-}
case "$nonce" in
  ''|*[!A-Za-z0-9._-]*) printf 'OD_S5_G_FAIL nonce=%s step=nonce rc=64\n' invalid; exit 64 ;;
esac
step=preflight
on_exit() {
  rc=$?
  if [ "$rc" -ne 0 ]; then
    printf 'OD_S5_G_FAIL nonce=%s step=%s rc=%s\n' "$nonce" "$step" "$rc"
  fi
}
trap on_exit 0
fixture_dir=${S5_FIXTURE_DIR:-/opt/open-desktop-stage5}
source=$fixture_dir/stage5_g.c
[ -f "$source" ]
memory_mib=${S5_G_MEMORY_MIB:-64}
case "$memory_mib" in ''|*[!0-9]*) step=memory-size; exit 64 ;; esac
[ "$memory_mib" -ge 16 ] && [ "$memory_mib" -le 128 ] || { step=memory-size; exit 64; }
base=/var/tmp/open-desktop-stage5/G-$nonce
mkdir -p /var/tmp/open-desktop-stage5
mkdir "$base"
step=prepare
cp "$source" "$base/stage5_g.c"
program=$base/stage5_g
log=$base/build.log
if ! /usr/bin/gcc-15 -std=c11 -O2 -Wall -Wextra -Werror -fno-ident -Wl,--build-id=none "$base/stage5_g.c" -o "$program" > "$log" 2>&1; then
  cat "$log"
  exit 1
fi
record_file() {
  file=$1
  line=$(sha256sum "$file")
  printf 'OD_S5_G_FILE nonce=%s path=%s sha256=%s\n' "$nonce" "$file" "${line%% *}"
}
record_file "$base/stage5_g.c"
record_file "$program"
record_file "$log"
persistent=/var/lib/open-desktop-stage5/G
mkdir -p "$persistent"
sentinel=$persistent/committed-$nonce.txt
step=sentinel
printf 'S5 G completed-before-stop nonce=%s\n' "$nonce" > "$sentinel"
sync
record_file "$sentinel"
step=memory-check
printf 'OD_S5_G_MEMORY_START nonce=%s mib=%s stop_gate=OD_S5_G_READY\n' "$nonce" "$memory_mib"
"$program" "$nonce" "$memory_mib"
# Returning from the process before the host stops the VM is not a pass.
step=ready-process-returned
exit 1
