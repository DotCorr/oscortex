import struct
from collections import Counter
from pathlib import Path

def main():
    path = Path("./initramfs/system/flutter/libapp.so")
    data = path.read_bytes()
    
    c_values = []
    for i in range(0, len(data) - 4, 4):
        val = struct.unpack("<I", data[i:i+4])[0]
        if 0x2000_0000 <= val < 0x2800_0000:
            c = 0x1_0000_0000 + (val << 1)
            if (c & 0x7) in (6, 7):
                c_values.append(c & ~1)
                
    print(f"Total tag-3 pointers: {len(c_values)}")
    # Print the min and max value
    print(f"Min value: {hex(min(c_values))}")
    print(f"Max value: {hex(max(c_values))}")
    
    # Bucket the values in 1MB ranges
    buckets = Counter()
    for c in c_values:
        buckets[c >> 20] += 1
        
    print("Distribution of pointers by 1MB bucket:")
    for bucket, count in sorted(buckets.items()):
        print(f"  Range {hex(bucket << 20)} - {hex((bucket + 1) << 20)}: {count} pointers")

if __name__ == '__main__':
    main()
