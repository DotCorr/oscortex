#!/usr/bin/env python3
"""Headless aarch64 multi-app responsiveness / freeze harness (HVF).

Boots a .kernel under -smp N, launches a sequence of launcher apps via QMP
virtio-tablet clicks, then for DUR seconds injects pointer moves and samples
present_callback + embedder/ptr to decide whether the OS stays RESPONSIVE or
FREEZES. Reports per-app render, final responsiveness, crash detection, and
dumps scheduler state (debug_runnable_states / preempt logs / fault context)
on a wedge. Built for the scheduler rework — see memory [[scheduler-rework]].

Env:
  KERN  kernel path (default ~/OSCortex-run/oscortex-aarch64-fpfix.kernel)
  SMP   core count (default 1)
  APPS  comma list from {canvas,files,weblink} (default files)
  DUR   responsiveness sampling seconds (default 45)
"""
import json, os, socket, subprocess, sys, time, signal

KERN = os.environ.get("KERN", os.path.expanduser("~/OSCortex-run/oscortex-aarch64-fpfix.kernel"))
SMP  = os.environ.get("SMP", "1")
DUR  = int(os.environ.get("DUR", "45"))
APPS = [a.strip() for a in os.environ.get("APPS", "files").split(",") if a.strip()]
SERIAL = "/tmp/arm-multiapp-serial.log"
QMP    = "/tmp/arm-multiapp-qmp.sock"
# launcher-grid card centers, normalized abs coords (0..32767)
CARDS  = {"canvas": (4375, 13197), "files": (12365, 13197), "weblink": (20355, 13197)}

for p in (SERIAL, QMP):
    try: os.remove(p)
    except OSError: pass
if not os.path.exists(KERN):
    print(f"[multiapp] kernel not found: {KERN}"); sys.exit(2)

qemu = subprocess.Popen([
    "qemu-system-aarch64", "-M", "virt,accel=hvf", "-cpu", "host", "-smp", SMP, "-m", "4G",
    "-kernel", KERN, "-device", "ramfb", "-device", "virtio-tablet-device",
    "-device", "virtio-keyboard-device", "-display", "none",
    "-serial", f"file:{SERIAL}", "-qmp", f"unix:{QMP},server,nowait", "-no-reboot",
], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

def reads():
    try:
        with open(SERIAL, "rb") as f: return f.read()
    except FileNotFoundError: return b""
def present(): return reads().count(b"present_callback")
def ptrs():    return reads().count(b"embedder/ptr")
def crashed():
    d = reads()
    return any(m in d for m in (b"EC=0x24", b"UNHANDLED", b"PANIC", b"KERNEL PANIC"))
def tail(n=50):
    return subprocess.run(["tail", "-" + str(n), SERIAL],
                          capture_output=True, text=True, errors="replace").stdout

# connect QMP
t0 = time.time(); s = None
while time.time() - t0 < 60:
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect(QMP); break
    except (FileNotFoundError, ConnectionRefusedError): time.sleep(0.3)
if s is None:
    print("[multiapp] QEMU/QMP never came up"); qemu.kill(); sys.exit(3)
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
def move(x, y): ev([{"type":"abs","data":{"axis":"x","value":x}},{"type":"abs","data":{"axis":"y","value":y}}])
def click(x, y):
    move(x, y); ev([{"type":"btn","data":{"down":True,"button":"left"}}]); time.sleep(0.12)
    ev([{"type":"btn","data":{"down":False,"button":"left"}}])
def shot(name):
    ppm = f"{name}.ppm"
    try: os.remove(ppm)
    except OSError: pass
    cmd("screendump", {"filename": ppm}); time.sleep(1.0)
    if os.path.exists(ppm):
        subprocess.run(["sips","-s","format","png",ppm,"--out",f"{name}.png"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

# 1. wait for shell
t0 = time.time()
while time.time() - t0 < 120 and present() <= 20: time.sleep(1)
pc0 = present()
if pc0 <= 20:
    print(f"[multiapp] shell did NOT render (present={pc0}); tail:\n{tail(25)}")
    qemu.send_signal(signal.SIGTERM); sys.exit(4)
print(f"[multiapp] smp={SMP} shell rendered (present={pc0})")
shot("/tmp/arm-multiapp-shell")

# 2. launch each requested app
for app in APPS:
    if app not in CARDS:
        print(f"[multiapp] unknown app '{app}'"); continue
    before = present()
    click(*CARDS[app])
    t1 = time.time(); rendered = False
    while time.time() - t1 < 12:
        time.sleep(1)
        if crashed(): break
        if present() - before > 15: rendered = True; break
    print(f"[multiapp] launch {app}: rendered={rendered} present+={present()-before} crash={crashed()}")
    if crashed(): break
    time.sleep(1)

# 3. responsiveness sampling: inject moves (+ in-app CLICKS if INTERACT=1 — this is
#    what a user does inside a launched app; the freeze may need real interaction)
INTERACT = os.environ.get("INTERACT", "") == "1"
INAPP = [(2200,2200),(8000,12000),(16000,12000),(24000,12000),(8000,20000),(16000,20000),(30000,2200)]
print(f"[multiapp] sampling responsiveness for {DUR}s (interact={INTERACT}) ...")
stall_since = None; frozen = False; ri = 0
last = present(); samples = [(0, last, ptrs())]
t2 = time.time()
while time.time() - t2 < DUR:
    if INTERACT:
        click(*INAPP[ri % len(INAPP)])
        if ri % 5 == 0: shot(f"/tmp/arm-multiapp-interact-{ri}")
    move(8000 + (int(time.time() - t2) % 6) * 1500, 9000)
    ri += 1
    time.sleep(2)
    cur, pt, el = present(), ptrs(), int(time.time() - t2)
    samples.append((el, cur, pt))
    if crashed(): break
    if cur > last:
        last = cur; stall_since = None
    else:
        stall_since = stall_since or time.time()
        if time.time() - stall_since > 10:   # 10s with no new frame = frozen
            frozen = True; break

cr = crashed()
result = "CRASH" if cr else ("FROZEN" if frozen else "RESPONSIVE")
print(f"[multiapp] samples (s,present,ptr): {samples}")
print(f"[multiapp] RESULT smp={SMP} apps={APPS}: {result}")
if result != "RESPONSIVE":
    print("[multiapp] --- scheduler/fault state at wedge (serial tail) ---")
    print(tail(60))
shot("/tmp/arm-multiapp-final")
s.close(); qemu.send_signal(signal.SIGTERM)
try: qemu.wait(timeout=10)
except subprocess.TimeoutExpired: qemu.kill()
sys.exit(0 if result == "RESPONSIVE" else 1)
