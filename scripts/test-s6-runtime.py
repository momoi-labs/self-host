#!/usr/bin/env python3
"""Exercise the built s6 tools without changing Host services or accounts."""

import os
from pathlib import Path
import shlex
import signal
import subprocess
import sys
import tempfile
import time


def check(bundle):
    binaries = Path(bundle).resolve() / "bin"
    environment = {"PATH": f"{binaries}:/usr/bin:/bin"}

    def run(tool, *arguments):
        return subprocess.check_output(
            [str(binaries / tool), *map(str, arguments)],
            env=environment, stderr=subprocess.STDOUT, timeout=10, text=True,
        ).strip()

    def wait_for(predicate, message):
        deadline = time.monotonic() + 5
        while not predicate():
            if time.monotonic() > deadline:
                raise AssertionError(message)
            time.sleep(0.02)

    def gone(pid):
        try:
            os.kill(pid, 0)
            return False
        except ProcessLookupError:
            return True

    with tempfile.TemporaryDirectory(prefix="self-host-s6-test-") as temporary:
        root = Path(temporary)
        scan = root / "scan"
        service = scan / "app"
        service.mkdir(parents=True)
        (service / "run").write_text("#!/bin/sh\necho $$ > runner.pid\n/bin/sleep 300 &\necho $! > child.pid\nwait\n")
        cleanup = "import os,signal,sys;\ntry: os.killpg(int(sys.argv[1]),signal.SIGKILL)\nexcept ProcessLookupError: pass"
        (service / "finish").write_text(f"#!/bin/sh\nexec {shlex.quote(sys.executable)} -c {shlex.quote(cleanup)} \"$4\"\n")
        (service / "finish").chmod(0o700)
        (service / "flag-timeout-killpg").touch()
        (service / "timeout-kill").write_text("1000\n")
        (service / "run").chmod(0o700)
        (service / "down").touch()
        with (root / "scanner.log").open("w+") as log:
            scanner = subprocess.Popen(
                [str(binaries / "s6-svscan"), str(scan)],
                env=environment, stdout=log, stderr=log,
            )
            try:
                deadline = time.monotonic() + 5
                while not (service / "supervise/control").exists():
                    if scanner.poll() is not None or time.monotonic() > deadline:
                        raise AssertionError("s6 did not discover the service")
                    time.sleep(0.02)
                run("s6-svc", "-U", service)
                run("s6-svwait", "-u", "-t", "5000", service)
                assert run("s6-svstat", "-o", "up", service) == "true"
                wait_for(lambda: (service / "child.pid").exists(), "service did not spawn its child")
                runner = int((service / "runner.pid").read_text())
                child = int((service / "child.pid").read_text())
                assert os.getpgid(child) == runner
                os.kill(runner, signal.SIGKILL)
                wait_for(lambda: int((service / "runner.pid").read_text()) != runner, "s6 did not restart after a crash")
                wait_for(lambda: gone(child), "finish left the old child running")
                child = int((service / "child.pid").read_text())
                run("s6-svc", "-D", service)
                run("s6-svwait", "-D", "-t", "5000", service)
                wait_for(lambda: gone(child), "stop left its child running")
                assert (service / "down").exists()
                run("s6-svscanctl", "-t", scan)
                assert scanner.wait(timeout=10) == 0, "scanner shutdown failed"
                log.seek(0)
                assert "unable to check internal pipes" not in log.read()
            except BaseException:
                log.seek(0)
                print(log.read(), file=sys.stderr)
                raise
            finally:
                if scanner.poll() is None:
                    scanner.terminate()
                    try:
                        scanner.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        scanner.kill()
                        scanner.wait()
                subprocess.run(
                    [str(binaries / "s6-svc"), "-dx", str(service)],
                    env=environment, stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL, timeout=5,
                )
    print("s6 start, crash recovery, child cleanup, persistent stop and scanner shutdown passed")


if __name__ == "__main__":
    check(sys.argv[1])
