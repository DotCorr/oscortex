#!/usr/bin/env python3
"""Boot the x86 ISO headless, screendump the shell + a launched app to PNG.
Visual interactivity proof for the interact-freeze fix (the harness only samples
present_callback; this captures the actual framebuffer). Env: ISO, SMP, RUNID, OUT."""
import subprocess, socket, json, time, sys, os, signal

ISO   = os.environ.get("ISO", "/Users/ghostportal/Desktop/Dotcorr/OSCortex/oscortex.iso")
SMP   = os.environ.get("SMP", "1")
RUNID = os.environ.get("RUNID", "shot")
SERIAL= f"/tmp/x86-shot{RUNID}-serial.log"
QMP   = f"/tmp/x86-shot{RUNID}-qmp.sock"
OUT   = os.environ.get("OUT", f"/tmp/oscortex-x86-{RUNID}")
for p in (SERIAL, QMP):
    try: os.remove(p)
    except FileNotFoundError: pass

qemu = subprocess.Popen([
    "qemu-system-x86_64", "-cdrom", ISO, "-m", "2048M", "-smp", SMP,
    "-cpu", "qemu64,+x2apic", "-machine", "q35",
    "-device", "qemu-xhci,id=xhci", "-device", "usb-tablet,bus=xhci.0", "-device", "usb-kbd,bus=xhci.0",
    "-no-reboot", "-display", "none",
    "-serial", f"file:{SERIAL}", "-qmp", f"unix:{QMP},server,nowait",
], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

def reads():
    try:
        with open(SERIAL, "rb") as f: return f.read()
    except FileNotFoundError: return b""
def present(): return reads().count(b"present_callback")

t0 = time.time(); s = None
while time.time() - t0 < 120:
    try: s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect(QMP); break
    except (FileNotFoundError, ConnectionRefusedError): time.sleep(0.5)
if s is None: print("[shot] QMP never came up"); qemu.kill(); sys.exit(3)
buf = b""
def rcv():
    global buf
    while b"\n" not in buf:
        chunk = s.recv(4096)
        if not chunk: raise TimeoutError("qmp-closed")
        buf += chunk
    l, buf = buf.split(b"\n", 1); return json.loads(l)
rcv(); s.sendall(b'{"execute":"qmp_capabilities"}\n'); rcv()
s.settimeout(75)
def cmd(c, a=None):
    m = {"execute": c}
    if a is not None: m["arguments"] = a
    s.sendall((json.dumps(m) + "\n").encode()); return rcv()
def move(x, y): cmd("input-send-event", {"events": [{"type":"abs","data":{"axis":"x","value":x}},{"type":"abs","data":{"axis":"y","value":y}}]})
def click(x, y):
    move(x, y); cmd("input-send-event", {"events": [{"type":"btn","data":{"down":True,"button":"left"}}]}); time.sleep(0.2)
    cmd("input-send-event", {"events": [{"type":"btn","data":{"down":False,"button":"left"}}]})
def shot(name):
    ppm = f"{name}.ppm"
    cmd("screendump", {"filename": ppm}); time.sleep(1.5)
    sz = os.path.getsize(ppm) if os.path.exists(ppm) else 0
    subprocess.run(["sips", "-s", "format", "png", ppm, "--out", f"{name}.png"],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    print(f"[shot] {name}.png  (ppm {sz}B)", flush=True)

print("[shot] waiting for shell (slow TCG)...", flush=True)
t0 = time.time()
while time.time() - t0 < 360 and present() <= 20: time.sleep(2)
print(f"[shot] shell present={present()} after {int(time.time()-t0)}s", flush=True)
shot(f"{OUT}-1-shell")

print("[shot] launching Files...", flush=True)
before = present(); click(12365, 13197)
t1 = time.time()
while time.time() - t1 < 60:
    time.sleep(2)
    if present() - before > 15: break
print(f"[shot] after Files click: present+={present()-before}", flush=True)
shot(f"{OUT}-2-files")

# a couple of in-app clicks, then capture again (interaction did not crash)
click(8000, 12000); time.sleep(1.0); click(16000, 12000); time.sleep(1.0)
print(f"[shot] post-interact present={present()} crash={'KERNEL PANIC' in reads().decode('latin1')}", flush=True)
shot(f"{OUT}-3-files-interact")

s.close(); qemu.send_signal(signal.SIGTERM)
try: qemu.wait(timeout=15)
except subprocess.TimeoutExpired: qemu.kill()
print("[shot] done", flush=True)
