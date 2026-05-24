import subprocess
import re
import sys

def main():
    binary_path = "/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-embedder/target/x86_64-unknown-none/debug/flutter-embedder"
    
    # Run objdump to get disassembly
    cmd = ["/usr/bin/objdump", "-d", binary_path]
    result = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if result.returncode != 0:
        print("objdump failed:", result.stderr)
        sys.exit(1)
        
    lines = result.stdout.splitlines()
    
    # Regex to match disassembly lines like:
    #   4066a6: 48 8b 05 53 99 bf ff         	movq	-0x4066ad(%rip), %rax   # 0x0
    # or
    #   4014d5: 48 8d 3d 24 9b 00 00         	leaq	0x9b24(%rip), %rdi      # 0x40b000
    pattern = re.compile(r'^\s*([0-9a-f]+):\s+((?:[0-9a-f]{2}\s+)+)\s+([a-z]+)\s+([^#\n]+)(?:#\s+(0x[0-9a-f]+|0))?')
    
    print("Searching for RIP-relative instructions resolving to 0 or unexpected low addresses:")
    
    for line in lines:
        match = pattern.match(line)
        if match:
            addr_str, bytes_str, instr, operands, comment = match.groups()
            addr = int(addr_str, 16)
            
            # Check if operands contain (%rip)
            if "(%rip)" in operands:
                # Let's parse displacement
                # Operands looks like: -0x4066ad(%rip), %rax  or  0x9b24(%rip), %rdi
                # We can also parse the target address from the comment if objdump calculated it,
                # or calculate it ourselves.
                target = None
                if comment:
                    target = int(comment, 16)
                else:
                    # Let's compute displacement manually from the operands
                    # Look for something like: -0xabc(%rip) or 0xabc(%rip)
                    disp_match = re.search(r'(-?0x[0-9a-f]+)\(%rip\)', operands)
                    if disp_match:
                        disp = int(disp_match.group(1), 16)
                        # Instruction length is number of bytes in bytes_str
                        instr_len = len(bytes_str.strip().split())
                        next_ip = addr + instr_len
                        target = next_ip + disp
                
                if target is not None and target < 0x400000:
                    print(f"Address: 0x{addr:x} | Bytes: {bytes_str.strip()} | {instr} {operands.strip()} | Resolved Target: 0x{target:x}")

if __name__ == "__main__":
    main()
