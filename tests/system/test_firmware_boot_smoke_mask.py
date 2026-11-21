#!/usr/bin/env python3
"""Self-test of the BS/RT CRC mask used by the firmware COM1 smoke comparisons (ISSUE-611)."""

from __future__ import annotations

import unittest

from firmware_boot_smoke import check_transcript, mask_layout_crcs

LINE = (
    b"OD-FW S7-05 table CRC ST=0x0000000014CB0AB4 "
    b"BS=0x000000001FC7EB72 RT=0x000000006DBBAFA8\r\n"
)
OTHER = b"OD-FW MEM type=0x0000000000000003 base=0x0000000000200000 pages=0x0000000000000055\r\n"
TRANSCRIPT = OTHER + LINE + b"OD-FW done\r\n"


def transcript(line: bytes) -> bytes:
    return OTHER + line + b"OD-FW done\r\n"


class MaskLayoutCrcs(unittest.TestCase):
    def test_masks_both_values_and_keeps_the_rest(self) -> None:
        self.assertEqual(
            mask_layout_crcs(LINE),
            b"OD-FW S7-05 table CRC ST=0x0000000014CB0AB4 BS=0x<layout> RT=0x<layout>\r\n",
        )
        self.assertEqual(mask_layout_crcs(OTHER), OTHER)

    def test_different_bs_and_rt_values_are_accepted(self) -> None:
        other = LINE.replace(b"1FC7EB72", b"E419FB7C").replace(b"6DBBAFA8", b"5709727A")
        check_transcript(transcript(other), TRANSCRIPT, "layout values only")

    def test_system_table_crc_stays_exact(self) -> None:
        with self.assertRaises(RuntimeError):
            check_transcript(transcript(LINE.replace(b"14CB0AB4", b"14CB0AB5")), TRANSCRIPT, "ST")

    def test_other_lines_stay_exact(self) -> None:
        changed = TRANSCRIPT.replace(b"pages=0x0000000000000055", b"pages=0x0000000000000054")
        with self.assertRaises(RuntimeError):
            check_transcript(changed, TRANSCRIPT, "MEM")

    def test_missing_or_malformed_crc_line_is_rejected(self) -> None:
        for label, line in (
            ("missing", b""),
            ("not hex", LINE.replace(b"BS=0x0", b"BS=0xZ", 1)),
            ("short value", LINE.replace(b"RT=0x000000006DBBAFA8", b"RT=0x6DBBAFA8")),
            ("duplicated", LINE + LINE),
        ):
            with self.subTest(label), self.assertRaises(RuntimeError):
                check_transcript(transcript(line), TRANSCRIPT, label)


if __name__ == "__main__":
    unittest.main()
