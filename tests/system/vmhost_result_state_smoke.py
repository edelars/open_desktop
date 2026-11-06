#!/usr/bin/env python3
"""Validate complete VMHost RESULT state over V0 and through the launcher."""

import argparse
import errno
import json
import os
from pathlib import Path
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time


MAGIC = b"ODCP"
VERSION = 0
HELLO, READY, RUN, RUN_ACK, STOP, STOP_ACK, RESULT = 1, 2, 3, 4, 5, 6, 7
MAX_PAYLOAD = 8192
RESULT_BYTES = 64
RESULT_FORMAT = "<6QIIQ"
VMHOST_HANDSHAKE_TIMEOUT_SECONDS = 30
CONTROL_RESPONSE_TIMEOUT_SECONDS = 5
LOOP = bytes.fromhex("48b8010000000000000075f4")
INITIAL_GPRS = (
    0xF00DFACE12345678,
    0x1122334455667788,
    0x8877665544332211,
    0xCAFEBABE0BADF00D,
)
INITIAL_RFLAGS = 0x202
# The launcher CLI currently fixes the RUN input RFLAGS to reserved bit 1.
LAUNCHER_RFLAGS = 2
STOP_BUDGET = (1 << 64) - 1
FIXED_BUDGETS = (0, 1, 2, 3, 4)


def frame(kind, request_id, payload=b""):
    if len(payload) > MAX_PAYLOAD:
        raise ValueError("payload exceeds protocol limit")
    return MAGIC + struct.pack("<HHII", VERSION, kind, request_id, len(payload)) + payload


def read_exact(channel, length):
    data = bytearray()
    while len(data) < length:
        chunk = channel.recv(length - len(data))
        if not chunk:
            raise EOFError("VMHost closed the control channel during a frame")
        data.extend(chunk)
    return bytes(data)


def read_frame(channel):
    raw_header = read_exact(channel, 16)
    magic, version, kind, request_id, payload_length = struct.unpack("<4sHHII", raw_header)
    if magic != MAGIC or version != VERSION or payload_length > MAX_PAYLOAD:
        raise RuntimeError("invalid VMHost response frame header")
    return kind, request_id, read_exact(channel, payload_length)


def run_payload(code, budget, gprs=INITIAL_GPRS, rip=0, rflags=INITIAL_RFLAGS):
    if len(gprs) != 4:
        raise ValueError("RUN requires four GPRs")
    return struct.pack("<IQ6Q", len(code), budget, *gprs, rip, rflags) + code


def decode_result(
    kind,
    request_id,
    payload,
    *,
    expected_request_id,
    initial_gprs,
    initial_rflags,
    budget,
    expected_reason,
):
    if kind != RESULT:
        raise ValueError(f"expected RESULT kind {RESULT}, received {kind}")
    if request_id != expected_request_id:
        raise ValueError(f"RESULT request ID {request_id} differs from {expected_request_id}")
    if len(payload) != RESULT_BYTES:
        raise ValueError(f"RESULT payload length must be {RESULT_BYTES}, got {len(payload)}")
    (
        rax,
        rcx,
        rdx,
        rbx,
        rip,
        rflags,
        reason,
        reserved,
        retired,
    ) = struct.unpack(RESULT_FORMAT, payload)
    if reason != expected_reason:
        raise ValueError(f"RESULT exit reason {reason} differs from {expected_reason}")
    if reserved != 0:
        raise ValueError(f"RESULT reserved field must be zero, got {reserved}")
    if retired > budget:
        raise ValueError(f"RESULT retired {retired} instructions beyond budget {budget}")
    if expected_reason == 1 and retired != budget:
        raise ValueError(f"budget RESULT retired {retired}, expected exact budget {budget}")
    expected_rax = initial_gprs[0] if retired == 0 else 1
    expected_rip = 0 if retired % 2 == 0 else 10
    if (rax, rcx, rdx, rbx) != (
        expected_rax,
        initial_gprs[1],
        initial_gprs[2],
        initial_gprs[3],
    ):
        raise ValueError("RESULT GPR state differs from the loop's architectural state")
    if rip != expected_rip:
        raise ValueError(f"RESULT RIP {rip} differs from retirement parity {expected_rip}")
    if rflags != initial_rflags:
        raise ValueError(f"RESULT RFLAGS {rflags:#x} differs from {initial_rflags:#x}")
    return {
        "gpr": [rax, rcx, rdx, rbx],
        "rip": rip,
        "rflags": rflags,
        "reason": reason,
        "reserved": reserved,
        "instructions_executed": retired,
        "request_id": request_id,
        "payload_length": len(payload),
    }


