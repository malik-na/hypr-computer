"""Finds and acts on elements in one window's AT-SPI accessibility tree, for hypr-computer's ui_find and ui_act.

The server runs this with the system Python, which has PyGObject (gi); the server's own environment does not.
It reads one JSON request on stdin and writes one JSON reply on stdout.

Positions are window-relative logical pixels. Apps report them inconsistently on Wayland (GTK4 gives zeros,
Chromium gives page content in device pixels), so each window's extents are checked against the size Hyprland
gives it, and a position is left out rather than guessed when they don't fit.
"""
import json
import re
import sys
import time
import warnings

STATES = ("focused", "checked", "pressed", "selected", "expanded", "indeterminate", "editable", "required",
          "invalid-entry", "busy")
SLACK = 2  # logical pixels of rounding allowed when comparing extents with the window's size


class Refused(Exception):
    pass


class Window:
    def __init__(self, atspi, req: dict):
        self.atspi, self.req = atspi, req
        self.w, self.h = req["size"]
        self.scale = req["scale"]
        self.deny = re.compile(req["deny"], re.I) if req.get("deny") else None
        self.end = time.monotonic() + req.get("deadline", 3)
        self.budget = req.get("max_nodes", 5000)
        self.nodes = 0
        self.frame, self.div = self.find_frame()

    def find_frame(self):
        desktop = self.atspi.get_desktop(0)
        apps = (desktop.get_child_at_index(i) for i in range(desktop.get_child_count()))
        app = next((a for a in apps if a is not None and a.get_process_id() == self.req["pid"]), None)
        if app is None:
            raise Refused("this window's app has no accessibility tree. Apps started without accessibility on "
                          "may not have one (open_url and launch_app turn it on); use find_text instead.")
        frames = [f for f in (app.get_child_at_index(i) for i in range(app.get_child_count()))
                  if f is not None and not self.denied(f.get_name())]
        title = self.req["title"]
        named = [f for f in frames if f.get_name() == title]
        prefixed = [f for f in frames if title and (f.get_name() or "").startswith(title)]
        sized = [f for f in frames if self.window_div(f) is not None]
        active = [f for f in frames if "active" in states(f)]
        pick = next((c for c in (named, prefixed, frames, sized, active) if len(c) == 1), None)
        frame = (named or pick or [None])[0]
        if frame is None:
            raise Refused(f"can't tell which of the app's {len(frames)} accessible windows is this one")
        return frame, self.window_div(frame)

    def denied(self, name: str) -> bool:
        return bool(self.deny and name and self.deny.search(name))

    def fits(self, x, y, w, h) -> bool:
        return -SLACK <= x and -SLACK <= y and x + w <= self.w + SLACK and y + h <= self.h + SLACK

    def window_div(self, frame):
        """Extent units per logical pixel for the window, or None if its extents don't match its size."""
        x, y, w, h = extents(self.atspi, frame) or (0, 0, 0, 0)
        for div in {1, self.scale}:
            if abs(w / div - self.w) <= SLACK and abs(h / div - self.h) <= SLACK:
                return div
        return None

    def own_div(self, node, parent_div):
        """Chromium reports its page in device pixels inside a window reported in logical ones. Such a page
        is too big for the window in logical pixels and, in device pixels, fills its width down to the bottom
        edge. Any other element that doesn't fit leaves its subtree without positions, unless the scale is 1."""
        if parent_div != 1 or self.scale == 1:
            return parent_div
        box = extents(self.atspi, node)
        if box is None or box[2] <= 0 or box[3] <= 0 or self.fits(*box):
            return parent_div
        x, y, w, h = (v / self.scale for v in box)
        if abs(w - self.w) <= SLACK and abs(y + h - self.h) <= SLACK and self.fits(x, y, w, h):
            return self.scale
        return None

    def centre(self, node, div):
        box = extents(self.atspi, node) if div else None
        if not box or box[2] <= 0 or box[3] <= 0:
            return None
        x, y, w, h = (v / div for v in box)
        left, top, right, bottom = max(x, 0), max(y, 0), min(x + w, self.w), min(y + h, self.h)
        return [round((left + right) / 2, 1), round((top + bottom) / 2, 1)] if left < right and top < bottom else None

    def walk(self, visit):
        """Visit the window's elements in tree order, within the node budget and deadline, until `visit`
        returns True. False if cut short."""
        stack = [(self.frame, [], self.div)]
        while stack:
            if self.nodes >= self.budget or time.monotonic() > self.end:
                return False
            node, path, div = stack.pop()
            self.nodes += 1
            try:
                if self.denied(node.get_name()):
                    continue
                div = self.own_div(node, div)
                if visit(node, path, div):
                    return True
                kids = [node.get_child_at_index(i) for i in range(node.get_child_count())]
            except Exception:  # noqa: BLE001 - elements vanish mid-walk; skip them
                continue
            stack.extend((k, path + [i], div) for i, k in reversed(list(enumerate(kids))) if k is not None)
        return True

    def row(self, node, path, div) -> dict:
        return {"path": path, "role": node.get_role_name(), "name": node.get_name() or "",
                "states": shown_states(node), "centre": self.centre(node, div)}

    def find(self) -> dict:
        query, role, limit = self.req["query"].lower(), self.req["role"].lower(), self.req["limit"]
        rows = []

        def visit(node, path, div):
            if (not role or role in node.get_role_name()) and query in (node.get_name() or "").lower():
                rows.append(self.row(node, path, div))
            return len(rows) >= limit
        return {"rows": rows, "complete": self.walk(visit), "nodes": self.nodes}

    def resolve(self, path, role, name):
        """The element at `path` if it is still the same one, else the same-looking element nearest to it."""
        node, div = self.frame, self.div
        try:
            for i in path:
                node = node.get_child_at_index(i)
                div = self.own_div(node, div)
            if node.get_role_name() == role and (node.get_name() or "") == name:
                return node, path, div
        except Exception:  # noqa: BLE001 - the tree changed under that path
            pass
        found = []

        def same(n, p, d):
            if n.get_role_name() == role and (n.get_name() or "") == name:
                found.append((n, p, d))
        self.walk(same)
        if not found:
            raise Refused(f'the {role} "{name}" is gone; call ui_find again')
        return max(found, key=lambda f: common_prefix(f[1], path))

    def act(self) -> dict:
        req = self.req
        node, path, div = self.resolve(req["path"], req["role"], req["name"])
        action = node.get_action_iface()
        names = [action.get_action_name(i) for i in range(action.get_n_actions())] if action else []
        if req["action"] == "focus":
            component = node.get_component_iface()
            if component is None or not component.grab_focus():
                raise Refused(f"the {req['role']} can't take focus")
            did = "focus"
        elif not names:
            return {"click": self.centre(node, div), "path": path}
        else:
            wanted = [n.lower() for n in names]
            if req["action"] and req["action"].lower() not in wanted:
                raise Refused(f"the {req['role']} has no action {req['action']!r}; it has {', '.join(names)}")
            did = names[wanted.index(req["action"].lower()) if req["action"] else 0]
            action.do_action(names.index(did))
        time.sleep(0.15)  # let the app update the element's state
        try:
            return {"did": did, "path": path, **self.row(node, path, div)}
        except Exception:  # noqa: BLE001
            return {"did": did, "path": path, "gone": True}


