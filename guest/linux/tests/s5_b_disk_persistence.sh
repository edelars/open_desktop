#!/bin/sh
set -eu

nonce=${1-}
phase=${2-}
case "$nonce" in
  ''|*[!A-Za-z0-9._-]*) printf 'OD_S5_B_FAIL nonce=%s step=nonce rc=64\n' invalid; exit 64 ;;
esac
case "$phase" in
  write|verify) ;;
  *) printf 'OD_S5_B_FAIL nonce=%s step=phase rc=64\n' "$nonce"; exit 64 ;;
esac
if [ "$phase" = write ]; then
  write_nonce=${S5_B_WRITE_NONCE:-$nonce}
  [ "$write_nonce" = "$nonce" ] || { printf 'OD_S5_B_FAIL nonce=%s step=write-nonce rc=64\n' "$nonce"; exit 64; }
else
  write_nonce=${S5_B_WRITE_NONCE-}
  case "$write_nonce" in
    ''|*[!A-Za-z0-9._-]*) printf 'OD_S5_B_FAIL nonce=%s step=write-nonce rc=64\n' "$nonce"; exit 64 ;;
  esac
  [ "$write_nonce" != "$nonce" ] || { printf 'OD_S5_B_FAIL nonce=%s step=nonce-not-fresh rc=64\n' "$nonce"; exit 64; }
fi
step=preflight
on_exit() {
  rc=$?
  if [ "$rc" -ne 0 ]; then
    printf 'OD_S5_B_FAIL nonce=%s phase=%s step=%s rc=%s\n' "$nonce" "$phase" "$step" "$rc"
  fi
}
trap on_exit 0
record_file() {
  file=$1
  line=$(sha256sum "$file")
  printf 'OD_S5_B_FILE nonce=%s phase=%s path=%s sha256=%s\n' "$nonce" "$phase" "$file" "${line%% *}"
}

base=/var/lib/open-desktop-stage5/B
payload=$base/payload-$write_nonce.bin
mkdir -p "$base"
case "$phase" in
  write)
    step=write
    if ! (set -C; printf 'OpenDesktop Stage 5 persistence fixture v1\nwrite_nonce=%s\npayload=known-content-v1\n' "$write_nonce" > "$payload"); then
      step=write-collision
      exit 73
    fi
    sync
    record_file "$payload"
    printf 'OD_S5_B_WRITE nonce=%s write_nonce=%s guest_flush=not_sent\n' "$nonce" "$write_nonce"
    ;;
  verify)
    expected=${S5_B_EXPECTED_SHA256-}
    case "$expected" in
      [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]*) ;;
      *) step=expected-hash; exit 64 ;;
    esac
    [ "${#expected}" -eq 64 ]
    step=verify
    [ -f "$payload" ]
    line=$(sha256sum "$payload")
    actual=${line%% *}
    [ "$actual" = "$expected" ]
    grep -F "write_nonce=$write_nonce" "$payload" >/dev/null
    record_file "$payload"
    printf 'OD_S5_B_VERIFY nonce=%s write_nonce=%s expected_sha256=%s guest_flush=not_sent\n' "$nonce" "$write_nonce" "$expected"
    ;;
esac
step=complete
printf 'OD_S5_B_PASS nonce=%s write_nonce=%s phase=%s\n' "$nonce" "$write_nonce" "$phase"
