import sys
import os

def patch_engine_null(file_path):
    if not os.path.exists(file_path):
        print(f"Error: File not found: {file_path}")
        sys.exit(1)
        
    target1 = b'837be28dd2' + b'\x00' * 30
    target2 = b'837be28dd21818d2169e1e6789e89e771a55f01d'
    replacement = b'0000000000' + b'\x00' * 30
    
    with open(file_path, "rb") as f:
        data = f.read()
        
    if target1 in data:
        print(f"Found patched SDK hash in {file_path}")
        patched_data = data.replace(target1, replacement)
    elif target2 in data:
        print(f"Found original SDK hash in {file_path}")
        patched_data = data.replace(target2, replacement)
    else:
        print("Neither original nor patched SDK hash found. Maybe already set to null?")
        return
        
    with open(file_path, "wb") as f:
        f.write(patched_data)
        
    print(f"Successfully patched {file_path} to use null SDK hash!")

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python3 patch_engine_null.py <path_to_libflutter_engine.so>")
        sys.exit(1)
    patch_engine_null(sys.argv[1])