def extents(atspi, node):
    component = node.get_component_iface()
    if component is None:
        return None
    r = component.get_extents(atspi.CoordType.WINDOW)
    return r.x, r.y, r.width, r.height


def states(node) -> list[str]:
    return [s.value_nick for s in node.get_state_set().get_states()]


def shown_states(node) -> list[str]:
    have = states(node)
    return [s for s in STATES if s in have] + ([] if "enabled" in have or "sensitive" in have else ["disabled"])


def common_prefix(a, b) -> int:
    n = 0
    while n < min(len(a), len(b)) and a[n] == b[n]:
        n += 1
    return n


def main() -> dict:
    req = json.load(sys.stdin)
    try:
        import gi
        gi.require_version("Atspi", "2.0")
        from gi.repository import Atspi
    except (ImportError, ValueError) as e:
        return {"error": f"AT-SPI isn't available to {sys.executable} ({e}); it needs python-gobject and at-spi2-core"}
    if req["op"] == "version":
        v = Atspi.get_version()
        return {"version": f"{v.major}.{v.minor}.{v.micro}"}
    Atspi.set_timeout(1000, 3000)
    try:
        window = Window(Atspi, req)
        return window.find() if req["op"] == "find" else window.act()
    except Refused as e:
        return {"error": str(e)}


if __name__ == "__main__":
    warnings.simplefilter("ignore", DeprecationWarning)
    json.dump(main(), sys.stdout)
