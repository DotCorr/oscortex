import sys
import os

def patch_kernel_blob(file_path):
    if not os.path.exists(file_path):
        print(f"Error: File not found: {file_path}")
        sys.exit(1)
        
    with open(file_path, "r+b") as f:
        data = f.read(32)
        if len(data) < 18:
            print(f"Error: File too short: {file_path}")
            sys.exit(1)
            
        # Verify Dill magic (90 ab cd ef)
        if data[0:4] != b'\x90\xab\xcd\xef':
            print(f"Warning: Magic mismatch in {file_path}: {data[0:4].hex()}")
            
        # Extract the current 10-char hash at offset 8
        current_hash = data[8:18]
        try:
            current_hash_str = current_hash.decode('ascii')
        except Exception:
            current_hash_str = current_hash.hex()
            
        print(f"Found current SDK hash in {file_path}: {current_hash_str}")
        
        # Patch the hash
        f.seek(8)
        f.write(b"0000000000")
        print(f"Patched SDK hash in {file_path} to: 0000000000")

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python3 patch_kernel_blob.py <path_to_kernel_blob.bin>")
        sys.exit(1)
    patch_kernel_blob(sys.argv[1])
