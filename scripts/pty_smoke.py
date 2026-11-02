#!/usr/bin/env python3
"""Terminal smoke test for the real systui TUI (Linux/macOS; needs python3, no packages).

Runs the release binary inside a pseudo-terminal, sends real keystrokes and checks that:
  * the alternate screen is entered and left, the cursor is shown again and the tty modes
    (canonical/echo) are restored after `q`, after Ctrl+C and after a storm of keys;
  * every view, overlay and the palette render; rapid resizes (down to 1x1) do not crash;
  * `--no-color` and NO_COLOR emit no SGR color parameters (only bold/dim/reverse).

Usage:  cargo build --release && python3 scripts/pty_smoke.py      (SYSTUI_BIN overrides the path)
Exit code 0 = all checks passed.
"""
import fcntl, os, pty, select, signal, struct, sys, termios, time, json, subprocess

BIN = os.environ.get("SYSTUI_BIN") or os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target", "release", "systui")

ROWS, COLS = 40, 120

def run(keys, label, extra_env=None, args=(), settle=2.5):
    """Spawn systui in a PTY, feed `keys` (list of (delay, bytes)), return (exit_code, output, termios_before, termios_after)."""
    env = dict(os.environ, TERM="xterm-256color", LANG="C.UTF-8")
    env.pop("NO_COLOR", None)
    env.update(extra_env or {})
    pid, fd = pty.fork()
    if pid == 0:
        os.execve(BIN, [BIN, *args], env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    before = termios.tcgetattr(fd)
    out = b""
    def pump(t):
        nonlocal out
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([fd], [], [], 0.05)
            if r:
                try:
                    d = os.read(fd, 65536)
                except OSError:
                    return False
                if not d:
                    return False
                out += d
        return True
    alive = pump(settle)
    snapshots = {}
    for delay, k in keys:
        if not alive:
            break
        os.write(fd, k)
        alive = pump(delay)
        snapshots[k] = len(out)
    # give it time to exit
    deadline = time.time() + 5
    code = None
    while time.time() < deadline:
        try:
            p, status = os.waitpid(pid, os.WNOHANG)
        except ChildProcessError:
            break
        if p:
            code = os.waitstatus_to_exitcode(status)
            break
        pump(0.1)
    if code is None:
        os.kill(pid, signal.SIGKILL); os.waitpid(pid, 0); code = "KILLED(hung)"
    pump(0.2)
    try:
        after = termios.tcgetattr(fd)
    except termios.error:
        after = None
    return code, out, before, after

def flags(t):
    if not t: return None
    return {"lflag_ICANON": bool(t[3] & termios.ICANON), "lflag_ECHO": bool(t[3] & termios.ECHO), "iflag_ICRNL": bool(t[0] & termios.ICRNL), "oflag_OPOST": bool(t[1] & termios.OPOST)}

import re
def color_params(out):
    """SGR parameters that set a color (30-37, 38, 40-47, 48, 90-97, 100-107). Resets 39/49 are fine."""
    found = set()
    for m in re.finditer(rb"\x1b\[([0-9;]*)m", out):

        for p in m.group(1).split(b";"):
            if p.isdigit():
                n = int(p)
                if 30 <= n <= 38 or 40 <= n <= 48 or 90 <= n <= 97 or 100 <= n <= 107:
                    found.add(n)
    return sorted(found)

def check(name, cond, detail=""):
    print(("PASS " if cond else "FAIL ") + name + (f"  [{detail}]" if detail and not cond else ""))
    return cond

ok = True
ALT_ON, ALT_OFF, CUR_SHOW = b"\x1b[?1049h", b"\x1b[?1049l", b"\x1b[?25h"

# ---- 1. basic navigation + quit with q
code, out, b, a = run([(0.8, b"2"), (0.5, b"j"), (0.5, b"j"), (0.5, b"\r"), (0.5, b"\x1b"), (0.8, b"?"), (0.5, b"\x1b"),
                       (0.8, b"3"), (0.8, b"4"), (0.8, b"5"), (1.0, b"6"), (0.8, b"1"), (0.6, b"q")], "nav")
txt = out.decode("utf-8", "replace")
ok &= check("exit code 0 after q", code == 0, str(code))
ok &= check("entered alternate screen", ALT_ON in out)
ok &= check("left alternate screen", ALT_OFF in out)
ok &= check("cursor shown again", CUR_SHOW in out)
ok &= check("alt screen left AFTER being entered", out.rfind(ALT_OFF) > out.find(ALT_ON))
ok &= check("terminal modes restored (canonical + echo back on)", flags(b) == flags(a), f"{flags(b)} vs {flags(a)}")
for word in ["Overview", "Processes", "Network", "Storage", "Services", "CPU", "RAM"]:
    ok &= check(f"screen text contains {word!r}", word in txt)
ok &= check("live data rendered (GiB)", "GiB" in txt)
ok &= check("help overlay rendered", "Help" in txt)
ok &= check("process details pane rendered", "Details" in txt)
ok &= check("no raw escape garbage like '^[' text", "^[" not in txt)
print("bytes of terminal output:", len(out))

# ---- 2. Ctrl+C exits cleanly
code, out, b, a = run([(0.8, b"\x03")], "ctrlc")
ok &= check("Ctrl+C exits 0", code == 0, str(code))
ok &= check("Ctrl+C restores terminal", ALT_OFF in out and CUR_SHOW in out and flags(b) == flags(a))

# ---- 3. palette, filter, sort, tree, changes, recording disabled message, help
code, out, b, a = run([
    (0.6, b"2"), (0.5, b"/"), (0.3, b"s"), (0.3, b"y"), (0.3, b"s"), (0.3, b"\r"), (0.5, b"\x1b"),
    (0.5, b"s"), (0.5, b"S"), (0.5, b"t"), (0.6, b"\x1b[D"), (0.6, b"\x1b[C"), (0.6, b"t"),
    (0.6, b"w"), (0.6, b"\x1b"), (0.5, b"\x0b"), (0.3, b"q"), (0.3, b"u"), (0.3, b"i"), (0.3, b"t"), (0.5, b"\r"),
], "keys")
txt = out.decode("utf-8", "replace")
ok &= check("palette/filter/tree/changes keys: exited 0 (via palette 'quit')", code == 0, str(code))
ok &= check("What changed overlay appeared", "What changed" in txt)
ok &= check("Commands palette appeared", "Commands" in txt)
ok &= check("tree mode announced", "tree" in txt)
ok &= check("terminal restored after key storm", ALT_OFF in out and flags(b) == flags(a))

# ---- 4. resize handling via SIGWINCH to a tiny terminal and back
def resize_test():
    env = dict(os.environ, TERM="xterm-256color", LANG="C.UTF-8")
    pid, fd = pty.fork()
    if pid == 0:
        os.execve(BIN, [BIN], env)
    out = b""
    def pump(t):
        nonlocal out
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([fd], [], [], 0.05)
            if r:
                try: out += os.read(fd, 65536)
                except OSError: return
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0)); pump(1.5)
    for rows, cols in [(10, 40), (3, 20), (1, 1), (60, 200), (24, 80), (18, 60), (40, 120)]:
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        os.kill(pid, signal.SIGWINCH)
        pump(0.5)
    os.write(fd, b"q"); pump(1.0)
    try:
        p, st = os.waitpid(pid, os.WNOHANG)
        if not p:
            time.sleep(1); p, st = os.waitpid(pid, os.WNOHANG)
        code = os.waitstatus_to_exitcode(st) if p else "HUNG"
    except ChildProcessError:
        code = "gone"
    if code == "HUNG":
        os.kill(pid, signal.SIGKILL); os.waitpid(pid, 0)
    return code, out
code, out = resize_test()
ok &= check("survives rapid resizes incl. 1x1 and exits 0", code == 0, str(code))
ok &= check("'Terminal too small' message shown at tiny sizes", b"too small" in out)

# ---- 5. non-TTY refusal & --no-color
r = subprocess.run([BIN], stdin=subprocess.DEVNULL, capture_output=True, text=True)
ok &= check("non-tty run refuses with exit 1", r.returncode == 1 and "terminal" in r.stderr)
code, out, b, a = run([(0.6, b"2"), (0.5, b"q")], "nocolor", args=("--no-color",))
ok &= check("--no-color emits no SGR color parameters", not color_params(out), repr(color_params(out)))
code, out, b, a = run([(0.6, b"q")], "nocolor-env", extra_env={"NO_COLOR": "1"})
ok &= check("NO_COLOR env emits no SGR color parameters", not color_params(out), repr(color_params(out)))
base = color_params(run([(0.6, b"q")], "color-baseline")[1])
ok &= check("sanity: the default theme DOES use color (so the check can fail)", len(base) > 0)

print("\nALL PASS" if ok else "\nSOME CHECKS FAILED")
sys.exit(0 if ok else 1)
