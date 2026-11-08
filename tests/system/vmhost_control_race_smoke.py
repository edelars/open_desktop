#!/usr/bin/env python3
"""Check VMHost stop races, partial-frame handling, and launcher completion."""

import argparse
import itertools
import json
from pathlib import Path
import socket
import struct
import subprocess
import sys
import time


MAGIC = b"ODCP"
VERSION = 0
HELLO, READY, RUN, RUN_ACK, STOP, STOP_ACK, RESULT, ERROR = 1, 2, 3, 4, 5, 6, 7, 255
SCENARIO_PATH = Path(__file__).resolve().parents[1] / "fixtures/control/control_v0_scenarios.json"
SCENARIOS = json.loads(SCENARIO_PATH.read_text())
DIAGNOSTICS_DIR = None
_CHILD_LOG_SEQUENCE = itertools.count(1)
if SCENARIOS.get("schema_version") != 1 or SCENARIOS.get("protocol") != "CONTROL_PROTOCOL_V0":
    raise RuntimeError(f"unsupported control scenario fixture: {SCENARIO_PATH}")
MAX_PAYLOAD = SCENARIOS["max_payload_bytes"]


def frame(kind, request_id, payload=b""):
    if len(payload) > MAX_PAYLOAD:
        raise ValueError("payload exceeds protocol limit")
    return MAGIC + struct.pack("<HHII", VERSION, kind, request_id, len(payload)) + payload


def send_frame(channel, kind, request_id, payload=b""):
    channel.sendall(frame(kind, request_id, payload))


def read_exact(channel, length):
    data = bytearray()
    while len(data) < length:
        chunk = channel.recv(length - len(data))
        if not chunk:
            raise EOFError("VMHost closed the control channel during a frame")
        data.extend(chunk)
    return bytes(data)


def read_frame(channel):
    header = read_exact(channel, 16)
    magic, version, kind, request_id, payload_length = struct.unpack("<4sHHII", header)
    if magic != MAGIC or version != VERSION or payload_length > MAX_PAYLOAD:
        raise RuntimeError("invalid VMHost response frame")
    return kind, request_id, read_exact(channel, payload_length)


def run_payload(code, budget, rax=0, rcx=0):
    return struct.pack("<IQ4QQQ", len(code), budget, rax, rcx, 0, 0, 0, 2) + code


def deterministic_chunks(data, seed, max_chunk_bytes):
    state = seed & 0xFFFFFFFF
    offset = 0
    while offset < len(data):
        state = (1664525 * state + 1013904223) & 0xFFFFFFFF
        size = 1 + state % min(max_chunk_bytes, len(data) - offset)
        yield data[offset:offset + size]
        offset += size


def launch_vmhost(path):
    stdout = stderr = subprocess.DEVNULL
    opened_logs = []
    if DIAGNOSTICS_DIR is not None:
        sequence = next(_CHILD_LOG_SEQUENCE)
        try:
            stdout = (DIAGNOSTICS_DIR / f"vmhost-{sequence:03d}.stdout.log").open("xb")
            opened_logs.append(stdout)
            stderr = (DIAGNOSTICS_DIR / f"vmhost-{sequence:03d}.stderr.log").open("xb")
            opened_logs.append(stderr)
        except Exception:
            for output in opened_logs:
                output.close()
            raise
    parent, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        process = subprocess.Popen(
            [path, "--socket-fd", str(child.fileno())],
            pass_fds=(child.fileno(),),
            stdout=stdout,
            stderr=stderr,
        )
    except Exception:
        parent.close()
        child.close()
        raise
    finally:
        for output in opened_logs:
            output.close()
    child.close()
    parent.settimeout(3)
    return process, parent


def start_vmhost(path, hello_seed=None, max_chunk_bytes=13, inter_chunk_delay_ms=1):
    process, parent = launch_vmhost(path)
    hello = frame(HELLO, 0)
    chunks = (
        (hello,)
        if hello_seed is None
        else deterministic_chunks(hello, hello_seed, max_chunk_bytes)
    )
    for chunk in chunks:
        parent.sendall(chunk)
        if hello_seed is not None:
            time.sleep(inter_chunk_delay_ms / 1000)
    if read_frame(parent) != (READY, 0, struct.pack("<I", 1)):
        raise RuntimeError("VMHost ABI handshake failed")
    return process, parent


def begin_run(
    channel,
    request_id,
    code,
    budget,
    rax=0,
    rcx=0,
    chunk_seed=None,
    max_chunk_bytes=13,
    inter_chunk_delay_ms=1,
):
    run = frame(RUN, request_id, run_payload(code, budget, rax, rcx))
    chunks = (
        (run,)
        if chunk_seed is None
        else deterministic_chunks(run, chunk_seed, max_chunk_bytes)
    )
    for chunk in chunks:
        channel.sendall(chunk)
        if chunk_seed is not None:
            time.sleep(inter_chunk_delay_ms / 1000)
    if read_frame(channel) != (RUN_ACK, request_id, struct.pack("<I", 0)):
        raise RuntimeError("VMHost did not acknowledge RUN")


