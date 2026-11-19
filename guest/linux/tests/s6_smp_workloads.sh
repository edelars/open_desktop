#!/bin/sh

PATH=/usr/sbin:/usr/bin:/sbin:/bin
LC_ALL=C
export PATH LC_ALL

run_workloads() (
    set -eu
    nonce=invalid
    step=nonce
    build_dir=

    finish() {
        status=$?
        if [ "$status" -ne 0 ]; then
            printf 'OD_S6_22_WRAPPER_FAIL nonce=%s step=%s rc=%s\n' "$nonce" "$step" "$status" >&2
        fi
        if [ -n "$build_dir" ]; then
            rm -rf -- "$build_dir"
        fi
    }
    trap finish 0

    [ "$#" -eq 1 ] || exit 64
    case $1 in
        ''|*[!A-Za-z0-9._-]*) exit 64 ;;
    esac
    [ "${#1}" -le 128 ] || exit 64
    nonce=$1

    step=source
    fixture_dir=/run/open-desktop-stage6
    source=$fixture_dir/s6_smp_workloads.c
    digest=$(sha256sum "$source")
    printf 'OD_S6_22_SOURCE nonce=%s sha256=%s\n' "$nonce" "${digest%% *}"

    step=compile
    build_dir=$(mktemp -d "$fixture_dir/build.XXXXXXXX")
    TMPDIR=$build_dir
    export TMPDIR
    /usr/bin/gcc-15 -std=c11 -O2 -Wall -Wextra -Wpedantic -Werror -pthread \
        "$source" -o "$build_dir/workloads"

    step=run
    "$build_dir/workloads" "$nonce"
    printf 'OD_S6_22_COMPLETE nonce=%s\n' "$nonce"
)

run_workloads "$@"
status=$?
# switch_root starts this shell as PID 1. Keep the guest alive for observation.
if [ "$$" -eq 1 ]; then
    while :; do
        sleep 3600
    done
fi
exit "$status"
