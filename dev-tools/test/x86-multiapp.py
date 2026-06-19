#!/usr/bin/env python3
"""Headless x86_64 multi-app responsiveness / freeze harness (TCG — SLOW).

Sibling of arm-multiapp.py for x86. Boots oscortex.iso under -smp N (no firmware,
legacy BIOS / Limine) with usb-tablet+usb-kbd input, launches a sequence of
launcher apps via QMP clicks, then injects pointer moves for DUR seconds and
samples present_callback + embedder/ptr to decide RESPONSIVE vs FROZEN.

A wedged VM stops servicing QMP, so a QMP call that blocks past the socket
timeout is itself treated as a FREEZE (this is what the first run missed). Used
to verify the 2026-06-17 x86 cooperative-safe scheduler fix. See [[scheduler-rework]].

Env:
  ISO    iso path (default <repo>/oscortex.iso)
  SMP    core count (default 1)
  APPS   comma list from {canvas,files,weblink} (default files)
  DUR    responsiveness sampling seconds (default 90; TCG is slow)
  RUNID  suffix for serial/qmp/disk paths so runs don't collide (default "")
"""
import json, os, socket, subprocess, sys, time, signal

REPO = subprocess.run(["git","rev-parse","--show-toplevel"], capture_output=True, text=True).stdout.strip() or "."
ISO   = os.environ.get("ISO", f"{REPO}/oscortex.iso")
SMP   = os.environ.get("SMP", "1")
DUR   = int(os.environ.get("DUR", "90"))
APPS  = [a.strip() for a in os.environ.get("APPS", "files").split(",") if a.strip()]
RUNID = os.environ.get("RUNID", "")
SERIAL = f"/tmp/x86-multiapp{RUNID}-serial.log"
QMP    = f"/tmp/x86-multiapp{RUNID}-qmp.sock"
VBLK   = f"/tmp/x86-multiapp{RUNID}-vblk.img"
NVME   = f"/tmp/x86-multiapp{RUNID}-nvme.img"
CARDS  = {"canvas": (4375, 13197), "files": (12365, 13197), "weblink": (20355, 13197)}

for p in (SERIAL, QMP):
    try: os.remove(p)
    except OSError: pass
