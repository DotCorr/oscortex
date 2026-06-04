import capstone

# New trampoline assembly bytes with prefix check 0x15b and negative relocation offset
# rel_offset = -0x19d7ebb8 = 0xe6281448
TRAMPOLINE = bytes([
    0x48, 0x8B, 0x47, 0x08,             # mov rax, [rdi + 8]
    0x48, 0x89, 0xC2,                   # mov rdx, rax
    0x48, 0xC1, 0xEA, 0x18,             # shr rdx, 24
    0x81, 0xEA, 0x5B, 0x01, 0x00, 0x00, # sub edx, 0x15b
    0x83, 0xFA, 0x02,                   # cmp edx, 2
    0x77, 0x06,                         # ja +6
    0x48, 0x05, 0x48, 0x14, 0x28, 0xE6, # add rax, -0x19d7ebb8 (0xe6281448)
    0x48, 0x63, 0xF6,                   # movsxd rsi, esi
    0x48, 0x8D, 0x44, 0x30, 0x01,       # lea rax, [rax + rsi + 1]
    0xC3                                # ret
])

md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
for i in md.disasm(TRAMPOLINE, 0):
    print(f"0x{i.address:x}:\t{i.mnemonic}\t{i.op_str}\t(bytes: {i.bytes.hex()})")
