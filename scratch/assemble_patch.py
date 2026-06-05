import sys
from pathlib import Path

def main():
    try:
        from keystone import Ks, KS_ARCH_X86, KS_MODE_64
        ks = Ks(KS_ARCH_X86, KS_MODE_64)
        
        # 1. mov rax, [r13]
        # 2. mov [r12 + 0x27], rax
        encoding, count = ks.asm("mov rax, [r13]\nmov [r12 + 0x27], rax")
        print("object_pool_ patch:", bytes(encoding).hex())
        
        # 3. mov [r12 + 0x57], rax
        encoding2, count = ks.asm("mov rax, [r13]\nmov [r12 + 0x57], rax")
        print("compressed_stackmaps_ patch:", bytes(encoding2).hex())
        
    except ImportError:
        print("Keystone not installed. We will use a script to call llvm-mc or check in another way.")
        # Alternatively, we can check by writing a test.s and compiling it with gcc/as if available.
        # But we can also compile a quick inline assembly in C or use rust inline assembly since cargo is installed!
        # Let's see: we can run a shell command to assemble using gcc/clang.

if __name__ == "__main__":
    main()