for img, mb in ((VBLK, 8), (NVME, 16)):
    if not os.path.exists(img):
        subprocess.run(["dd","if=/dev/zero",f"of={img}","bs=1M",f"count={mb}"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
if not os.path.exists(ISO):
    print(f"[x86mt] ISO not found: {ISO}"); sys.exit(2)

qemu = subprocess.Popen([
    "qemu-system-x86_64", "-cdrom", ISO, "-m", "2048M", "-smp", SMP,
    "-cpu", "qemu64,+x2apic", "-machine", "q35",
    "-device", "virtio-net-pci,netdev=net0", "-netdev", "user,id=net0",
    "-device", "virtio-blk-pci,drive=vblk", "-drive", f"file={VBLK},format=raw,id=vblk,if=none",
    "-device", "nvme,drive=nvmedrive,serial=oscortex0", "-drive", f"file={NVME},format=raw,id=nvmedrive,if=none",
    "-device", "qemu-xhci,id=xhci", "-device", "usb-tablet,bus=xhci.0", "-device", "usb-kbd,bus=xhci.0",
    "-no-reboot", "-display", "none",
    "-serial", f"file:{SERIAL}", "-qmp", f"unix:{QMP},server,nowait",
], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

def reads():
    try:
        with open(SERIAL, "rb") as f: return f.read()
    except FileNotFoundError: return b""
def present(): return reads().count(b"present_callback")
def ptrs():    return reads().count(b"embedder/ptr")
def crashed():
    d = reads()
    return any(m in d for m in (b"KERNEL PANIC", b"Unhandled Exception", b"FATAL", b"triple fault"))
def tail(n=60):
    return subprocess.run(["tail","-"+str(n),SERIAL], capture_output=True, text=True, errors="replace").stdout

# connect QMP (TCG slow to come up)
t0 = time.time(); s = None
while time.time() - t0 < 120:
    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect(QMP); break
    except (FileNotFoundError, ConnectionRefusedError): time.sleep(0.5)
if s is None:
    print("[x86mt] QEMU/QMP never came up"); qemu.kill(); sys.exit(3)
buf = b""
def rcv():
    global buf
    while b"\n" not in buf:
        try: chunk = s.recv(4096)
        except (socket.timeout, OSError): raise TimeoutError("qmp-stall")
        if not chunk: raise TimeoutError("qmp-closed")
        buf += chunk
    l, buf = buf.split(b"\n", 1); return json.loads(l)
rcv(); s.sendall(b'{"execute":"qmp_capabilities"}\n'); rcv()
s.settimeout(75)   # a wedged VM stops servicing QMP within 75s → treat as freeze
def cmd(c, a=None):
    m = {"execute": c}
    if a is not None: m["arguments"] = a
    s.sendall((json.dumps(m) + "\n").encode()); return rcv()
def ev(e): cmd("input-send-event", {"events": e})
def move(x, y): ev([{"type":"abs","data":{"axis":"x","value":x}},{"type":"abs","data":{"axis":"y","value":y}}])
def click(x, y):
    move(x, y); ev([{"type":"btn","data":{"down":True,"button":"left"}}]); time.sleep(0.15)
    ev([{"type":"btn","data":{"down":False,"button":"left"}}])

def die(code):
    try: s.close()
    except Exception: pass
    qemu.send_signal(signal.SIGTERM)
    try: qemu.wait(timeout=15)
    except subprocess.TimeoutExpired: qemu.kill()
    sys.exit(code)

# 1. wait for shell (TCG: up to 6 min)
print("[x86mt] waiting for shell (slow TCG)...", flush=True)
t0 = time.time()
while time.time() - t0 < 360 and present() <= 20:
    if crashed(): break
    time.sleep(3)
pc0 = present()
if pc0 <= 20:
    print(f"[x86mt] shell did NOT render (present={pc0}); tail:\n{tail(25)}"); die(4)
print(f"[x86mt] smp={SMP} shell rendered (present={pc0}) after {int(time.time()-t0)}s", flush=True)

# 2. launch each requested app (TCG: be patient). A QMP stall during launch = wedge.
for app in APPS:
    if app not in CARDS:
        print(f"[x86mt] unknown app '{app}'"); continue
    before = present()
    try:
        click(*CARDS[app])
    except TimeoutError:
        print(f"[x86mt] launch {app}: QMP STALLED — VM wedged on launch");
        print(tail(60)); die(1)
    t1 = time.time(); rendered = False
    while time.time() - t1 < 40:
        time.sleep(2)
        if crashed(): break
        if present() - before > 15: rendered = True; break
    print(f"[x86mt] launch {app}: rendered={rendered} present+={present()-before} crash={crashed()}", flush=True)
    if crashed(): print(tail(60)); die(1)
    time.sleep(2)

# 3. responsiveness sampling: inject moves; present must keep advancing AND QMP must respond
INTERACT = os.environ.get("INTERACT", "") == "1"
INTERACT_N = int(os.environ.get("INTERACT_N", "0"))  # if >0: click only first N rounds, then STOP (recovery test)
INAPP = [(2200,2200),(8000,12000),(16000,12000),(24000,12000),(8000,20000),(16000,20000),(30000,2200)]
print(f"[x86mt] sampling responsiveness for {DUR}s (interact={INTERACT} n={INTERACT_N}) ...", flush=True)
stall_since = None; frozen = False; reason = ""; ri = 0
last = present(); samples = [(0, last, ptrs())]
t2 = time.time()
while time.time() - t2 < DUR:
    try:
        if INTERACT and (INTERACT_N == 0 or ri < INTERACT_N): click(*INAPP[ri % len(INAPP)])
        move(8000 + (int(time.time() - t2) % 6) * 1500, 9000)
    except TimeoutError:
        frozen = True; reason = "QMP stalled >75s (VM wedged)"; break
    ri += 1
    time.sleep(3)
    cur, pt, el = present(), ptrs(), int(time.time() - t2)
    samples.append((el, cur, pt))
    if crashed(): break
    if cur > last:
        last = cur; stall_since = None
    else:
        stall_since = stall_since or time.time()
        if time.time() - stall_since > 40:   # 40s with no new frame (TCG-slow) = frozen
            frozen = True; reason = "present flatlined >40s"; break

cr = crashed()
result = "CRASH" if cr else ("FROZEN" if frozen else "RESPONSIVE")
print(f"[x86mt] samples (s,present,ptr): {samples}", flush=True)
print(f"[x86mt] RESULT smp={SMP} apps={APPS}: {result}  {('('+reason+')') if reason else ''}", flush=True)
if result != "RESPONSIVE":
    print("[x86mt] --- state at wedge (serial tail) ---"); print(tail(60))
die(0 if result == "RESPONSIVE" else 1)
