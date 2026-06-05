import sys
from capstone import Cs, CS_ARCH_X86, CS_MODE_64

bytes_str = "48 8b 7f 08 48 01 d2 48 8d 04 b6 48 c1 e0 04 48 8d 34 07 48 83 c6 3f 48 ff cf e9 d1 49 f8 ff cc"
code = bytes.fromhex(bytes_str.replace(" ", ""))

md = Cs(CS_ARCH_X86, CS_MODE_64)
for i in md.disasm(code, 0x142346f60):
    print(f"0x{i.address:x}:\t{i.mnemonic}\t{i.op_str}")