def raw_header(magic=MAGIC, version=VERSION, kind=HELLO, request_id=0, payload_length=0):
    return magic + struct.pack("<HHII", version, kind, request_id, payload_length)


def expect_failed_process(process, channel, scenario):
    try:
        try:
            data = channel.recv(1)
        except ConnectionResetError:
            data = b""
        if data:
            raise RuntimeError(f"{scenario}: VMHost sent a response after rejecting the frame")
        if process.wait(timeout=2) == 0:
            raise RuntimeError(f"{scenario}: VMHost accepted a malformed frame")
    finally:
        channel.close()
        if process.poll() is None:
            process.kill()
            process.wait()


def check_malformed_hello(vmhost, label, header):
    process, channel = launch_vmhost(vmhost)
    try:
        channel.sendall(header)
    except OSError:
        channel.close()
        process.kill()
        process.wait()
        raise
    expect_failed_process(process, channel, label)


def check_malformed_stop(vmhost, label, header):
    process, channel = start_vmhost(vmhost)
    try:
        begin_run(channel, 19, bytes.fromhex("75fe"), (1 << 64) - 1)
        channel.sendall(header)
        expect_failed_process(process, channel, label)
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()


def check_chunked_frames(vmhost, seed, parameters):
    process, channel = start_vmhost(
        vmhost,
        hello_seed=seed,
        max_chunk_bytes=parameters["max_chunk_bytes"],
        inter_chunk_delay_ms=parameters["inter_chunk_delay_ms"],
    )
    try:
        begin_run(
            channel,
            20,
            bytes.fromhex("75fe"),
            (1 << 64) - 1,
            chunk_seed=seed,
            max_chunk_bytes=parameters["max_chunk_bytes"],
            inter_chunk_delay_ms=parameters["inter_chunk_delay_ms"],
        )
        send_frame(channel, STOP, 20)
        kind, request_id, payload = read_frame(channel)
        if (kind, request_id, payload) != (STOP_ACK, 20, struct.pack("<I", 0)):
            raise RuntimeError(f"seed {seed}: expected STOP_ACK before RESULT")
        kind, request_id, payload = read_frame(channel)
        if kind != RESULT or request_id != 20 or struct.unpack_from("<I", payload, 48)[0] != 2:
            raise RuntimeError(f"seed {seed}: chunked RUN did not stop with a valid RESULT")
        if process.wait(timeout=2) != 0:
            raise RuntimeError(f"seed {seed}: VMHost exited unsuccessfully")
    finally:
        channel.close()
        if process.poll() is None:
            process.kill()
            process.wait()


def check_invalid_stop_requests(vmhost, scenarios):
    process, channel = start_vmhost(vmhost)
    try:
        begin_run(channel, 21, bytes.fromhex("75fe"), (1 << 64) - 1)
        for scenario in scenarios:
            request_id = 21 + scenario["request_id_delta"]
            payload = bytes.fromhex(scenario["payload_hex"])
            send_frame(channel, STOP, request_id, payload)
            if read_frame(channel) != (ERROR, request_id, struct.pack("<I", 1)):
                raise RuntimeError(f"VMHost did not reject STOP scenario: {scenario['label']}")
        send_frame(channel, STOP, 21)
        if read_frame(channel) != (STOP_ACK, 21, struct.pack("<I", 0)):
            raise RuntimeError("VMHost failed to service STOP after rejected requests")
        kind, request_id, payload = read_frame(channel)
        if kind != RESULT or request_id != 21 or struct.unpack_from("<I", payload, 48)[0] != 2:
            raise RuntimeError("VMHost did not return RESULT after accepted STOP")
        if process.wait(timeout=2) != 0:
            raise RuntimeError("VMHost exited unsuccessfully after rejected STOP requests")
    finally:
        channel.close()
        if process.poll() is None:
            process.kill()
            process.wait()


def check_partial_frame(vmhost, partial):
    process, channel = start_vmhost(vmhost)
    try:
        # The finite loop takes long enough to leave a partial control frame
        # pending while its worker reaches its instruction budget.
        begin_run(channel, 7, bytes.fromhex("482bc175fb"), 2_000_000, rax=1 << 63, rcx=1)
        channel.sendall(partial)
        channel.settimeout(2)
        kind, request_id, payload = read_frame(channel)
        if kind != RESULT or request_id != 7 or len(payload) != 64:
            raise RuntimeError("VMHost failed to return RESULT with a partial request buffered")
        if struct.unpack_from("<I", payload, 48)[0] != 1:
            raise RuntimeError("partial-frame scenario did not reach its instruction budget")
        if process.wait(timeout=2) != 0:
            raise RuntimeError("VMHost exited unsuccessfully after RESULT")
    finally:
        channel.close()
        if process.poll() is None:
            process.kill()
            process.wait()


