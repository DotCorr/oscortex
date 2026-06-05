def decode_sim(b):
    # Step 1
    rax = b[0]
    if rax >= 128:
        rax -= 256
    if b[0] & 0x80 or len(b) == 1:
        return rax, 1
    
    # Step 2
    rdx = b[1]
    if rdx >= 128:
        rdx -= 256
    rax = (rax << 7) + rdx
    rax = (rax + 2**63) % 2**64 - 2**63
    if b[1] & 0x80 or len(b) == 2:
        return rax, 2
        
    # Step 3
    rdx = b[2]
    if rdx >= 128:
        rdx -= 256
    rax = (rax << 7) + rdx
    rax = (rax + 2**63) % 2**64 - 2**63
    if b[2] & 0x80 or len(b) == 3:
        return rax, 3
        
    # Step 4
    rdx = b[3]
    if rdx >= 128:
        rdx -= 256
    rax = (rax << 7) + rdx
    rax = (rax + 2**63) % 2**64 - 2**63
    return rax, 4

# Search 1 to 4 bytes
found = []
import itertools

# Let's test if we can find it
# Since 4 bytes is 256^4 (4 billion), let's optimize the search.
# If 1 byte:
for b1 in range(256):
    val, length = decode_sim([b1])
    if (val & 0xffffffffffffffff) == 0x1400000:
        found.append(([b1], val))

# If 2 bytes:
for b1 in range(128): # B1 must not have high bit set
    for b2 in range(128, 256): # B2 must have high bit set
        val, length = decode_sim([b1, b2])
        if (val & 0xffffffffffffffff) == 0x1400000:
            found.append(([b1, b2], val))

# If 3 bytes:
for b1 in range(128):
    for b2 in range(128):
        # B3 must have high bit set
        # rax = (b1 << 14) + (b2 << 7) + b3_signed
        # We want rax = 0x1400000.
        # But max value of (b1<<14) + (b2<<7) is (127 << 14) + (127 << 7) = 2,080,768 + 16,256 = 2,097,024.
        # This is less than 20,971,520 (0x1400000). So 3 bytes cannot reach it unless there's negative overflow,
        # but negative overflow with 3 bytes is impossible since b1, b2 are positive.
        pass

# If 4 bytes:
# rax = (b1 << 21) + (b2 << 14) + (b3 << 7) + b4_signed
# We want this to be 20,971,520.
# Let B = (b2 << 14) + (b3 << 7) + b4_signed
# We want b1 * 2,097,152 + B = 20,971,520.
# Since b1 is an integer in [0, 127]:
# Let's try different b1 values:
for b1 in range(128):
    target_B = 20971520 - b1 * 2097152
    # B must be within the range of B2, B3, B4.
    # B2 in [0, 127], B3 in [0, 127], B4_signed in [-128, -1]
    # So B = (B2 << 14) + (B3 << 7) + B4_signed.
    # B is in range [-128, (127<<14) + (127<<7) - 1] = [-128, 2096895].
    # So target_B must be in this range.
    if -128 <= target_B <= 2096895:
        # We can find B2, B3, B4
        # target_B = (b2 << 14) + (b3 << 7) + b4_signed
        # Since b4_signed is in [-128, -1], let's write b4_signed = b4 - 256 (where b4 in [128, 255])
        # target_B = (b2 << 14) + (b3 << 7) + b4 - 256
        # target_B + 256 = (b2 << 14) + (b3 << 7) + b4
        # Since b4 is in [128, 255], and (b3 << 7) is a multiple of 128:
        # Let's check target_B + 256:
        val_to_decompose = target_B + 256
        b2 = val_to_decompose >> 14
        rem = val_to_decompose & 0x3fff
        b3 = rem >> 7
        b4 = rem & 0x7f
        # Wait, b4 must be in [128, 255], so if rem & 0x7f is b4, it doesn't have the high bit set.
        # But wait! If we add 128 to b4, we must subtract 1 from b3.
        # Let's do:
        b4 = (rem & 0x7f) + 128
        b3 = (rem >> 7) - 1
        if b3 < 0:
            b3 += 128
            b2 -= 1
        if 0 <= b2 < 128 and 0 <= b3 < 128 and 128 <= b4 < 256:
            found.append(([b1, b2, b3, b4], decode_sim([b1, b2, b3, b4])[0]))

for f in found:
    print(f"Bytes: {[hex(x) for x in f[0]]} -> Value: {hex(f[1] & 0xffffffffffffffff)}")
