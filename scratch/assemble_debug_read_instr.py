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
    push rbp
    
    mov rbx, rsi   # rbx = Code object
    mov r14, rdi   # r14 = Deserializer
    
    # --- DEBUG PRINT ---
    # Print ImageReader pointer [r14 + 0x58]
    mov rax, [r14 + 0x58]
    call print_hex_sys
    
    # Print ImageReader base [[r14 + 0x58] + 8]
    test rax, rax
    jz .skip_base
    mov rax, [rax + 8]
    call print_hex_sys
.skip_base:
    # -------------------
    
    # 1. Decode pc_offset
    mov rsi, [r14 + 0x48]   # rsi = cursor
    call decode_header
    mov [r14 + 0x48], rsi   # save cursor
    
    # --- DEBUG PRINT ---
    # Print decoded pc_offset
    call print_hex_sys
    # -------------------
    
    # Call GetInstructionsAt(image, pc_offset)
    mov rsi, rax            # rsi = pc_offset
    mov rdi, [r14 + 0x58]   # rdi = image
    
    # call GetInstructionsAt
    .byte 0xe8
    .long {disp_call1}
    
    # --- DEBUG PRINT ---
    # Print returned InstructionsPtr
    call print_hex_sys
    # -------------------
    
    # Store instructions_
    mov [rbx + 0x2f], rax
    
    # 2. Decode unchecked_entry_point_pc_offset
    mov rsi, [r14 + 0x48]
    call decode_header
    mov [r14 + 0x48], rsi
    mov [rbx + 0xab], eax   # unchecked_entry_point_pc_offset_ (32-bit)
    
    # 3. Decode monomorphic_pc_offset
    mov rsi, [r14 + 0x48]
    call decode_header
    mov [r14 + 0x48], rsi
    
    # Call GetInstructionsAt(image, monomorphic_pc_offset)
    mov rsi, rax            # rsi = monomorphic_pc_offset
    mov rdi, [r14 + 0x58]   # rdi = image
    
    # call GetInstructionsAt
    .byte 0xe8
    .long {disp_call2}
    
    # Store monomorphic_instructions_
    mov [rbx + 0x6f], rax
    
    # 4. Decode monomorphic_entry_point_pc_offset
    mov rsi, [r14 + 0x48]
    call decode_header
    mov [r14 + 0x48], rsi
    
    # Tail call setup
    mov rdi, rbx            # rdi = Code
    mov rsi, rax            # rsi = monomorphic_instructions
    
    pop rbp
    pop rbx
    pop r14
    
    # jmp tail_call_target
    .byte 0xe9
    .long {disp_jmp}

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
    
    # format rax to hex in [rdi]
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
    
    # sys_write(fd=1, buf=rsp, len=17)
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

# Helper to decode one ReadHeaderValue
decode_header:
    movzx eax, byte ptr [rsi]
    inc rsi
    test al, al
    js .byte1_term          # if b1 >= 0x80, it's a 1-byte value

    # 2-byte case:
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 7
    or eax, edx
    test cl, cl
    js .byte2_term          # if b2 >= 0x80, it's a 2-byte value

    # 3-byte case:
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 14
    or eax, edx
    test cl, cl
    js .byte3_term          # if b3 >= 0x80, it's a 3-byte value

    # 4-byte case:
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 21
    or eax, edx
    test cl, cl
    js .byte4_term          # if b4 >= 0x80, it's a 4-byte value

    # 5-byte case:
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 28
    or eax, edx
    ret

.byte1_term:
    sub eax, 192
    cdqe                    # sign-extend eax to rax
    ret

.byte2_term:
    sub eax, 24576
    cdqe
    ret

.byte3_term:
    sub eax, 3145728
    cdqe
    ret

.byte4_term:
    sub eax, 402653184
    cdqe
    ret
    """

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
        
        # We expect 3 calls (2 to GetInstructionsAt and 1 to print_hex_sys inside decode_header? No, wait!
        # Let's count calls in asm_template:
        # 1. call print_hex_sys (ImageReader pointer)
        # 2. call print_hex_sys (ImageReader base)
        # 3. call decode_header
        # 4. call print_hex_sys (decoded pc_offset)
        # 5. call GetInstructionsAt (disp_call1)
        # 6. call print_hex_sys (returned InstructionsPtr)
        # 7. call decode_header
        # 8. call decode_header
        # 9. call GetInstructionsAt (disp_call2)
        # 10. call decode_header
        # Wait, decode_header is a local symbol, so clang resolves it locally (using relative call offset, which is not 0x00000000).
        # print_hex_sys is also a local symbol, resolved locally.
        # Only the external targets GetInstructionsAt and tail_call_target are left as relocations with 0 displacements (if they are treated as external).
        # Let's check which indices correspond to disp_call1, disp_call2, and disp_jmp.
        
        print(f"Call indices: {indices}, Jmp index: {jmp_idx}")
        # Let's see: GetInstructionsAt placeholders will have relocations, so they are the ones we formatted.
        
        # Let's look at the generated assembly file text to locate GetInstructionsAt calls.
        # To avoid confusion, let's use different placeholders for local vs external.
        # Actually, since decode_header and print_hex_sys are defined in the same assembly file,
        # clang compiles them as local relocations or resolves them directly, so they will not be \xe8\x00\x00\x00\x00.
        # Only the external targets (.long 0) will be \xe8\x00\x00\x00\x00.
        # So we should have exactly 2 call placeholders and 1 jmp placeholder!
        # Let's verify this by checking len(indices).
        print(f"Number of 0-displacement calls: {len(indices)}")
        if len(indices) != 2 or jmp_idx == -1:
            print("Error: could not locate placeholders precisely")
            return
            
        target_call_va = 0x22cfe60
        target_jmp_va = 0x2346f60
        patch_start_va = 0x2292070
        
        call1_site_va = patch_start_va + indices[0]
        disp_call1 = target_call_va - (call1_site_va + 5)
        
        call2_site_va = patch_start_va + indices[1]
        disp_call2 = target_call_va - (call2_site_va + 5)
        
        jmp_site_va = patch_start_va + jmp_idx
        disp_jmp = target_jmp_va - (jmp_site_va + 5)
        
        disp_call1_u32 = disp_call1 & 0xffffffff
        disp_call2_u32 = disp_call2 & 0xffffffff
        disp_jmp_u32 = disp_jmp & 0xffffffff
        
        s_file.write_text(asm_template.format(disp_call1=disp_call1_u32, disp_call2=disp_call2_u32, disp_jmp=disp_jmp_u32))
        res = subprocess.run(["clang", "-target", "x86_64-pc-linux-gnu", "-c", "-o", str(o_file), str(s_file)], capture_output=True)
        if res.returncode != 0:
            print("Clang failed:", res.stderr.decode())
            return
            
        final_text = get_text_section(o_file.read_bytes())
        print("Final patch bytes (length = {}):".format(len(final_text)))
        print(final_text.hex())
        
        if len(final_text) > 496:
            print("WARNING: Patch exceeds budget of 496 bytes!")

if __name__ == "__main__":
    main()
