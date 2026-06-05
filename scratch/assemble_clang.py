import subprocess
import tempfile
import struct
from pathlib import Path

def get_text_section(elf_bytes):
    # Parse ELF64 header
    e_shoff = struct.unpack("<Q", elf_bytes[0x28:0x30])[0]
    e_shentsize = struct.unpack("<H", elf_bytes[0x3a:0x3c])[0]
    e_shnum = struct.unpack("<H", elf_bytes[0x3c:0x3e])[0]
    e_shstrndx = struct.unpack("<H", elf_bytes[0x3e:0x40])[0]
    
    # Get section header string table
    shstr_offset_addr = e_shoff + e_shstrndx * e_shentsize
    shstr_offset = struct.unpack("<Q", elf_bytes[shstr_offset_addr + 24 : shstr_offset_addr + 32])[0]
    
    for i in range(e_shnum):
        sh_addr = e_shoff + i * e_shentsize
        sh_name_offset = struct.unpack("<I", elf_bytes[sh_addr : sh_addr + 4])[0]
        
        # Read name
        name_bytes = []
        curr = shstr_offset + sh_name_offset
        while curr < len(elf_bytes) and elf_bytes[curr] != 0:
            name_bytes.append(elf_bytes[curr])
            curr += 1
        name = bytes(name_bytes).decode('ascii', errors='ignore')
        
        if name == ".text":
            sh_offset = struct.unpack("<Q", elf_bytes[sh_addr + 24 : sh_addr + 32])[0]
            sh_size = struct.unpack("<Q", elf_bytes[sh_addr + 32 : sh_addr + 40])[0]
            return elf_bytes[sh_offset : sh_offset + sh_size]
    return None

def main():
    asm_code = """
    .intel_syntax noprefix
    .global _start
    _start:
    mov rax, [r13]
    mov [r12 + 0x27], rax
    mov [r12 + 0x57], rax
    """
    
    with tempfile.TemporaryDirectory() as tmpdir:
        s_file = Path(tmpdir) / "temp.s"
        o_file = Path(tmpdir) / "temp.o"
        s_file.write_text(asm_code)
        
        # Compile targeting x86_64
        res = subprocess.run(["clang", "-target", "x86_64-pc-linux-gnu", "-c", "-o", str(o_file), str(s_file)], capture_output=True)
        if res.returncode != 0:
            print("Clang failed:", res.stderr.decode())
            return
            
        data = o_file.read_bytes()
        text_data = get_text_section(data)
        if text_data:
            print("Hex of instructions (.text):", text_data.hex())
            # mov rax, [r13]
            # mov [r12 + 0x27], rax
            # mov [r12 + 0x57], rax
            print("mov rax, [r13]:", text_data[0:4].hex())
            print("mov [r12 + 0x27], rax:", text_data[4:9].hex())
            print("mov [r12 + 0x57], rax:", text_data[9:14].hex())
        else:
            print(".text section not found")

if __name__ == "__main__":
    main()
