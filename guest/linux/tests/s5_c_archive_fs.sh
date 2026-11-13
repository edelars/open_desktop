#!/bin/sh
set -eu

scenario=C
nonce=${1-}
case "$nonce" in
  ''|*[!A-Za-z0-9._-]*) printf 'OD_S5_C_FAIL nonce=%s step=nonce rc=64\n' invalid; exit 64 ;;
esac
step=preflight
on_exit() {
  rc=$?
  if [ "$rc" -ne 0 ]; then
    printf 'OD_S5_C_FAIL nonce=%s step=%s rc=%s\n' "$nonce" "$step" "$rc"
  fi
}
trap on_exit 0

record_file() {
  file=$1
  line=$(sha256sum "$file")
  digest=${line%% *}
  printf 'OD_S5_C_FILE nonce=%s path=%s sha256=%s\n' "$nonce" "$file" "$digest"
}
manifest() {
  root=$1
  (
    cd "$root"
    sha256sum nested/odd.txt renamed.txt small.txt big.bin
  ) | while IFS=' ' read -r digest path; do printf '%s\n' "$digest"; done
}

step=workspace
base=/var/tmp/open-desktop-stage5/C-$nonce
mkdir -p /var/tmp/open-desktop-stage5
mkdir "$base"
src=$base/source
dst=$base/extracted
mkdir "$src" "$dst" "$src/nested"
step=files
printf 'stage5-C-small nonce=%s\n' "$nonce" > "$src/small.txt"
printf 'odd-name nonce=%s\n' "$nonce" > "$src/nested/odd.txt"
dd if=/dev/zero of="$src/big.bin" bs=4096 count=16 2>/dev/null
printf 'old contents\n' > "$src/replace.tmp"
printf 'replacement nonce=%s\n' "$nonce" > "$src/replace.tmp"
mv "$src/replace.tmp" "$src/renamed.txt"
printf 'removed\n' > "$src/delete.me"
rm "$src/delete.me"
chmod 640 "$src/renamed.txt"
[ "$(stat -c '%a' "$src/renamed.txt")" = 640 ]
step=archive
archive=$base/tree.tar
tar -C "$src" -cf "$archive" .
tar -C "$dst" -xf "$archive"
source_hashes=$(manifest "$src")
extracted_hashes=$(manifest "$dst")
[ "$source_hashes" = "$extracted_hashes" ]
[ "$(stat -c '%a' "$dst/renamed.txt")" = 640 ]
[ ! -e "$src/delete.me" ] && [ ! -e "$dst/delete.me" ]
step=verify
record_file "$src/small.txt"
record_file "$src/nested/odd.txt"
record_file "$src/renamed.txt"
record_file "$src/big.bin"
record_file "$dst/small.txt"
record_file "$dst/nested/odd.txt"
record_file "$dst/renamed.txt"
record_file "$dst/big.bin"
record_file "$archive"
printf 'OD_S5_C_TREE_MATCH nonce=%s files=4\n' "$nonce"
step=complete
printf 'OD_S5_C_PASS nonce=%s archive=%s\n' "$nonce" "$archive"
