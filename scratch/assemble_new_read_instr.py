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
    push r15
    
    mov rbx, rsi   # rbx = Code object
    mov r14, rdi   # r14 = Deserializer
    
    # 1. Decode pc_offset (signed)
    mov rsi, [r14 + 0x48]   # rsi = cursor
    call decode_signed
    mov [r14 + 0x48], rsi   # save cursor
    
    # Call ImageReader::GetInstructionsAt(image, pc_offset)
    mov rsi, rax            # rsi = pc_offset
    mov rdi, [r14 + 0x58]   # rdi = image
    
    # call GetInstructionsAt
    .byte 0xe8
    .long {disp_call1}
    
    # Store instructions_
    mov [rbx + 0x2f], rax
    
    # 2. Decode unchecked_entry_point_pc_offset (unsigned)
    mov rsi, [r14 + 0x48]
    call decode_unsigned
    mov [r14 + 0x48], rsi
    mov [rbx + 0xab], eax   # unchecked_entry_point_pc_offset_ (32-bit)
    
    # 3. Decode monomorphic_pc_offset (signed)
    mov rsi, [r14 + 0x48]
    call decode_signed
    mov [r14 + 0x48], rsi
    
    # Call ImageReader::GetInstructionsAt(image, monomorphic_pc_offset)
    mov rsi, rax            # rsi = monomorphic_pc_offset
    mov rdi, [r14 + 0x58]   # rdi = image
    
    # call GetInstructionsAt
    .byte 0xe8
    .long {disp_call2}
    
    # Store monomorphic_instructions_
    mov [rbx + 0x6f], rax
    mov r15, rax            # Save monomorphic_instructions in r15
    
    # 4. Decode monomorphic_entry_point_pc_offset (unsigned)
    mov rsi, [r14 + 0x48]
    call decode_unsigned
    mov [r14 + 0x48], rsi
    
    # Tail call setup
    mov rdi, rbx            # rdi = Code
    mov rsi, r15            # rsi = monomorphic_instructions
    mov rdx, rax            # rdx = monomorphic_entry_point_pc_offset (3rd argument)
    
    pop r15
    pop rbp
    pop rbx
    pop r14
    
    # jmp tail_call_target
    .byte 0xe9
    .long {disp_jmp}

# Helper to decode one signed ReadHeaderValue
# Input: rsi = stream cursor
# Output: rax = decoded value, rsi = updated cursor
# Clobbers: rcx, rdx
decode_signed:
    movzx eax, byte ptr [rsi]
    inc rsi
    test al, al
    js .s_byte1_term          # if b1 >= 0x80, it's a 1-byte value

    # 2-byte case:
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 7
    or eax, edx
    test cl, cl
    js .s_byte2_term          # if b2 >= 0x80, it's a 2-byte value

    # 3-byte case:
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 14
    or eax, edx
    test cl, cl
    js .s_byte3_term          # if b3 >= 0x80, it's a 3-byte value

    # 4-byte case:
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 21
    or eax, edx
    test cl, cl
    js .s_byte4_term          # if b4 >= 0x80, it's a 4-byte value

    # 5-byte case:
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 28
    or eax, edx
    ret

.s_byte1_term:
    sub eax, 192
    cdqe                    # sign-extend eax to rax
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
# Input: rsi = stream cursor
# Output: rax = decoded value, rsi = updated cursor
# Clobbers: rcx, rdx
decode_unsigned:
    movzx eax, byte ptr [rsi]
    inc rsi
    test al, al
    js .u_byte1_term          # if b1 >= 0x80, it's a 1-byte value

    # 2-byte case:
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 7
    or eax, edx
    test cl, cl
    js .u_byte2_term          # if b2 >= 0x80, it's a 2-byte value

    # 3-byte case:
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 14
    or eax, edx
    test cl, cl
    js .u_byte3_term          # if b3 >= 0x80, it's a 3-byte value

    # 4-byte case:
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 21
    or eax, edx
    test cl, cl
    js .u_byte4_term          # if b4 >= 0x80, it's a 4-byte value

    # 5-byte case:
    movzx ecx, byte ptr [rsi]
    inc rsi
    mov edx, ecx
    shl edx, 28
    or eax, edx
    ret

.u_byte1_term:
    sub eax, 128
    cdqe                    # sign-extend eax to rax
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
        
        # Write final asm with calculated displacements
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
