"""Tiny client for an hc-screen control socket.  python3 hcctl.py <name> <command...>"""
import json, os, socket, sys


class Screen:
    def __init__(self, name):
        self.name = name
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.connect(f"{os.environ['XDG_RUNTIME_DIR']}/{name}.ctl")
        self.file = self.sock.makefile("rw")

    def __call__(self, command):
        self.file.write(command + "\n")
        self.file.flush()
        return json.loads(self.file.readline())


if __name__ == "__main__":
    print(json.dumps(Screen(sys.argv[1])(" ".join(sys.argv[2:]))))
