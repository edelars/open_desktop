#!/bin/sh
set -eu

nonce=${1-}
case "$nonce" in
  ''|*[!A-Za-z0-9._-]*) printf 'OD_S5_F_FAIL nonce=%s step=nonce rc=64\n' invalid; exit 64 ;;
esac
step=preflight
on_exit() {
  rc=$?
  if [ "$rc" -ne 0 ]; then
    printf 'OD_S5_F_FAIL nonce=%s step=%s rc=%s\n' "$nonce" "$step" "$rc"
  fi
}
trap on_exit 0

fixture_dir=${S5_FIXTURE_DIR:-/opt/open-desktop-stage5}
source=$fixture_dir/stage5_f.c
[ -f "$source" ] || { step=client-source; exit 66; }
for name in S5_F_HOST S5_F_PORT S5_F_DROP_HOST S5_F_DROP_PORT S5_F_UNREACHABLE_HOST S5_F_UNREACHABLE_PORT; do
  eval "value=\${$name-}"
  if [ -z "$value" ]; then
    printf 'OD_S5_F_BLOCKED nonce=%s missing=%s gate=S11-32,S11-31e,S11-33\n' "$nonce" "$name"
    step=endpoint-configuration
    trap - 0
    exit 78
  fi
done
for host in "$S5_F_HOST" "$S5_F_DROP_HOST" "$S5_F_UNREACHABLE_HOST"; do
  case "$host" in
    localhost|127.*|::1|0.0.0.0|::)
      printf 'OD_S5_F_BLOCKED nonce=%s reason=loopback-endpoint-forbidden\n' "$nonce"
      step=loopback-endpoint
      trap - 0
      exit 78
      ;;
  esac
done
for port in "$S5_F_PORT" "$S5_F_DROP_PORT" "$S5_F_UNREACHABLE_PORT"; do
  case "$port" in ''|*[!0-9]*) step=invalid-port; exit 64 ;; esac
  [ "$port" -ge 1 ] && [ "$port" -le 65535 ] || { step=invalid-port; exit 64; }
done
base=/var/tmp/open-desktop-stage5/F-$nonce
mkdir -p /var/tmp/open-desktop-stage5
mkdir "$base"
client=$base/stage5_f
payload=$base/payload.bin
received=$base/received.bin
log=$base/build.log
step=build-client
cp "$source" "$base/stage5_f.c"
if ! gcc -std=c11 -O2 -Wall -Wextra -Werror -fno-ident -Wl,--build-id=none "$base/stage5_f.c" -o "$client" > "$log" 2>&1; then
  cat "$log"
  exit 1
fi
record_file() {
  file=$1
  line=$(sha256sum "$file")
  printf 'OD_S5_F_FILE nonce=%s path=%s sha256=%s\n' "$nonce" "$file" "${line%% *}"
}
record_file "$base/stage5_f.c"
record_file "$client"
record_file "$log"
step=payload
printf 'open-desktop-stage5-network-payload-v1 nonce=%s\n' "$nonce" > "$payload"
record_file "$payload"
step=transfer
"$client" roundtrip "$S5_F_HOST" "$S5_F_PORT" "$nonce" "$payload" "$received"
cmp "$payload" "$received"
record_file "$received"
printf 'OD_S5_F_ROUNDTRIP nonce=%s sha256=%s\n' "$nonce" "$(sha256sum "$received" | cut -d ' ' -f 1)"
step=drop-probe
"$client" expect-drop "$S5_F_DROP_HOST" "$S5_F_DROP_PORT" "$nonce"
step=unreachable-probe
"$client" expect-unreachable "$S5_F_UNREACHABLE_HOST" "$S5_F_UNREACHABLE_PORT" "$nonce"
step=complete
printf 'OD_S5_F_PASS nonce=%s endpoint=remote\n' "$nonce"
