import subprocess
import tempfile
import struct
from pathlib import Path

# VA layout info
# Code start VA: 0x2292070
# GetInstructionsAt VA: 0x22cfe60
# Tail-call target VA: 0x2346f60
# Cave start VA: 0x1F45800

def main():
    asm_template = """
    .intel_syntax noprefix
    .global _start
    _start:
    push r14
    push rbx
    push rbp
    push r15
    
    mov r15, r14   # r15 = loop counter (stored in r14 on entry)
    mov rbx, rsi   # rbx = Code object
    mov r14, rdi   # r14 = Deserializer
    
    # Print Header
    mov rax, 0xbbbbbbbbbbbbbbbb
    call maybe_print
    mov rax, r15
    call maybe_print
    mov rax, rbx
    call maybe_print
    
    # 1. Decode pc_offset (signed)
    mov rax, [r14 + 0x48]
    call maybe_print # print cursor
    mov rcx, [r14 + 0x48]
    mov rax, [rcx]
    call maybe_print # print stream bytes (8 bytes)
    
    mov rsi, [r14 + 0x48]   # rsi = cursor
    call decode_signed
    mov [r14 + 0x48], rsi   # save cursor
    call maybe_print # print decoded pc_offset
    
    # Call ImageReader::GetInstructionsAt(image, pc_offset)
    mov rsi, rax            # rsi = pc_offset
    mov rdi, [r14 + 0x58]   # rdi = image
    
    # call GetInstructionsAt
    .byte 0xe8
    .long {disp_call1}
    
    # Store instructions_
    mov [rbx + 0x2f], rax
    
    # 2. Decode unchecked_entry_point_pc_offset (unsigned)
    mov rax, [r14 + 0x48]
    call maybe_print # print cursor
    mov rcx, [r14 + 0x48]
    mov rax, [rcx]
    call maybe_print # print stream bytes
    
    mov rsi, [r14 + 0x48]
    call decode_unsigned
    mov [r14 + 0x48], rsi
    call maybe_print # print decoded unchecked_entry_point_pc_offset
    mov [rbx + 0xab], eax   # unchecked_entry_point_pc_offset_ (32-bit)
    
    # 3. Decode monomorphic_pc_offset (signed)
    mov rax, [r14 + 0x48]
    call maybe_print # print cursor
    mov rcx, [r14 + 0x48]
    mov rax, [rcx]
    call maybe_print # print stream bytes
    
    mov rsi, [r14 + 0x48]
    call decode_signed
    mov [r14 + 0x48], rsi
    call maybe_print # print decoded monomorphic_pc_offset
    
    # Call ImageReader::GetInstructionsAt(image, monomorphic_pc_offset)
    mov rsi, rax            # rsi = monomorphic_pc_offset
    mov rdi, [r14 + 0x58]   # rdi = image
    
    # call GetInstructionsAt
    .byte 0xe8
    .long {disp_call2}
    
    # Store monomorphic_instructions_
    mov [rbx + 0x6f], rax
    mov r12, rax            # Save monomorphic_instructions in r12 (callee-saved)
    call maybe_print # print monomorphic_instructions ptr
    
    # 4. Decode monomorphic_entry_point_pc_offset (unsigned)
    mov rax, [r14 + 0x48]
    call maybe_print # print cursor
    mov rcx, [r14 + 0x48]
    mov rax, [rcx]
    call maybe_print # print stream bytes
    
    mov rsi, [r14 + 0x48]
    call decode_unsigned
    mov [r14 + 0x48], rsi
    call maybe_print # print decoded monomorphic_entry_point_pc_offset
    
    # Print Footer
    push rax
    mov rax, 0xeeeeeeeeeeeeeeee
    call maybe_print
    pop rax
    
    # Tail call setup
    mov rdi, rbx            # rdi = Code
    mov rsi, r12            # rsi = monomorphic_instructions
    mov rdx, rax            # rdx = monomorphic_entry_point_pc_offset (3rd argument)
    
    pop r15
    pop rbp
    pop rbx
    pop r14
    
    # jmp tail_call_target
    .byte 0xe9
    .long {disp_jmp}

maybe_print:
    cmp r15, 0x4c0
    jb .no_print
    call print_hex_sys
.no_print:
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

# Helper to decode one signed ReadHeaderValue
decode_signed:
    movzx eax, byte ptr [rsi]
    inc rsi
    test al, al
    js .s_byte1_term
    
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 7
    or eax, edx
    test cl, cl
    js .s_byte2_term
    
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 14
    or eax, edx
    test cl, cl
    js .s_byte3_term
    
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 21
    or eax, edx
    test cl, cl
    js .s_byte4_term
    
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 28
    or eax, edx
    ret

.s_byte1_term:
    sub eax, 192
    cdqe
    ret

.s_byte2_term:
    sub eax, 24576
    cdqe
    ret

.s_byte3_term:
    sub eax, 3145728
    cdqe
    ret

.s_byte4_term:
    sub eax, 402653184
    cdqe
    ret

# Helper to decode one unsigned Base 128 Varint
decode_unsigned:
    movzx eax, byte ptr [rsi]
    inc rsi
    test al, al
    js .u_byte1_term
    
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 7
    or eax, edx
    test cl, cl
    js .u_byte2_term
    
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 14
    or eax, edx
    test cl, cl
    js .u_byte3_term
    
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 21
    or eax, edx
    test cl, cl
    js .u_byte4_term
    
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 28
    or eax, edx
    ret

.u_byte1_term:
    sub eax, 128
    cdqe
    ret

.u_byte2_term:
    sub eax, 16384
    cdqe
    ret

.u_byte3_term:
    sub eax, 2097152
    cdqe
    ret

.u_byte4_term:
    sub eax, 268435456
    cdqe
    ret
    """

    import sys
    sys.path.append("/Users/ghostportal/Desktop/Dotcorr/OSCortex/scratch")
    from assemble_clang import get_text_section

    with tempfile.TemporaryDirectory() as tmpdir:
        s_file = Path(tmpdir) / "temp.s"
        o_file = Path(tmpdir) / "temp.o"
        
        # Write dummy asm to find call/jmp sites
        s_file.write_text(asm_template.format(disp_call1=0, disp_call2=0, disp_jmp=0))
        res = subprocess.run(["clang", "-target", "x86_64-pc-linux-gnu", "-c", "-o", str(o_file), str(s_file)], capture_output=True)
        if res.returncode != 0:
            print("Clang failed:", res.stderr.decode())
            return
            
        text_data = get_text_section(o_file.read_bytes())
        
        indices = []
        start = 0
        while True:
            idx = text_data.find(b"\xe8\x00\x00\x00\x00", start)
            if idx == -1:
                break
            indices.append(idx)
            start = idx + 5
            
        jmp_idx = text_data.find(b"\xe9\x00\x00\x00\x00")
        
        print(f"Call indices: {indices}, Jmp index: {jmp_idx}")
        if len(indices) != 2 or jmp_idx == -1:
            print("Error: could not locate placeholders precisely")
            return
            
        target_call_va = 0x22cfe60
        target_jmp_va = 0x2346f60
        cave_start_va = 0x1F45A00
        
        call1_site_va = cave_start_va + indices[0]
        disp_call1 = target_call_va - (call1_site_va + 5)
        
        call2_site_va = cave_start_va + indices[1]
        disp_call2 = target_call_va - (call2_site_va + 5)
        
        jmp_site_va = cave_start_va + jmp_idx
        disp_jmp = target_jmp_va - (jmp_site_va + 5)
        
        disp_call1_u32 = disp_call1 & 0xffffffff
        disp_call2_u32 = disp_call2 & 0xffffffff
        disp_jmp_u32 = disp_jmp & 0xffffffff
        
        # Write final asm with calculated displacements
        s_file.write_text(asm_template.format(disp_call1=disp_call1_u32, disp_call2=disp_call2_u32, disp_jmp=disp_jmp_u32))
        res = subprocess.run(["clang", "-target", "x86_64-pc-linux-gnu", "-c", "-o", str(o_file), str(s_file)], capture_output=True)
        if res.returncode != 0:
            print("Clang failed:", res.stderr.decode())
            return
            
        final_text = get_text_section(o_file.read_bytes())
        print("Final patch bytes (length = {}):".format(len(final_text)))
        print(final_text.hex())
        
        # Output binary files or python formatted bytes
        with open("scratch/debug_cave_bytes.bin", "wb") as f:
            f.write(final_text)

if __name__ == "__main__":
    main()
