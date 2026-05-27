import os
import sys

def patch_engine(file_path):
    if not os.path.exists(file_path):
        print(f"Error: File not found: {file_path}")
        sys.exit(1)
        
    target = b'837be28dd21818d2169e1e6789e89e771a55f01d'
    replacement = b'837be28dd2' + b'\x00' * 30
    
    with open(file_path, "rb") as f:
        data = f.read()
        
    count = data.count(target)
    if count == 0:
        print("Expected SDK hash not found in engine. Maybe it is already patched?")
        return
        
    print(f"Found {count} occurrences of expected SDK hash in {file_path}")
    patched_data = data.replace(target, replacement)
    
    with open(file_path, "wb") as f:
        f.write(patched_data)
        
    print(f"Patched {file_path} successfully!")

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python3 patch_engine.py <path_to_libflutter_engine.so>")
        sys.exit(1)
    patch_engine(sys.argv[1])
