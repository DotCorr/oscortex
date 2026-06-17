#!/usr/bin/env python3
# Headless freeze repro for OSCortex x86_64 (Limine ISO, pure TCG — SLOW).
# Boots the ISO, waits (patiently) for the shell, launches an app via QMP, sweeps,
# and samples present_callback to decide FROZEN vs ADVANCING.
import json, os, socket, subprocess, sys, time, signal

ROOT = (os.environ.get("OSCORTEX_REPO") or __import__("subprocess").run(["git","rev-parse","--show-toplevel"],capture_output=True,text=True).stdout.strip())
ISO = f"{ROOT}/oscortex.iso"
VBLK = f"{ROOT}/vdisk.img"
NVME = f"{ROOT}/nvme.img"
SERIAL = "/tmp/oscortex-x86-serial.log"
QMP = "/tmp/oscortex-x86-qmp.sock"
for p in (SERIAL, QMP):
    try: os.remove(p)
    except OSError: pass
for img, mb in ((VBLK, 8), (NVME, 16)):
    if not os.path.exists(img):
        subprocess.run(["dd", "if=/dev/zero", f"of={img}", "bs=1M", f"count={mb}"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

qemu = subprocess.Popen([
    "qemu-system-x86_64", "-cdrom", ISO, "-m", "2048M", "-smp", "2",
    "-cpu", "qemu64,+x2apic", "-machine", "q35",
    "-device", "virtio-net-pci,netdev=net0", "-netdev", "user,id=net0",
    "-device", "virtio-blk-pci,drive=vblk",
    "-drive", f"file={VBLK},format=raw,id=vblk,if=none",
    "-device", "nvme,drive=nvmedrive,serial=oscortex0",
    "-drive", f"file={NVME},format=raw,id=nvmedrive,if=none",
    "-device", "qemu-xhci,id=xhci",
    "-serial", f"file:{SERIAL}", "-no-reboot", "-display", "none",
    "-qmp", f"unix:{QMP},server,nowait",
], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

def reads():
    try:
        with open(SERIAL, "rb") as f: return f.read()
    except FileNotFoundError: return b""
def present(): return reads().count(b"present_callback")
def has(t): return t.encode() in reads()

def qmp_connect(timeout=300):
    t0 = time.time()
    while time.time() - t0 < timeout:
        try:
            s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect(QMP); return s
        except (FileNotFoundError, ConnectionRefusedError): time.sleep(0.5)
    raise RuntimeError("QMP never came up")
qmp = qmp_connect(); buf = b""
def rcv():
    global buf
    while b"\n" not in buf: buf += qmp.recv(4096)
    l, buf = buf.split(b"\n", 1); return json.loads(l)
rcv(); qmp.sendall(b'{"execute":"qmp_capabilities"}\n'); rcv()
def ev(e): qmp.sendall((json.dumps({"execute":"input-send-event","arguments":{"events":e}})+"\n").encode()); rcv()
def move(x,y): ev([{"type":"abs","data":{"axis":"x","value":x}},{"type":"abs","data":{"axis":"y","value":y}}])
def click(x,y):
    move(x,y); ev([{"type":"btn","data":{"down":True,"button":"left"}}]); time.sleep(0.1); ev([{"type":"btn","data":{"down":False,"button":"left"}}])

# 1. wait for shell (TCG is slow — be patient: up to 6 min)
print("[x86] waiting for shell first frames (slow TCG)...", flush=True)
t0 = time.time()
while time.time() - t0 < 360:
    if has("FATAL") and has("disallowed Dart VM flag"):
        print("[x86] *** FATAL: engine rejected dart-flags (UNPATCHED x64 engine) ***", flush=True)
        qemu.send_signal(signal.SIGTERM); sys.exit(2)
    if present() > 20: break
    time.sleep(3)
pc0 = present()
print(f"[x86] shell present_callback={pc0} after {int(time.time()-t0)}s", flush=True)
if pc0 <= 20:
    print("[x86] shell never rendered — abort. serial tail:", flush=True)
    print(subprocess.run(["tail","-30",SERIAL],capture_output=True,text=True).stdout)
    qemu.send_signal(signal.SIGTERM); sys.exit(3)

# 2. launch an app (try a couple of grid positions; x86 res may differ)
print("[x86] launching app (grid click)...", flush=True)
for (lx,ly) in [(12380,11880),(8000,9000),(16000,9000)]:
    click(lx,ly); time.sleep(8)
    if present() - pc0 > 15: break
time.sleep(15)
pc_launch = present()
print(f"[x86] post-launch present_callback={pc_launch} (delta {pc_launch-pc0})", flush=True)

# 3. sweep + sample
print("[x86] sweeping + sampling...", flush=True)
samples = []
sweep = [(8000,8000),(14000,8000),(20000,12000),(14000,16000),(9000,14000)]
for w in range(6):
    for (x,y) in sweep: move(x,y); time.sleep(0.3)
    time.sleep(4)
    samples.append(present()); print(f"[x86]   window {w}: present={samples[-1]}", flush=True)

advanced = samples[-1] - pc_launch
stuck = sum(1 for i in range(1,len(samples)) if samples[i]==samples[i-1])
data = reads().decode("latin1")
crash = {m: data.count(m) for m in ["panic","KERNEL PANIC","FATAL","Unhandled Exception"] if data.count(m)>0}
print("[x86] ===== VERDICT =====", flush=True)
print(f"[x86] launch={pc_launch} final={samples[-1]} advanced={advanced} stuck={stuck}/{len(samples)-1}", flush=True)
print(f"[x86] crash markers: {crash if crash else 'NONE'}", flush=True)
ok = advanced > 5 and stuck <= 1 and not any(k in crash for k in ['panic','KERNEL PANIC','FATAL'])
print("[x86] RESULT:", "ADVANCING — no freeze (FIX WORKS)" if ok else "FROZEN / PROBLEM", flush=True)
qmp.close(); qemu.send_signal(signal.SIGTERM)
try: qemu.wait(timeout=15)
except subprocess.TimeoutExpired: qemu.kill()
sys.exit(0 if ok else 1)
