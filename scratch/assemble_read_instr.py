import subprocess
import tempfile
import struct
from pathlib import Path

# VA layout info
# Code start VA: 0x2292070
# GetInstructionsAt VA: 0x22cfe60
# Tail-call target VA: 0x2346f60

def main():
    asm_template = """
    .intel_syntax noprefix
    .global _start
    _start:
    push r14
    push rbx
    push rax
    
    mov rbx, rsi   # rbx = code
    mov r14, rdi   # r14 = Deserializer
    
    # Read one LEB128 from stream cursor [r14 + 0x48]
    mov rsi, [r14 + 0x48]   # rsi = cursor
    xor eax, eax            # eax = result (pc_offset)
    xor ecx, ecx            # ecx = shift (cl)
    
.leb_loop:
    movzx r8d, byte ptr [rsi]
    inc rsi
    mov r9d, r8d
    and r9d, 0x7f
    shl r9, cl
    or rax, r9
    add ecx, 7
    test r8b, 0x80
    jnz .leb_loop
    
    mov [r14 + 0x48], rsi   # Save cursor back
    
    # Call ImageReader::GetInstructionsAt(image, pc_offset)
    # 1st arg: rdi = image = [r14 + 0x58]
    # 2nd arg: rsi = pc_offset = rax
    mov rsi, rax
    mov rdi, [r14 + 0x58]
    
    # call 0x22cfe60
    .byte 0xe8
    .long {disp_call}
    
    # Store returned InstructionsPtr (rax) into Code fields
    mov [rbx + 0x2f], rax      # instructions_ (offset 48)
    mov [rbx + 0x6f], rax      # monomorphic_instructions_ (offset 112)
    mov dword ptr [rbx + 0xab], 0  # unchecked_entry_point_pc_offset_ (offset 172)
    
    # Tail call to 0x2346f60
    mov rdi, rbx   # rdi = code
    mov rsi, rax   # rsi = instructions
    
    add rsp, 8
    pop rbx
    pop r14
    
    # jmp 0x2346f60
    .byte 0xe9
    .long {disp_jmp}
    """

    # Helper function to get text section
    from assemble_clang import get_text_section

    with tempfile.TemporaryDirectory() as tmpdir:
        s_file = Path(tmpdir) / "temp.s"
        o_file = Path(tmpdir) / "temp.o"
        
        s_file.write_text(asm_template.format(disp_call=0, disp_jmp=0))
        res = subprocess.run(["clang", "-target", "x86_64-pc-linux-gnu", "-c", "-o", str(o_file), str(s_file)], capture_output=True)
        if res.returncode != 0:
            print("Clang failed:", res.stderr.decode())
            return
            
        text_data = get_text_section(o_file.read_bytes())
        call_idx = text_data.find(b"\xe8\x00\x00\x00\x00")
        jmp_idx = text_data.find(b"\xe9\x00\x00\x00\x00")
        
        print(f"Call index: {call_idx}, Jmp index: {jmp_idx}")
        
        target_call_va = 0x22cfe60
        call_site_va = 0x2292070 + call_idx
        disp_call = target_call_va - (call_site_va + 5)
        
        target_jmp_va = 0x2346f60
        jmp_site_va = 0x2292070 + jmp_idx
        disp_jmp = target_jmp_va - (jmp_site_va + 5)
        
        disp_call_u32 = disp_call & 0xffffffff
        disp_jmp_u32 = disp_jmp & 0xffffffff
        
        s_file.write_text(asm_template.format(disp_call=disp_call_u32, disp_jmp=disp_jmp_u32))
        res = subprocess.run(["clang", "-target", "x86_64-pc-linux-gnu", "-c", "-o", str(o_file), str(s_file)], capture_output=True)
        if res.returncode != 0:
            print("Clang failed:", res.stderr.decode())
            return
            
        final_text = get_text_section(o_file.read_bytes())
        print("Final patch bytes:")
        print(final_text.hex())
        print(f"Total length: {len(final_text)} bytes")

if __name__ == "__main__":
    main()