def expected_result(budget, retired=None, reason=1, initial_gprs=INITIAL_GPRS):
    if retired is None:
        retired = budget
    rax = initial_gprs[0] if retired == 0 else 1
    rip = 0 if retired % 2 == 0 else 10
    return struct.pack(
        RESULT_FORMAT,
        rax,
        initial_gprs[1],
        initial_gprs[2],
        initial_gprs[3],
        rip,
        INITIAL_RFLAGS,
        reason,
        0,
        retired,
    )


def process_group_exists(process_group_id):
    try:
        os.killpg(process_group_id, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def terminate_group(process_group_id):
    try:
        os.killpg(process_group_id, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        os.killpg(process_group_id, signal.SIGKILL)
    except ProcessLookupError:
        pass


def open_child_logs(directory, label):
    stdout_path = directory / f"{label}.stdout.log"
    stderr_path = directory / f"{label}.stderr.log"
    if stdout_path.is_symlink() or stderr_path.is_symlink():
        raise RuntimeError("child diagnostics log path is a symlink")
    return stdout_path.open("xb"), stderr_path.open("xb")


def start_vmhost(vmhost, diagnostics_dir, label):
    parent, child = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    stdout = stderr = None
    try:
        stdout, stderr = open_child_logs(diagnostics_dir, label)
        process = subprocess.Popen(
            [str(vmhost), "--socket-fd", str(child.fileno())],
            pass_fds=(child.fileno(),),
            stdin=subprocess.DEVNULL,
            stdout=stdout,
            stderr=stderr,
            start_new_session=True,
        )
    except Exception:
        parent.close()
        child.close()
        if stdout is not None:
            stdout.close()
        if stderr is not None:
            stderr.close()
        raise
    finally:
        if stdout is not None:
            stdout.close()
        if stderr is not None:
            stderr.close()
    child.close()
    parent.settimeout(VMHOST_HANDSHAKE_TIMEOUT_SECONDS)
    try:
        parent.sendall(frame(HELLO, 0))
        response = read_frame(parent)
        if response != (READY, 0, struct.pack("<I", 1)):
            raise RuntimeError(f"{label}: invalid READY after HELLO: {response!r}")
    except socket.timeout as error:
        parent.close()
        terminate_group(process.pid)
        process.wait()
        raise RuntimeError(
            f"{label}: timed out after {VMHOST_HANDSHAKE_TIMEOUT_SECONDS}s waiting READY after HELLO"
        ) from error
    except Exception:
        parent.close()
        terminate_group(process.pid)
        process.wait()
        raise
    parent.settimeout(CONTROL_RESPONSE_TIMEOUT_SECONDS)
    return process, parent


def send_run(channel, request_id, budget):
    channel.sendall(frame(RUN, request_id, run_payload(LOOP, budget)))
    if read_frame(channel) != (RUN_ACK, request_id, struct.pack("<I", 0)):
        raise RuntimeError("VMHost did not acknowledge the RESULT smoke RUN")


def wait_for_successful_helper(process, label):
    try:
        status = process.wait(timeout=5)
    except subprocess.TimeoutExpired as error:
        terminate_group(process.pid)
        process.wait()
        raise RuntimeError(f"{label}: VMHost did not exit after RESULT") from error
    if status != 0:
        raise RuntimeError(f"{label}: VMHost exited with status {status}")
    if process_group_exists(process.pid):
        terminate_group(process.pid)
        raise RuntimeError(f"{label}: VMHost left a child in its process group")


def run_raw_budget(vmhost, diagnostics_dir, budget, request_id):
    label = f"raw-budget-{budget}"
    process, channel = start_vmhost(vmhost, diagnostics_dir, label)
    try:
        send_run(channel, request_id, budget)
        result = decode_result(
            *read_frame(channel),
            expected_request_id=request_id,
            initial_gprs=INITIAL_GPRS,
            initial_rflags=INITIAL_RFLAGS,
            budget=budget,
            expected_reason=1,
        )
        wait_for_successful_helper(process, label)
        return result
    finally:
        channel.close()
        if process_group_exists(process.pid):
            terminate_group(process.pid)
        if process.poll() is None:
            process.wait()


def run_raw_stop(vmhost, diagnostics_dir, delay_seconds, label, request_id):
    process, channel = start_vmhost(vmhost, diagnostics_dir, label)
    ack_order = ["RUN_ACK"]
    try:
        send_run(channel, request_id, STOP_BUDGET)
        if delay_seconds:
            time.sleep(delay_seconds)
        channel.sendall(frame(STOP, request_id))
        kind, frame_request_id, payload = read_frame(channel)
        if (kind, frame_request_id, payload) != (
            STOP_ACK,
            request_id,
            struct.pack("<I", 0),
        ):
            raise RuntimeError(f"{label}: expected accepted STOP_ACK before RESULT")
        ack_order.append("STOP_ACK")
        result = decode_result(
            *read_frame(channel),
            expected_request_id=request_id,
            initial_gprs=INITIAL_GPRS,
            initial_rflags=INITIAL_RFLAGS,
            budget=STOP_BUDGET,
            expected_reason=2,
        )
        ack_order.append("RESULT")
        wait_for_successful_helper(process, label)
        result["ack_order"] = ack_order
        result["stop_observation"] = (
            "early_stop_no_guest_progress"
            if result["instructions_executed"] == 0
            else "stop_after_observed_guest_progress"
        )
        return result
    finally:
        channel.close()
        if process_group_exists(process.pid):
            terminate_group(process.pid)
        if process.poll() is None:
            process.wait()


def run_launcher_budget(launcher, vmhost, diagnostics_dir, budget):
    label = f"launcher-budget-{budget}"
    stdout, stderr = open_child_logs(diagnostics_dir, label)
    argv = [
        str(launcher),
        "--vmhost",
        str(vmhost),
        "--hex",
        LOOP.hex(),
        "--budget",
        str(budget),
        "--rax",
        str(INITIAL_GPRS[0]),
        "--rcx",
        str(INITIAL_GPRS[1]),
        "--rdx",
        str(INITIAL_GPRS[2]),
        "--rbx",
        str(INITIAL_GPRS[3]),
        "--rip",
        "0",
    ]
    try:
        process = subprocess.Popen(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=stdout,
            stderr=stderr,
            start_new_session=True,
        )
    finally:
        stdout.close()
        stderr.close()
    try:
        status = process.wait(timeout=10)
    except subprocess.TimeoutExpired as error:
        terminate_group(process.pid)
        process.wait()
        raise RuntimeError(f"{label}: launcher/VMHost child group timed out") from error
    if status != 0:
        raise RuntimeError(f"{label}: launcher exited with status {status}")
    if process_group_exists(process.pid):
        terminate_group(process.pid)
        raise RuntimeError(f"{label}: launcher left a child in its process group")

    report = json.loads((diagnostics_dir / f"{label}.stdout.log").read_text())
    expected_rax = INITIAL_GPRS[0] if budget == 0 else 1
    if report != {
        "gpr": [expected_rax, INITIAL_GPRS[1], INITIAL_GPRS[2], INITIAL_GPRS[3]],
        "instructions_executed": budget,
        "reason": 1,
        "rflags": LAUNCHER_RFLAGS,
        "rip": 0 if budget % 2 == 0 else 10,
    }:
        raise RuntimeError(f"{label}: launcher RESULT projection differs: {report!r}")
    return report


def run_smoke(launcher, vmhost, diagnostics_dir):
    if diagnostics_dir.is_symlink() or not diagnostics_dir.is_dir():
        raise RuntimeError("diagnostics directory must be an existing non-symlink directory")
    results = {"raw_budget": [], "launcher_budget": [], "stop": []}
    for index, budget in enumerate(FIXED_BUDGETS, start=1):
        results["raw_budget"].append(
            run_raw_budget(vmhost, diagnostics_dir, budget, 0x380D_0000 + index)
        )
        results["launcher_budget"].append(
            run_launcher_budget(launcher, vmhost, diagnostics_dir, budget)
        )
    results["stop"].append(
        run_raw_stop(vmhost, diagnostics_dir, 0, "raw-stop-immediate", 0x380D_1001)
    )
    results["stop"].append(
        run_raw_stop(vmhost, diagnostics_dir, 0.005, "raw-stop-delayed", 0x380D_1002)
    )
    print("VMHost RESULT state smoke passed: raw V0 budgets 0/1/2/3/4, launcher projections, immediate/delayed STOP")
    print("vmhost_result_state_observation=" + json.dumps(results, sort_keys=True, separators=(",", ":")))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    root = Path(__file__).resolve().parents[2]
    parser.add_argument("launcher", type=Path, nargs="?", default=root / "apps" / ".build" / "debug" / "open-desktop-launcher-smoke")
    parser.add_argument("vmhost", type=Path, nargs="?", default=root / "apps" / ".build" / "debug" / "od-vmhost")
    parser.add_argument("--diagnostics-dir", type=Path, help="existing directory for child logs (default: a temporary directory removed afterwards)")
    args = parser.parse_args(argv)
    if args.diagnostics_dir is not None:
        run_smoke(args.launcher, args.vmhost, args.diagnostics_dir.resolve())
        return
    with tempfile.TemporaryDirectory(prefix="od-result-state-") as temporary:
        run_smoke(args.launcher, args.vmhost, Path(temporary).resolve())


if __name__ == "__main__":
    try:
        main()
    except (EOFError, OSError, RuntimeError, ValueError, json.JSONDecodeError, subprocess.SubprocessError) as error:
        print(f"vmhost_result_state_smoke: {error}", file=sys.stderr)
        sys.exit(1)
