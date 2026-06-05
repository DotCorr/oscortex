import struct
from pathlib import Path

def main():
    engine_path = Path("/Users/ghostportal/Desktop/Dotcorr/OSCortex/tools/flutter-engine/libflutter_engine.so")
    if not engine_path.exists():
        print("Engine not found")
        return
    
    data = engine_path.read_bytes()
    
    # Let's find the start of the CodeDeserializationCluster::ReadFill function:
    # 0000000002299f40 <_ZN4dart26CodeDeserializationCluster8ReadFillEPNS_12DeserializerEllb>:
    #  2299f40: 48 39 ca                        cmpq    %rcx, %rdx
    #  2299f43: 0f 8d 6d 06 00 00               jge     0x229a5b6
    #  2299f49: 55                              pushq   %rbp
    #  2299f4a: 41 57                           pushq   %r15
    #  2299f4c: 41 56                           pushq   %r14
    #  2299f4e: 41 55                           pushq   %r13
    #  2299f50: 41 54                           pushq   %r12
    #  2299f52: 53                              pushq   %rbx
    #  2299f53: 50                              pushq   %rax
    
    # We can check file offset for VA 0x2299f40.
    # In engine_patch.py, va_to_file converts VA to file offset.
    # Let's look at LOAD_SEGMENTS:
    # LOAD_SEGMENTS = (
    #     (0x0000000, 0x0000000),
    #     (0x1954248, 0x1953248),
    #     (0x266CA30, 0x266AA30),
    #     (0x26F3E00, 0x26F0E00),
    # )
    # Since 0x2299f40 >= 0x1954248 and < 0x266CA30, the file offset is:
    # 0x2299f40 - 0x1954248 + 0x1953248 = 0x2298f40.
    
    # Let's verify the bytes at file offset 0x2298f40:
    base_file_off = 0x2298f40
    print("Bytes at 0x2298f40 (expected 48 39 ca):", data[base_file_off : base_file_off + 16].hex())
    
    # Let's calculate the file offsets of object_pool_ block:
    # VA 0x2299fbd is file offset: 0x2299fbd - 0x1954248 + 0x1953248 = 0x2298fbd.
    # VA 0x229a035 is file offset: 0x229a035 - 0x1954248 + 0x1953248 = 0x2298035 + 0x1000 = 0x2299035.
    obj_pool_start = 0x2298fbd
    obj_pool_end = 0x2299035
    obj_pool_len = obj_pool_end - obj_pool_start
    print(f"object_pool_ block file offset range: {hex(obj_pool_start)} - {hex(obj_pool_end)} (len={obj_pool_len})")
    print("object_pool_ block hex:")
    print(data[obj_pool_start:obj_pool_end].hex())
    
    # Let's calculate the file offsets of compressed_stackmaps_ block:
    # VA 0x229a235 is file offset: 0x229a235 - 0x1954248 + 0x1953248 = 0x2299235.
    # VA 0x229a2b5 is file offset: 0x229a2b5 - 0x1954248 + 0x1953248 = 0x22992b5.
    stackmaps_start = 0x2299235
    stackmaps_end = 0x22992b5
    stackmaps_len = stackmaps_end - stackmaps_start
    print(f"compressed_stackmaps_ block file offset range: {hex(stackmaps_start)} - {hex(stackmaps_end)} (len={stackmaps_len})")
    print("compressed_stackmaps_ block hex:")
    print(data[stackmaps_start:stackmaps_end].hex())

if __name__ == "__main__":
    main()
