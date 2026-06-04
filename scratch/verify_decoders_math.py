def decode_signed_pristine(data, offset):
    b1 = data[offset]
    offset += 1
    if b1 >= 128:
        return b1 - 192, offset
    
    b2 = data[offset]
    offset += 1
    if b2 >= 128:
        return (b2 << 7) + b1 - 24576, offset
        
    b3 = data[offset]
    offset += 1
    if b3 >= 128:
        return (b3 << 14) + (b2 << 7) + b1 - 3145728, offset
        
    b4 = data[offset]
    offset += 1
    if b4 >= 128:
        return (b4 << 21) + (b3 << 14) + (b2 << 7) + b1 - 402653184, offset
        
    b5 = data[offset]
    offset += 1
    # Note: in pristine, there is no subtraction for 5-byte case because it fits in 32-bit unsigned/signed without offset bias
    val = (b5 << 28) + (b4 << 21) + (b3 << 14) + (b2 << 7) + b1
    # Sign extend 32-bit to 64-bit:
    if val & 0x80000000:
        val -= 0x100000000
    return val, offset

def decode_unsigned_pristine(data, offset):
    b1 = data[offset]
    offset += 1
    if b1 >= 128:
        return b1 - 128, offset
        
    rdi = b1
    shift = 7
    while True:
        b = data[offset]
        offset += 1
        if b >= 128:
            val = ((b - 128) << shift) | rdi
            # Sign extend 32-bit to 64-bit (since cdqe is used at the end of .u_byteX_term):
            if val & 0x80000000:
                val -= 0x100000000
            return val, offset
        else:
            rdi = (b << shift) | rdi
            shift += 7

# Our assembly-equivalent python models:
def decode_signed_ours(data, offset):
    # Model of our assembly decode_signed
    b1 = data[offset]
    offset += 1
    if b1 >= 128:
        val = b1 - 192
        return val, offset
        
    b2 = data[offset]
    offset += 1
    if b2 >= 128:
        val = (b2 << 7) | b1  # wait, or/add are equivalent since bits don't overlap
        val -= 24576
        return val, offset
        
    b3 = data[offset]
    offset += 1
    if b3 >= 128:
        val = (b3 << 14) | (b2 << 7) | b1
        val -= 3145728
        return val, offset
        
    b4 = data[offset]
    offset += 1
    if b4 >= 128:
        val = (b4 << 21) | (b3 << 14) | (b2 << 7) | b1
        val -= 402653184
        return val, offset
        
    b5 = data[offset]
    offset += 1
    val = (b5 << 28) | (b4 << 21) | (b3 << 14) | (b2 << 7) | b1
    # sign-extend 32-bit:
    if val & 0x80000000:
        val -= 0x100000000
    return val, offset

def decode_unsigned_ours(data, offset):
    # Model of our assembly decode_unsigned
    b1 = data[offset]
    offset += 1
    if b1 >= 128:
        val = b1 - 128
        return val, offset
        
    b2 = data[offset]
    offset += 1
    if b2 >= 128:
        val = (b2 << 7) | b1
        val -= 16384
        return val, offset
        
    b3 = data[offset]
    offset += 1
    if b3 >= 128:
        val = (b3 << 14) | (b2 << 7) | b1
        val -= 2097152
        return val, offset
        
    b4 = data[offset]
    offset += 1
    if b4 >= 128:
        val = (b4 << 21) | (b3 << 14) | (b2 << 7) | b1
        val -= 268435456
        return val, offset
        
    b5 = data[offset]
    offset += 1
    val = (b5 << 28) | (b4 << 21) | (b3 << 14) | (b2 << 7) | b1
    if val & 0x80000000:
        val -= 0x100000000
    return val, offset

def test_all():
    print("Testing signed decoders...")
    # Test a wide range of byte sequences
    # Let's generate sequences of length 1 to 5
    import itertools
    
    # We can check subset of values to keep it fast
    test_bytes = [0, 1, 2, 63, 64, 127, 128, 129, 191, 192, 255]
    
    # 1-byte
    for b1 in test_bytes:
        if b1 >= 128:
            data = bytes([b1])
            v1, o1 = decode_signed_pristine(data, 0)
            v2, o2 = decode_signed_ours(data, 0)
            assert (v1, o1) == (v2, o2), f"1-byte mismatch: {b1} -> {(v1, o1)} vs {(v2, o2)}"
            
    # 2-byte
    for b1, b2 in itertools.product(test_bytes, test_bytes):
        if b1 < 128 and b2 >= 128:
            data = bytes([b1, b2])
            v1, o1 = decode_signed_pristine(data, 0)
            v2, o2 = decode_signed_ours(data, 0)
            assert (v1, o1) == (v2, o2), f"2-byte mismatch: {b1},{b2} -> {(v1, o1)} vs {(v2, o2)}"
            
    # 3-byte
    for b1, b2, b3 in itertools.product(test_bytes, test_bytes, test_bytes):
        if b1 < 128 and b2 < 128 and b3 >= 128:
            data = bytes([b1, b2, b3])
            v1, o1 = decode_signed_pristine(data, 0)
            v2, o2 = decode_signed_ours(data, 0)
            assert (v1, o1) == (v2, o2), f"3-byte mismatch: {b1},{b2},{b3} -> {(v1, o1)} vs {(v2, o2)}"

    # 4-byte
    for b1, b2, b3, b4 in itertools.product(test_bytes, test_bytes, test_bytes, test_bytes):
        if b1 < 128 and b2 < 128 and b3 < 128 and b4 >= 128:
            data = bytes([b1, b2, b3, b4])
            v1, o1 = decode_signed_pristine(data, 0)
            v2, o2 = decode_signed_ours(data, 0)
            assert (v1, o1) == (v2, o2), f"4-byte mismatch: {b1},{b2},{b3},{b4} -> {(v1, o1)} vs {(v2, o2)}"

    print("Signed decoders OK!")
    
    print("Testing unsigned decoders...")
    # 1-byte
    for b1 in test_bytes:
        if b1 >= 128:
            data = bytes([b1])
            v1, o1 = decode_unsigned_pristine(data, 0)
            v2, o2 = decode_unsigned_ours(data, 0)
            assert (v1, o1) == (v2, o2), f"1-byte mismatch: {b1} -> {(v1, o1)} vs {(v2, o2)}"
            
    # 2-byte
    for b1, b2 in itertools.product(test_bytes, test_bytes):
        if b1 < 128 and b2 >= 128:
            data = bytes([b1, b2])
            v1, o1 = decode_unsigned_pristine(data, 0)
            v2, o2 = decode_unsigned_ours(data, 0)
            assert (v1, o1) == (v2, o2), f"2-byte mismatch: {b1},{b2} -> {(v1, o1)} vs {(v2, o2)}"
            
    # 3-byte
    for b1, b2, b3 in itertools.product(test_bytes, test_bytes, test_bytes):
        if b1 < 128 and b2 < 128 and b3 >= 128:
            data = bytes([b1, b2, b3])
            v1, o1 = decode_unsigned_pristine(data, 0)
            v2, o2 = decode_unsigned_ours(data, 0)
            assert (v1, o1) == (v2, o2), f"3-byte mismatch: {b1},{b2},{b3} -> {(v1, o1)} vs {(v2, o2)}"

    print("Unsigned decoders OK!")
    print("ALL TESTS PASSED!")

if __name__ == "__main__":
    test_all()
