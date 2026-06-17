#!/usr/bin/env python3
# Headless aarch64 SMP app-launch test under HVF (REAL parallel cores).
#
# Boots the given .kernel under `-smp 2`, waits for the shell, launches an app via
# QMP virtio-tablet click, then samples present_callback to decide RENDERED vs
# FROZEN, and scans for the EC=0x24 / UNHANDLED crash. This is the harness that
# proved the 2026-06-17 aarch64 SMP app-launch fix (see memory [[smp-bringup]] M6).
#
# Usage:  arm-smp-applaunch.py [/path/to/oscortex-aarch64.kernel]
#   default kernel: ~/OSCortex-run/oscortex-aarch64-fpfix.kernel
import json, os, socket, subprocess, sys, time, signal

KERN = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser(
    "~/OSCortex-run/oscortex-aarch64-fpfix.kernel")
SERIAL = "/tmp/arm-applaunch-serial.log"
QMP = "/tmp/arm-applaunch-qmp.sock"
SHOT = "/tmp/arm-applaunch-shot"
for p in (SERIAL, QMP):
    try: os.remove(p)
    except OSError: pass
if not os.path.exists(KERN):
    print(f"[applaunch] kernel not found: {KERN}"); sys.exit(2)

qemu = subprocess.Popen([
    "qemu-system-aarch64", "-M", "virt,accel=hvf", "-cpu", "host", "-smp", "2", "-m", "4G",
    "-kernel", KERN, "-device", "ramfb", "-device", "virtio-tablet-device",
    "-device", "virtio-keyboard-device", "-display", "none",
    "-serial", f"file:{SERIAL}", "-qmp", f"unix:{QMP},server,nowait", "-no-reboot",
], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

def reads():
    try:
        with open(SERIAL, "rb") as f: return f.read()
    except FileNotFoundError: return b""
def present(): return reads().count(b"present_callback")
def crashed(): d = reads(); return b"EC=0x24" in d or b"UNHANDLED" in d or b"PANIC" in d

# connect QMP
t0 = time.time(); s = None
while time.time() - t0 < 60:
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect(QMP); break
    except (FileNotFoundError, ConnectionRefusedError): time.sleep(0.3)
if s is None:
    print("[applaunch] QEMU/QMP never came up (host overloaded? bad kernel?)"); qemu.kill(); sys.exit(3)
buf = b""
def rcv():
    global buf
    while b"\n" not in buf: buf += s.recv(4096)
    l, buf = buf.split(b"\n", 1); return json.loads(l)
rcv(); s.sendall(b'{"execute":"qmp_capabilities"}\n'); rcv()
def cmd(c, a=None):
    m = {"execute": c}
    if a is not None: m["arguments"] = a
    s.sendall((json.dumps(m) + "\n").encode()); return rcv()
def ev(e): cmd("input-send-event", {"events": e})
def move(x, y): ev([{"type": "abs", "data": {"axis": "x", "value": x}}, {"type": "abs", "data": {"axis": "y", "value": y}}])
def click(x, y):
    move(x, y); ev([{"type": "btn", "data": {"down": True, "button": "left"}}]); time.sleep(0.12)
    ev([{"type": "btn", "data": {"down": False, "button": "left"}}])
def shot(n):
    ppm = f"{n}.ppm"
    try: os.remove(ppm)
    except OSError: pass
    cmd("screendump", {"filename": ppm}); time.sleep(1.0)
    if os.path.exists(ppm):
        subprocess.run(["sips", "-s", "format", "png", ppm, "--out", f"{n}.png"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

# 1. wait for shell (HVF is fast)
t0 = time.time()
while time.time() - t0 < 90 and present() <= 20: time.sleep(1)
pc0 = present()
if pc0 <= 20:
    print(f"[applaunch] shell did NOT render (present={pc0}) — tail:")
    print(subprocess.run(["tail", "-20", SERIAL], capture_output=True, text=True).stdout)
    qemu.send_signal(signal.SIGTERM); sys.exit(4)
shot(f"{SHOT}-shell")

# 2. launch Web Link (normalized abs coords for the launcher grid), fall back to Files
click(20355, 13197); time.sleep(8)
if present() - pc0 < 10:
    click(12365, 13197); time.sleep(8)
pc1 = present()

# 3. sample for freeze
samples = []
for w in range(5):
    move(8000 + w * 1500, 9000); time.sleep(3); samples.append(present())
shot(f"{SHOT}-final")
adv = samples[-1] - pc1
cr = crashed()
result = "CRASH" if cr else ("APP RENDERED (no freeze)" if adv > 20 else "FROZEN")
print(f"[applaunch] shell={pc0} post-launch={pc1} final={samples[-1]} advanced={adv} crash={cr}")
print(f"[applaunch] RESULT: {result}")
s.close(); qemu.send_signal(signal.SIGTERM)
try: qemu.wait(timeout=10)
except subprocess.TimeoutExpired: qemu.kill()
sys.exit(0 if result.startswith("APP RENDERED") else 1)
