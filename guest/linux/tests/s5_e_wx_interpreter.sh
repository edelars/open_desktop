#!/bin/sh
set -eu

nonce=${1-}
case "$nonce" in
  ''|*[!A-Za-z0-9._-]*) printf 'OD_S5_E_FAIL nonce=%s step=nonce rc=64\n' invalid; exit 64 ;;
esac
step=preflight
on_exit() {
  rc=$?
  if [ "$rc" -ne 0 ]; then
    printf 'OD_S5_E_FAIL nonce=%s step=%s rc=%s\n' "$nonce" "$step" "$rc"
  fi
}
trap on_exit 0
record_file() {
  file=$1
  line=$(sha256sum "$file")
  printf 'OD_S5_E_FILE nonce=%s path=%s sha256=%s\n' "$nonce" "$file" "${line%% *}"
}

step=architecture
[ "$(uname -m)" = x86_64 ]
fixture_dir=${S5_FIXTURE_DIR:-/opt/open-desktop-stage5}
source=$fixture_dir/stage5_e.c
[ -f "$source" ]
base=/var/tmp/open-desktop-stage5/E-$nonce
mkdir -p /var/tmp/open-desktop-stage5
mkdir "$base"
copy=$base/stage5_e.c
cp "$source" "$copy"
record_file "$copy"
step=compile
program=$base/stage5_e
log=$base/build.log
if ! gcc -std=c11 -O2 -Wall -Wextra -Werror -fno-ident -Wl,--build-id=none "$copy" -o "$program" > "$log" 2>&1; then
  cat "$log"
  exit 1
fi
record_file "$program"
record_file "$log"
step=execute
output=$base/output.txt
"$program" "$nonce" > "$output"
cat "$output"
grep -F "OD_S5_E_RESULT nonce=$nonce first=41 second=42 nx=denied" "$output" >/dev/null
record_file "$output"
step=complete
printf 'OD_S5_E_PASS nonce=%s profile=interpreter jit_claim=none\n' "$nonce"
