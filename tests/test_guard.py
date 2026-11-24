import os
import subprocess
import sys

CHILD = """
import socket, subprocess
for attempt in (lambda: subprocess.run(["true"]),
                lambda: socket.socket(socket.AF_UNIX).connect("/run/user/0/hypr/x/.socket.sock")):
    try:
        attempt()
    except PermissionError as e:
        print(e)
"""


def test_a_process_the_tests_start_cannot_run_desktop_programs_or_reach_session_sockets(tmp_path):
    log = tmp_path / "touched"
    r = subprocess.run([sys.executable, "-c", CHILD], capture_output=True, text=True, timeout=30,
                       env={**os.environ, "HYPR_COMPUTER_TEST_GUARD": str(log)})
    refusals = r.stdout.splitlines()
    assert len(refusals) == 2 and all(line.endswith("Add a fake to tests/fakes.py (fake_bin) or a stub in "
                                                    "conftest.no_real_desktop, and point the test at it.")
                                      for line in refusals), r.stdout + r.stderr
    ran, connected = (line.split(" ", 1)[1] for line in log.read_text().splitlines())
    assert ran.startswith("ran /") and ran.endswith("/true")
    assert connected == "connected to '/run/user/0/hypr/x/.socket.sock'"
