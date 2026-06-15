#!/usr/bin/env python3
"""aarch64 post-app-launch freeze regression test.

Boots the OSCortex aarch64 .kernel headless under HVF, waits for the shell to
render, launches an app via QMP pointer events, sweeps the grid, and asserts that
`present_callback` keeps advancing (the freeze symptom is present_callback
sticking ~38 frames after launch).

Env overrides:
  OSCORTEX_KERNEL   path to the aarch64 .kernel  (default ~/OSCortex-run/oscortex-aarch64.kernel)
Exit 0 = advancing (no freeze); non-zero = frozen / failed to render.
"""
import json, os, socket, subprocess, sys, time, signal

KERNEL = os.environ.get("OSCORTEX_KERNEL", os.path.expanduser("~/OSCortex-run/oscortex-aarch64.kernel"))
SERIAL = "/tmp/oscortex-freeze-serial.log"
QMP = "/tmp/oscortex-freeze-qmp.sock"
for p in (SERIAL, QMP):
    try: os.remove(p)
    except OSError: pass

if not os.path.exists(KERNEL):
    print(f"[repro] kernel not found: {KERNEL} (set OSCORTEX_KERNEL)"); sys.exit(3)

qemu = subprocess.Popen([
    "qemu-system-aarch64", "-M", "virt,accel=hvf", "-cpu", "host", "-smp", "1", "-m", "4G",
    "-kernel", KERNEL, "-device", "ramfb",
    "-device", "virtio-tablet-device", "-device", "virtio-keyboard-device",
    "-display", "none", "-serial", f"file:{SERIAL}", "-qmp", f"unix:{QMP},server,nowait",
], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

def present():
    try:
        with open(SERIAL, "rb") as f: return f.read().count(b"present_callback")
    except FileNotFoundError: return 0

def qmp_connect(timeout=40):
    t0 = time.time()
    while time.time() - t0 < timeout:
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect(QMP); return s
        except (FileNotFoundError, ConnectionRefusedError): time.sleep(0.3)
    raise RuntimeError("QMP never came up")

qmp = qmp_connect(); buf = b""
def rcv():
    global buf
    while b"\n" not in buf: buf += qmp.recv(4096)
    line, buf = buf.split(b"\n", 1); return json.loads(line)
rcv(); qmp.sendall(b'{"execute":"qmp_capabilities"}\n'); rcv()
def ev(e): qmp.sendall((json.dumps({"execute":"input-send-event","arguments":{"events":e}})+"\n").encode()); rcv()
def move(x,y): ev([{"type":"abs","data":{"axis":"x","value":x}},{"type":"abs","data":{"axis":"y","value":y}}])
def click(x,y):
    move(x,y); ev([{"type":"btn","data":{"down":True,"button":"left"}}]); time.sleep(0.05); ev([{"type":"btn","data":{"down":False,"button":"left"}}])

print("[repro] waiting for shell...", flush=True)
t0 = time.time()
while time.time() - t0 < 90 and present() <= 20: time.sleep(1)
pc0 = present()
print(f"[repro] shell present={pc0} after {int(time.time()-t0)}s", flush=True)
if pc0 <= 20:
    qemu.send_signal(signal.SIGTERM); print("[repro] shell never rendered"); sys.exit(3)

click(12380, 11880); time.sleep(12)
launch = present()
print(f"[repro] post-launch present={launch}", flush=True)
samples = []
sweep = [(8000,8000),(14000,8000),(20000,8000),(20000,14000),(14000,14000),(8000,14000)]
for w in range(6):
    for (x,y) in sweep: move(x,y); time.sleep(0.15)
    time.sleep(2); samples.append(present()); print(f"[repro]   win{w} present={samples[-1]}", flush=True)

advanced = samples[-1] - launch
stuck = sum(1 for i in range(1,len(samples)) if samples[i]==samples[i-1])
ok = advanced > 5 and stuck <= 1
print(f"[repro] launch={launch} final={samples[-1]} advanced={advanced} stuck={stuck}/{len(samples)-1}")
print("[repro] RESULT:", "ADVANCING — no freeze" if ok else "FROZEN")
qmp.close(); qemu.send_signal(signal.SIGTERM)
try: qemu.wait(timeout=10)
except subprocess.TimeoutExpired: qemu.kill()
sys.exit(0 if ok else 1)
