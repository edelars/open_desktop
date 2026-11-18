#!/usr/bin/env python3
"""Verify a saved S5-08 diskless shutdown series with the production comparator."""

import argparse
import json
from pathlib import Path
import subprocess


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("series", type=Path)
    parser.add_argument("--runner", default="target/release/od-runner")
    args = parser.parse_args()

    series = json.loads(args.series.read_bytes())
    if (
        series["scenario_id"] != "s5-08"
        or series["run_profile"] != "stage5_functional"
        or series["outcome"] != "passed"
        or series["requested_runs"] < 2
    ):
        parser.error("requires a passed s5-08 series with at least two guest boots")

    # The comparator loads and validates every raw run, hash, UART and stop.
    comparison = subprocess.run(
        [args.runner, "--compare-results", str(args.series), "--linux-stage5", "--json"],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    result = json.loads(comparison.stdout)
    if result["matches"] is not True or result["unexplained_differences"]:
        parser.error("the S5-08 series has unexplained differences")
    print(f"S5-08 shutdown and repeatability verified for {series['requested_runs']} boots")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
