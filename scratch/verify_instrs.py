import struct

def main():
    final_text = bytes.fromhex("415653504889f34989fe498b764831c031c9440fb60648ffc64589c14183e17f49d3e14c09c883c10741f6c08075e3498976484889c6498b7e58e8b1cd03004889432f4889436fc783ab000000000000004889df4889c64883c4085b415ee98d3e0b00")
    
    try:
        import capstone
        md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
        print("Capstone Disassembly:")
        for i in md.disasm(final_text, 0x2292070):
            print(f"VA {hex(i.address)}: {i.mnemonic:<8} {i.op_str:<40}")
    except ImportError:
        print("Capstone not installed.")

if __name__ == "__main__":
    main()
