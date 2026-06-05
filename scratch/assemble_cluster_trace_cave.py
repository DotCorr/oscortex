import subprocess
import tempfile
from pathlib import Path

def main():
    asm_template = """
    .intel_syntax noprefix
    .global _start
    _start:
    # Save all registers
    push rdi
    push rsi
    push rdx
    push rcx
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    push rbx
    push rbp
    
    # 1. Print sentinel 0xcccccccccccccccc
    mov rax, 0xcccccccccccccccc
    call print_hex_sys
    
    # 2. Print cluster index r13
    mov rax, r13
    call print_hex_sys
    
    # 3. Print cluster pointer rdi
    mov rax, rdi
    call print_hex_sys
    
    # 4. Print stream cursor before [r14 + 0x48]
    mov rax, [r14 + 0x48]
    call print_hex_sys
    
    # Restore registers before calling ReadFill
    # (Note: rax is not popped here, but rax is clobbered by call anyway, we need to preserve rax which holds the vtable)
    # Actually, we pushed rdi..rbp, rbx, rbp. Let us pop in exact reverse order:
    pop rbp
    pop rbx
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rcx
    pop rdx
    pop rsi
    pop rdi
    
    # Call the actual ReadFill function (vtable + 0x18)
    call qword ptr [rax + 0x18]
    
    # Save registers again to print after-state
    push rdi
    push rsi
    push rdx
    push rcx
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    push rbx
    push rbp
    
    # 5. Print stream cursor after [r14 + 0x48]
    mov rax, [r14 + 0x48]
    call print_hex_sys
    
    # 6. Print sentinel 0xdddddddddddddddd
    mov rax, 0xdddddddddddddddd
    call print_hex_sys
    
    # Restore registers
    pop rbp
    pop rbx
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rcx
    pop rdx
    pop rsi
    pop rdi
    
    # Replaced instruction
    inc r13
    ret

# Prints rax as hex using sys_write
print_hex_sys:
    push rdi
    push rsi
    push rdx
    push rcx
    push r11
    push rax
    push rbx
    
    sub rsp, 32
    mov rdi, rsp
    
    mov rcx, 16
.loop_print:
    rol rax, 4
    mov ebx, eax
    and ebx, 0xf
    add bl, '0'
    cmp bl, '9'
    jbe .ok_print
    add bl, 7
.ok_print:
    mov [rdi], bl
    inc rdi
    dec rcx
    jnz .loop_print
    
    mov byte ptr [rdi], 10 # newline
    
    mov eax, 1             # sys_write
    mov edi, 1             # fd = 1
    mov rsi, rsp           # buf
    mov edx, 17            # len = 17
    syscall
    
    add rsp, 32
    pop rbx
    pop rax
    pop r11
    pop rcx
    pop rdx
    pop rsi
    pop rdi
    ret
    """

    import sys
    sys.path.append("/Users/ghostportal/Desktop/Dotcorr/OSCortex/scratch")
    from assemble_clang import get_text_section

    with tempfile.TemporaryDirectory() as tmpdir:
        s_file = Path(tmpdir) / "temp.s"
        o_file = Path(tmpdir) / "temp.o"
        
        s_file.write_text(asm_template)
        res = subprocess.run(["clang", "-target", "x86_64-pc-linux-gnu", "-c", "-o", str(o_file), str(s_file)], capture_output=True)
        if res.returncode != 0:
            print("Clang failed:", res.stderr.decode())
            return
            
        final_text = get_text_section(o_file.read_bytes())
        print(f"Cave length = {len(final_text)}")
        print(final_text.hex())
        
        with open("scratch/cluster_trace_cave.bin", "wb") as f:
            f.write(final_text)

if __name__ == "__main__":
    main()
