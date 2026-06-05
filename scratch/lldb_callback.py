import lldb
import struct

def on_vsnprintf_hit(frame, bp_loc, dict):
    thread = frame.GetThread()
    process = thread.GetProcess()
    # Read rdx register
    rdx_reg = frame.FindRegister("rdx")
    if not rdx_reg or not rdx_reg.IsValid():
        return False
    rdx = rdx_reg.GetValueAsUnsigned()
    # If rdx points to the target format string
    if rdx == 0x11ad1c7:
        print("====== TARGET VSNPRINTF HIT ======")
        # Read registers
        print("Kernel registers:")
        for reg in frame.registers[0]:
            if reg.name in ["rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rsp", "rbp", "rip"]:
                print(f"  {reg.name} = {reg.value}")
        
        # Read user stack
        error = lldb.SBError()
        # We scan the user stack around 0x3e30ffa00
        mem = process.ReadMemory(0x3e30ffa00, 512, error)
        if error.Success():
            print("User Stack Dump:")
            for offset in range(0, 512, 8):
                val = struct.unpack("<Q", mem[offset:offset+8])[0]
                # If val looks like a code address in the engine (e.g. 0x01000000 to 0x04000000)
                # print it with symbol info
                if 0x01000000 <= val <= 0x04000000:
                    print(f"  [0x{0x3e30ffa00 + offset:x}] = 0x{val:x} (Engine Offset: 0x{val - 0x01000000:x})")
                else:
                    print(f"  [0x{0x3e30ffa00 + offset:x}] = 0x{val:x}")
        else:
            print(f"Failed to read user stack: {error}")
            
        print("====== END OF DUMP ======")
        # Stop execution so we can inspect
        return True
    return False

def __lldb_init_module(debugger, internal_dict):
    # Set the breakpoint command when imported
    target = debugger.GetSelectedTarget()
    bp = target.BreakpointCreateByAddress(0xffffffff800122e0)
    bp.SetScriptCallbackFunction("lldb_callback.on_vsnprintf_hit")
    print("Breakpoint set at 0xffffffff800122e0 with script callback.")
