"""Check the built image as dev, with the runtime PATH and no network."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tomllib


def run(command):
    return subprocess.run(command, check=True, timeout=60, stdin=subprocess.DEVNULL)


def check():
    print("Checking runtime as dev...", flush=True)
    if os.getuid() == 0:
        raise RuntimeError("Build checks must run as the runtime user, not root")
    for directory in ("/data/home", "/data/repos", "/data/t3home"):
        probe = Path(directory) / ".self-host-build-check"
        probe.write_text("writable")
        probe.unlink()
    run(["ps", "-eo", "pid,ppid,args"])
    installed = json.loads(subprocess.check_output(
        ["mise", "ls", "--current", "--json"], timeout=60, text=True
    ))
    with open("/opt/mise/config/config.toml", "rb") as source:
        config = tomllib.load(source)
    for tool in config["tools"]:
        versions = installed.get(tool, [])
        if not versions or any(not item.get("installed") or not os.access(
            item.get("install_path", ""), os.R_OK | os.X_OK
        ) for item in versions):
            raise RuntimeError(f"Installed tool is unavailable to dev: {tool}")
        print(f"Installed: {tool}", flush=True)
    commands = config.get("tasks", {}).get("check", {}).get("run", [])
    for index, command in enumerate(commands, start=1):
        print(f"Build check {index}/{len(commands)}: {command}", flush=True)
        run(["/bin/sh", "-ec", command])
    if not commands:
        print("No package checks configured. Add tasks.check commands to verify package behavior.", flush=True)
    print("Build checks passed.", flush=True)


if __name__ == "__main__":
    try:
        check()
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"Build check failed: {error}", file=sys.stderr, flush=True)
        sys.exit(1)