def check_disconnect(vmhost):
    process, channel = start_vmhost(vmhost)
    try:
        begin_run(channel, 8, bytes.fromhex("75fe"), (1 << 64) - 1)
        channel.close()
        try:
            status = process.wait(timeout=2)
        except subprocess.TimeoutExpired as error:
            raise RuntimeError("VMHost did not stop after the launcher disconnected") from error
        if status == 0:
            raise RuntimeError("VMHost unexpectedly reported success without a result receiver")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()


def check_launcher(launcher, vmhost, code, args, expected_reason):
    result = subprocess.run(
        [launcher, "--vmhost", vmhost, "--hex", code, *args],
        capture_output=True,
        text=True,
        timeout=5,
    )
    if result.returncode != 0:
        raise RuntimeError(f"launcher failed ({result.returncode}): {result.stderr.strip()}")
    try:
        report = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError(f"launcher returned invalid JSON: {result.stdout!r}") from error
    expected = expected_reason if isinstance(expected_reason, tuple) else (expected_reason,)
    if report.get("reason") not in expected:
        raise RuntimeError(f"launcher returned reason {report.get('reason')}, expected one of {expected}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "launcher", nargs="?", default=str(Path(__file__).resolve().parents[2] / "apps" / ".build" / "debug" / "open-desktop-launcher-smoke"),
        help="path to the built launcher smoke binary (default: apps/.build/debug/open-desktop-launcher-smoke)",
    )
    parser.add_argument(
        "vmhost", nargs="?", default=str(Path(__file__).resolve().parents[2] / "apps" / ".build" / "debug" / "od-vmhost"),
        help="path to the built VMHost binary (default: apps/.build/debug/od-vmhost)",
    )
    parser.add_argument("--diagnostics-dir", type=Path)
    args = parser.parse_args()
    global DIAGNOSTICS_DIR
    if args.diagnostics_dir is not None:
        if args.diagnostics_dir.is_symlink() or not args.diagnostics_dir.is_dir():
            raise RuntimeError("diagnostics directory must be an existing non-symlink directory")
        DIAGNOSTICS_DIR = args.diagnostics_dir.resolve()

    # Reproduce the original 100-run timing window. An early STOP must be kept
    # until the worker has entered the C ABI execution state.
    for _ in range(100):
        check_launcher(
            args.launcher,
            args.vmhost,
            "75fe",
            ["--budget", str((1 << 64) - 1), "--stop-after-ms", "0"],
            expected_reason=2,
        )

    # The guest returns before the requested 25 ms stop delay. The result wins.
    check_launcher(args.launcher, args.vmhost, "48b80100000000000000", ["--stop-after-ms", "25"], 0)

    # Either natural completion or accepted STOP may win this zero-delay race.
    for _ in range(20):
        check_launcher(args.launcher, args.vmhost, "48b80100000000000000", ["--stop-after-ms", "0"], (0, 2))

    for scenario in SCENARIOS["partial_frames"]:
        check_partial_frame(args.vmhost, bytes.fromhex(scenario["hex"]))
    check_disconnect(args.vmhost)

    chunking = SCENARIOS["frame_chunking"]
    for seed in chunking["seeds"]:
        check_chunked_frames(args.vmhost, seed, chunking)
    check_invalid_stop_requests(args.vmhost, SCENARIOS["invalid_stop_requests"])

    for scenario in SCENARIOS["malformed_hello"]:
        check_malformed_hello(
            args.vmhost,
            scenario["label"],
            raw_header(
                magic=bytes.fromhex(scenario["magic_hex"]),
                version=scenario["version"],
                kind=scenario["kind"],
                request_id=scenario["request_id"],
                payload_length=scenario["payload_length"],
            ),
        )
    for scenario in SCENARIOS["malformed_stop"]:
        check_malformed_stop(
            args.vmhost,
            scenario["label"],
            raw_header(
                version=scenario["version"],
                kind=scenario["kind"],
                request_id=scenario["request_id"],
                payload_length=scenario["payload_length"],
            ),
        )

    print(
        "VMHost control smoke passed: stop races, chunked frames (seeds 0/1/0x5eed), "
        "partial header/payload, malformed frames, and disconnect"
    )


if __name__ == "__main__":
    try:
        main()
    except (EOFError, OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"vmhost_control_race_smoke: {error}", file=sys.stderr)
        sys.exit(1)
