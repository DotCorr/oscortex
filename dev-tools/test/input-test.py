import json,os,socket,subprocess,time,signal
ISO=""+os.environ.get("OSCORTEX_ISO", (os.environ.get("OSCORTEX_REPO") or ".")+"/oscortex.iso")+""
SER="/tmp/osc-x86-input.log"; QMP="/tmp/osc-x86-input-qmp.sock"
for p in (SER,QMP):
    try: os.remove(p)
    except OSError: pass
q=subprocess.Popen(["qemu-system-x86_64",
  "-drive","if=pflash,format=raw,readonly=on,file=/tmp/ovmf-code.fd",
  "-drive","if=pflash,format=raw,file=/tmp/ovmf-vars.fd",
  "-cdrom",ISO,"-m","4096M","-smp","2","-cpu","qemu64","-machine","q35",
  "-device","virtio-vga","-device","qemu-xhci,id=xhci",
  "-device","usb-tablet,bus=xhci.0","-device","usb-kbd,bus=xhci.0",
  "-serial",f"file:{SER}","-display","none","-no-reboot",
  "-qmp",f"unix:{QMP},server,nowait"],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
def reads():
    try: return open(SER,'rb').read()
    except: return b""
def present(): return reads().count(b"present_callback")
t0=time.time()
while time.time()-t0<200 and present()<=30: time.sleep(3)
print("shell present=",present(),"after",int(time.time()-t0),"s")
s=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
for _ in range(60):
    try: s.connect(QMP); break
    except: time.sleep(0.5)
buf=b""
def rcv():
    global buf
    while b"\n" not in buf: buf+=s.recv(4096)
    l,buf=buf.split(b"\n",1); return json.loads(l)
rcv(); s.sendall(b'{"execute":"qmp_capabilities"}\n'); rcv()
def ev(e): s.sendall((json.dumps({"execute":"input-send-event","arguments":{"events":e}})+"\n").encode()); rcv()
def move(x,y): ev([{"type":"abs","data":{"axis":"x","value":x}},{"type":"abs","data":{"axis":"y","value":y}}])
base=present()
# sweep abs pointer — if the kernel processes tablet input, cursor redraws → present advances
for i in range(8):
    move(4000+i*3000, 6000+i*2000); time.sleep(0.6)
time.sleep(3)
after=present()
print(f"present base={base} after_sweep={after} delta={after-base}")
print("tablet detected:", b"absolute pointer (tablet)" in reads())
print("pointer working (present advanced on moves):", after-base>3)
s.close(); q.send_signal(signal.SIGTERM)
try: q.wait(timeout=10)
except: q.kill()
