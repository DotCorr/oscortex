import socket
import sys
import time

def send_cmds(cmds):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        s.connect("/tmp/qemu-monitor.sock")
    except Exception as e:
        print(f"Failed to connect: {e}")
        sys.exit(1)

    # Read initial prompt
    time.sleep(0.5)
    s.setblocking(False)
    try:
        initial = s.recv(4096)
        print("Initial:", initial.decode(errors='ignore'))
    except BlockingIOError:
        pass

    for cmd in cmds:
        print(f"Sending: {cmd}")
        s.sendall(f"{cmd}\n".encode())
        time.sleep(0.5)
        try:
            resp = s.recv(4096)
            print("Resp:", resp.decode(errors='ignore'))
        except BlockingIOError:
            pass
    s.close()

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python3 send_monitor_cmd.py 'cmd1' 'cmd2' ...")
        sys.exit(1)
    send_cmds(sys.argv[1:])
