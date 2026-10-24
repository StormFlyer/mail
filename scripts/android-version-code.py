#!/usr/bin/env python3
"""Derive one Android version sequence for every release channel and workflow."""

import argparse
import re


def version_code(version: str) -> int:
    match = re.fullmatch(
        r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
        r"(?:-(alpha|beta|rc)\.([1-9][0-9]*))?",
        version,
    )
    if not match:
        raise ValueError("Android versions must be X.Y.Z or X.Y.Z-{alpha,beta,rc}.N")
    major, minor, patch = map(int, match.group(1, 2, 3))
    stage, number = match.group(4, 5)
    if major > 20 or minor > 99 or patch > 99 or (number and int(number) > 999):
        raise ValueError("Android versions support major 0–20, minor/patch 0–99, and prerelease 1–999")
    # Reserve 10,000 codes for each patch. This stays below Play's 2.1B limit.
    # alpha.999 < beta.1 < rc.1 < stable < next patch's alpha.1.
    suffix = {"alpha": 0, "beta": 1000, "rc": 2000}.get(stage, 9999)
    return major * 100_000_000 + minor * 1_000_000 + patch * 10_000 + suffix + int(number or 0)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version")
    args = parser.parse_args()
    try:
        print(version_code(args.version))
    except ValueError as error:
        raise SystemExit(str(error)) from error
