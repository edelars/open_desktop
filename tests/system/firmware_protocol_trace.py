#!/usr/bin/env python3
"""S7-31q: the protocol and service call trace of the unified loop (pure helpers, no process).

The runner watches the entry of the ROM functions below (`od-runner boot-firmware --cpu-loop unified
--trace-call NAME=0xADDR[:guid=N][:name=N]`; N is the argument, 0..3 = RCX, RDX, R8, R9). Each call is
recorded with the GUID and the UCS-2 name its arguments point to; values are never read. This module
builds the arguments from the ROM symbols and folds the calls into a report. Names for GUIDs come from the
table below (UEFI 2.11 protocol and variable GUIDs, plus the Linux initrd media GUID the kernel defines);
an unknown GUID is reported as such, never guessed.
"""
from __future__ import annotations

import uuid

# trace name -> (ROM symbol or None, runtime services table slot or None, guid argument, name argument).
# Microsoft x64 ABI argument numbers (0 = RCX, 1 = RDX, 2 = R8, 3 = R9). The runtime driver (od-runtime.efi)
# replaces the ROM runtime services after the boot, so those services are also followed through the slots
# of EFI_RUNTIME_SERVICES (UEFI 2.11: GetTime 24, SetVirtualAddressMap 56, GetVariable 72,
# GetNextVariableName 80, SetVariable 88); the ROM symbol still catches the firmware's own direct calls.
SERVICES = {
    "HandleProtocol": ("od_firmware::bs::protocol::handle_protocol", None, 1, None),
    "OpenProtocol": ("od_firmware::bs::protocol::open_protocol", None, 1, None),
    "LocateProtocol": ("od_firmware::bs::protocol::locate_protocol", None, 0, None),
    "LocateHandle": ("od_firmware::bs::protocol::query::locate_handle", None, 1, None),
    "LocateHandleBuffer": ("od_firmware::bs::protocol::query::locate_handle_buffer", None, 1, None),
    "LocateDevicePath": ("od_firmware::bs::protocol::query::locate_device_path", None, 0, None),
    "InstallProtocolInterface": ("od_firmware::bs::protocol::install_protocol_interface", None, 1, None),
    "GetMemoryMap": ("od_firmware::bs::memory::get_memory_map", None, None, None),
    "ExitBootServices": ("od_firmware::bs::exit::exit_boot_services", None, None, None),
    "GetVariable": ("od_firmware::rt::variable::get_variable", 72, 1, 0),
    "SetVariable": ("od_firmware::rt::variable::set_variable", 88, 1, 0),
    # The name and GUID are in/out buffers: at the entry they hold the previous variable.
    "GetNextVariableName": ("od_firmware::rt::variable::get_next_variable_name", 80, 2, 1),
    "GetTime": (None, 24, None, None),
    "SetVirtualAddressMap": (None, 56, None, None),
}
EXIT_CALL = "ExitBootServices"

GUID_NAMES = {
    "5b1b31a1-9562-11d2-8e3f-00a0c969723b": "EFI_LOADED_IMAGE_PROTOCOL",
    "bc62157e-3e33-4fec-9920-2d3b36d750df": "EFI_LOADED_IMAGE_DEVICE_PATH_PROTOCOL",
    "09576e91-6d3f-11d2-8e39-00a0c969723b": "EFI_DEVICE_PATH_PROTOCOL",
    "964e5b22-6459-11d2-8e39-00a0c969723b": "EFI_SIMPLE_FILE_SYSTEM_PROTOCOL",
    "964e5b21-6459-11d2-8e39-00a0c969723b": "EFI_BLOCK_IO_PROTOCOL",
    "9042a9de-23dc-4a38-96fb-7aded080516a": "EFI_GRAPHICS_OUTPUT_PROTOCOL",
    "3152bca5-eade-433d-862e-c01cdc291f44": "EFI_RNG_PROTOCOL",
    "4006c0c1-fcb3-403e-996d-4a6c8724e06d": "EFI_LOAD_FILE2_PROTOCOL",
    "56ec3091-954c-11d2-8e3f-00a0c969723b": "EFI_LOAD_FILE_PROTOCOL",
    "607f766c-7455-42be-930b-e4d76db2720f": "EFI_TCG2_PROTOCOL",
    "387477c1-69c7-11d2-8e39-00a0c969723b": "EFI_SIMPLE_TEXT_INPUT_PROTOCOL",
    "dd9e7534-7762-4698-8c14-f58517a625aa": "EFI_SIMPLE_TEXT_INPUT_EX_PROTOCOL",
    "387477c2-69c7-11d2-8e39-00a0c969723b": "EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL",
    "4cf5b200-68b8-4ca5-9eec-b23e3f50029a": "EFI_PCI_IO_PROTOCOL",
    "2f707ebb-4a1a-11d4-9a38-0090273fc14d": "EFI_PCI_ROOT_BRIDGE_IO_PROTOCOL",
    "8be4df61-93ca-11d2-aa0d-00e098032b8c": "EFI_GLOBAL_VARIABLE (variable vendor GUID)",
    "5568e427-68fc-4f3d-ac74-ca555231cc68": "LINUX_EFI_INITRD_MEDIA_GUID (defined by the Linux kernel, not UEFI)",
    # Names from the Linux kernel's include/linux/efi.h (linux-source-7.0.0), read for this slice.
    "f4560cf6-40ec-4b4a-a192-bf1d57d0b189": "EFI_MEMORY_ATTRIBUTE_PROTOCOL (name from Linux include/linux/efi.h)",
    "96751a3d-72f4-41a6-a794-ed5d0e67ae6b": "EFI_CC_MEASUREMENT_PROTOCOL (name from Linux include/linux/efi.h)",
    "05c99a21-c70f-4ad2-8a5f-35df3343f51e": "EFI_DEVICE_PATH_FROM_TEXT_PROTOCOL (name from Linux include/linux/efi.h)",
    "bd8c1056-9f36-44ec-92a8-a6337f817986": "EFI_EDID_ACTIVE_PROTOCOL (name from Linux include/linux/efi.h)",
    "c5a010fe-38a7-4531-8a4a-0500d2fd1649": "OVMF_SEV_MEMORY_ACCEPTANCE_PROTOCOL (OVMF, name from Linux include/linux/efi.h)",
    "d3b36f2c-d551-11d4-9a46-0090273fc14d": "EFI_CONSOLE_OUT_DEVICE_GUID (name from Linux include/linux/efi.h)",
    "1ce1e5bc-7ceb-42f2-81e5-8aadf180f57b": "LINUX_EFI_RANDOM_SEED_TABLE_GUID (name from Linux include/linux/efi.h)",
    "e20939be-32d4-41be-a150-897f85d49829": "MEMORY_ONLY_RESET_CONTROL_GUID (name from Linux drivers/firmware/efi/libstub/tpm.c:20-21; vendor GUID of MemoryOverwriteRequestControl)",
}


