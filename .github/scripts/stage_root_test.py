"""Select the lib test executable from Cargo JSON, never from a stale glob."""

import argparse
import json
from pathlib import Path
import shutil
import subprocess


TEST_NAME = "root_pipeline::root_target_tests::weekly_fetch_root_providers"


def select_executable(lines: list[str]) -> Path:
    executables = set()
    for line in lines:
        message = json.loads(line)
        if (message.get("reason") == "compiler-artifact"
                and message.get("target", {}).get("name") == "ltbox_patch"
                and message.get("profile", {}).get("test")
                and message.get("executable")):
            executables.add(Path(message["executable"]))
    if len(executables) != 1:
        raise ValueError(f"Expected exactly one ltbox-patch test executable, got {executables}")
    return executables.pop()


def stage(messages: Path, destination: Path) -> None:
    executable = select_executable(messages.read_text(encoding="utf-8").splitlines())
    listing = subprocess.check_output(
        [str(executable), "--list", "--ignored", "--exact", TEST_NAME], text=True
    )
    if listing.splitlines().count(f"{TEST_NAME}: test") != 1:
        raise ValueError(f"Required download contract is missing: {listing}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(executable, destination)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("messages", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    stage(args.messages, args.destination)
