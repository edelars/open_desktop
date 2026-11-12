#!/bin/sh
set -eu

nonce=${1-}
case "$nonce" in
  ''|*[!A-Za-z0-9._-]*) printf 'OD_S5_D_FAIL nonce=%s step=nonce rc=64\n' invalid; exit 64 ;;
esac
step=preflight
on_exit() {
  rc=$?
  if [ "$rc" -ne 0 ]; then
    printf 'OD_S5_D_FAIL nonce=%s step=%s rc=%s\n' "$nonce" "$step" "$rc"
  fi
}
trap on_exit 0
record_file() {
  file=$1
  line=$(sha256sum "$file")
  printf 'OD_S5_D_FILE nonce=%s path=%s sha256=%s\n' "$nonce" "$file" "${line%% *}"
}

step=source
fixture_dir=${S5_FIXTURE_DIR:-/opt/open-desktop-stage5}
source=$fixture_dir/stage5_d.c
math_source=$fixture_dir/stage5_d_math.c
[ -f "$source" ]
[ -f "$math_source" ]
base=/var/tmp/open-desktop-stage5/D-$nonce
mkdir -p /var/tmp/open-desktop-stage5
mkdir "$base"
copy=$base/stage5_d.c
cp "$source" "$copy"
math_copy=$base/stage5_d_math.c
cp "$math_source" "$math_copy"
record_file "$copy"
record_file "$math_copy"
step=compile
program=$base/stage5_d
log=$base/build.log
if ! /usr/bin/gcc-15 -std=c11 -O2 -Wall -Wextra -Werror -fno-ident -Wl,--build-id=none "$copy" "$math_copy" -o "$program" > "$log" 2>&1; then
  cat "$log"
  exit 1
fi
record_file "$program"
record_file "$log"
step=run
output=$base/output.txt
"$program" "$nonce" > "$output"
cat "$output"
grep -F "OD_S5_D_RESULT nonce=$nonce sum=5050" "$output" >/dev/null
record_file "$output"
step=complete
printf 'OD_S5_D_PASS nonce=%s output=%s\n' "$nonce" "$output"