def trace_arguments(symbols: list[tuple[int, str]]) -> tuple[list[str], list[str]]:
    """The --trace-call arguments for the ROM symbols, and the services whose symbol was not found."""
    by_name = {name: address for address, name in symbols}
    arguments, missing = [], []
    for name, (symbol, slot, guid_arg, name_arg) in SERVICES.items():
        address = by_name.get(symbol) if symbol else 0
        if address is None:
            if slot is None:
                missing.append(name)
                continue
            address = 0  # only the runtime services slot is followed
        spec = f"{name}=0x{address:x}"
        if guid_arg is not None:
            spec += f":guid={guid_arg}"
        if name_arg is not None:
            spec += f":name={name_arg}"
        if slot is not None:
            spec += f":slot={slot}"
        arguments += ["--trace-call", spec]
    return arguments, missing


def system_table_address(com1: bytes) -> int | None:
    """The EFI_SYSTEM_TABLE address the firmware prints early ('... ST=0x...'); None when absent."""
    import re
    # Not the "table CRC ST=0x..." line: that one is a checksum.
    match = re.search(rb"utilities verified ST=0x([0-9A-Fa-f]{16})", com1)
    return int(match.group(1), 16) if match else None


def guid_text(raw_hex: str | None) -> str | None:
    """A GUID as the guest stores it (16 bytes, mixed endian) in the usual text form."""
    return None if raw_hex is None else str(uuid.UUID(bytes_le=bytes.fromhex(raw_hex)))


def origin(call: dict, rom: tuple[int, int]) -> str:
    """Who called: the firmware itself (the return address is in the ROM) or guest code (the kernel stub or the kernel)."""
    caller = call.get("caller")
    if caller is None:
        return "unknown"
    return "firmware" if rom[0] <= int(caller, 16) < rom[1] else "guest"


def summarize(trace: dict, rom: tuple[int, int]) -> dict:
    """Fold the runner's call list: counts per call, caller (firmware or guest), GUID and variable name, before
    and after the guest's ExitBootServices. `rom` is the [start, end) address range of the ROM image."""
    calls = trace["calls"]
    exits = [call["retired"] for call in calls if call["name"] == EXIT_CALL and origin(call, rom) == "guest"]
    # The last guest call is the one that succeeded (a failed call, for example a stale map key, is retried).
    boundary = max(exits) if exits else None
    phases: dict[str, dict] = {"before_exit_boot_services": {}, "from_exit_boot_services": {}}
    for call in calls:
        phase = "before_exit_boot_services" if boundary is None or call["retired"] < boundary else "from_exit_boot_services"
        guid = guid_text(call["guid"])
        key = (call["name"], origin(call, rom), guid, call["variable_name"])
        slot = phases[phase].setdefault(key, {"calls": 0, "first_retired": call["retired"], "cpl_values": set()})
        slot["calls"] += 1
        slot["cpl_values"].add(call["cpl"])
    folded = {}
    for phase, entries in phases.items():
        rows = []
        for (name, caller, guid, variable), slot in entries.items():
            row = {"call": name, "caller": caller, "calls": slot["calls"], "first_retired": slot["first_retired"],
                   "cpl": sorted(slot["cpl_values"])}
            if guid is not None:
                row["guid"] = guid
                row["guid_name"] = GUID_NAMES.get(guid, "unknown (not in the table of this script)")
            if variable is not None:
                row["variable_name"] = variable
            rows.append(row)
        folded[phase] = sorted(rows, key=lambda row: row["first_retired"])
    totals: dict[str, dict[str, int]] = {}
    for call in calls:
        by = totals.setdefault(call["name"], {})
        by[origin(call, rom)] = by.get(origin(call, rom), 0) + 1
    return {"total_calls": trace["total"], "dropped": trace["dropped"], "calls_by_service_and_caller": dict(sorted(totals.items())),
            "exit_boot_services_at_retired": boundary, "phases": folded}
