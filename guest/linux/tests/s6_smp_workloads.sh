#!/bin/sh

PATH=/usr/sbin:/usr/bin:/sbin:/bin
LC_ALL=C
export PATH LC_ALL

run_workloads() (
    set -eu
    task=22
    nonce=invalid
    step=nonce
    build_dir=
    prebuilt=
    fixture_dir=/run/open-desktop-stage6

    finish() {
        status=$?
        if [ "$status" -ne 0 ]; then
            printf 'OD_S6_%s_WRAPPER_FAIL nonce=%s step=%s rc=%s\n' "$task" "$nonce" "$step" "$status" >&2
        fi
        if [ -n "$build_dir" ]; then
            rm -rf -- "$build_dir"
        fi
    }
    trap finish 0

    if [ "$#" -ge 2 ] && [ "$1" = --prebuilt ]; then
        step=prebuilt-arguments
        [ "$#" -ge 3 ] || exit 64
        prebuilt=$2
        [ "$prebuilt" = "$fixture_dir/s6_smp_workloads.elf" ] || exit 64
        shift 2
    fi
    if [ "$#" -eq 3 ] && [ "$2" = --compute ]; then
        task=38
    else
        [ "$#" -eq 1 ] || exit 64
    fi
    case $1 in
        ''|*[!A-Za-z0-9._-]*) exit 64 ;;
    esac
    [ "${#1}" -le 128 ] || exit 64
    nonce=$1
    if [ "$task" -eq 38 ]; then
        step=compute-arguments
        workers=${3%%,*}
        remainder=${3#*,}
        blocks=${remainder%%,*}
        checksum=${remainder#*,}
        [ "$3" = "$workers,$blocks,$checksum" ] || exit 64
        set -- "$nonce" --compute "$workers" "$blocks" "$checksum"
    fi

    step=source
    source=$fixture_dir/s6_smp_workloads.c
    digest=$(sha256sum "$source")
    printf 'OD_S6_%s_SOURCE nonce=%s sha256=%s\n' "$task" "$nonce" "${digest%% *}"

    if [ -n "$prebuilt" ]; then
        step=prebuilt
        [ -x "$prebuilt" ] || exit 1
        executable=$prebuilt
    else
        step=compile
        build_dir=$(mktemp -d "$fixture_dir/build.XXXXXXXX")
        TMPDIR=$build_dir
        export TMPDIR
        /usr/bin/gcc-15 -std=c11 -O2 -Wall -Wextra -Wpedantic -Werror -pthread \
            "$source" -o "$build_dir/workloads"
        executable=$build_dir/workloads
    fi

    step=run
    "$executable" "$@"
    step=online
    IFS= read -r online </sys/devices/system/cpu/online
    printf 'OD_S6_%s_ONLINE nonce=%s cpus=%s\n' "$task" "$nonce" "$online"
    if [ "$task" -eq 22 ]; then
        printf 'OD_S6_22_COMPLETE nonce=%s\n' "$nonce"
    fi
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
