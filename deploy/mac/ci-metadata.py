#!/usr/bin/env python3
"""Resolve one version/build for every job; tag filters are globs, not SemVer."""
import argparse
import os
from pathlib import Path
import re

VERSION = r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"


def resolve(event, ref, run_number, run_attempt, cargo_text):
    if event == "push":
        match = re.fullmatch(r"refs/tags/v(" + VERSION + r")", ref)
        if not match:
            raise ValueError("Publishing requires a tag exactly like v1.2.3 (no prerelease suffix).")
        version = match[1]
    elif event == "workflow_dispatch":
        section = cargo_text.split("[workspace.package]", 1)[1].split("[", 1)[0]
        version = re.search(r'^version\s*=\s*"([^"]+)"', section, re.M)[1]
        if not re.fullmatch(VERSION, version):
            raise ValueError("Workspace version must be X.Y.Z.")
    else:
        raise ValueError("Only release tags and manual builds are supported.")
    if run_number < 1 or not 1 <= run_attempt < 100:
        raise ValueError("Invalid GitHub run number or attempt.")
    # Starts above pre-CI build 108. Reruns get new immutable archive URLs.
    build = 1000 + run_number * 100 + run_attempt
    return {"version": version, "build": str(build), "publish": str(event == "push").lower()}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check-tag", help="Validate a tag without GitHub environment variables")
    parser.add_argument("--build-only", action="store_true",
                        help="Resolve like a manual run, which never publishes (Windows pull requests)")
    args = parser.parse_args()
    if args.build_only:
        event = "workflow_dispatch"
    else:
        event = "push" if args.check_tag else os.environ["GITHUB_EVENT_NAME"]
    ref = "refs/tags/" + args.check_tag if args.check_tag else os.environ.get("GITHUB_REF", "")
    try:
        values = resolve(event, ref, int(os.environ.get("GITHUB_RUN_NUMBER", "1")),
                         int(os.environ.get("GITHUB_RUN_ATTEMPT", "1")), Path("Cargo.toml").read_text())
    except (ValueError, IndexError, TypeError) as error:
        raise SystemExit(str(error)) from error
    output = "".join(f"{key}={value}\n" for key, value in values.items())
    if "GITHUB_OUTPUT" in os.environ:
        with open(os.environ["GITHUB_OUTPUT"], "a") as stream:
            stream.write(output)
    print(output, end="")
