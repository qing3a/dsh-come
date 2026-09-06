import struct

exe_path = r"C:\Users\Administrator\Desktop\dsh-come\dist\dsh-come.exe"
with open(exe_path, 'rb') as f:
    data = f.read()

print(f"File size: {len(data)} bytes")

# DOS header
e_lfanew = struct.unpack_from('<I', data, 0x3C)[0]
print(f"e_lfanew: {e_lfanew:#x} ({e_lfanew})")

# PE signature
pe_sig = struct.unpack_from('<I', data, e_lfanew)[0]
print(f"PE signature: {pe_sig:#x} (expected 0x4550)")

# COFF header
coff_offset = e_lfanew + 4
machine = struct.unpack_from('<H', data, coff_offset)[0]
num_sections = struct.unpack_from('<H', data, coff_offset + 2)[0]
opt_header_size = struct.unpack_from('<H', data, coff_offset + 16)[0]
print(f"Machine: {machine:#x} (0x8664=AMD64, 0x14c=i386)")
print(f"Num sections: {num_sections}")
print(f"Opt header size: {opt_header_size}")

# Optional header
opt_offset = coff_offset + 20
magic = struct.unpack_from('<H', data, opt_offset)[0]
print(f"Optional header magic: {magic:#x} (0x10b=PE32, 0x20b=PE32+)")

if magic == 0x10b:
    dd_offset = opt_offset + 96
    print("Using PE32 data directory offset: +96")
else:
    dd_offset = opt_offset + 112
    print("Using PE32+ data directory offset: +112")

# Read all data directories
print("\nData directories:")
for i in range(16):
    rva = struct.unpack_from('<I', data, dd_offset + i*8)[0]
    size = struct.unpack_from('<I', data, dd_offset + i*8 + 4)[0]
    if rva != 0 or size != 0:
        print(f"  [{i}] RVA={rva:#x}, size={size:#x}")

resource_rva = struct.unpack_from('<I', data, dd_offset + 2*8)[0]
resource_size = struct.unpack_from('<I', data, dd_offset + 2*8 + 4)[0]
print(f"\nResource directory: RVA={resource_rva:#x}, size={resource_size:#x}")

# Section headers
section_offset = opt_offset + opt_header_size
print(f"\nSection headers (at {section_offset:#x}):")
sections = []
for i in range(num_sections):
    sec_off = section_offset + i * 40
    name = data[sec_off:sec_off+8].rstrip(b'\x00').decode('ascii', errors='replace')
    vsize = struct.unpack_from('<I', data, sec_off + 8)[0]
    vaddr = struct.unpack_from('<I', data, sec_off + 12)[0]
    raw_size = struct.unpack_from('<I', data, sec_off + 16)[0]
    raw_ptr = struct.unpack_from('<I', data, sec_off + 20)[0]
    print(f"  {name}: vaddr={vaddr:#x} vsize={vsize:#x} raw_ptr={raw_ptr:#x} raw_size={raw_size:#x}")
    sections.append({'name': name, 'vaddr': vaddr, 'vsize': vsize, 'raw_ptr': raw_ptr, 'raw_size': raw_size})

def rva_to_offset(rva):
    for sec in sections:
        sec_end = sec['vaddr'] + max(sec['vsize'], sec['raw_size'])
        if sec['vaddr'] <= rva < sec_end:
            return sec['raw_ptr'] + (rva - sec['vaddr'])
    return None

resource_base = rva_to_offset(resource_rva)
print(f"\nResource base file offset: {resource_base:#x}" if resource_base else "\nResource base: NOT FOUND!")

if resource_base:
    # Check if it looks like a valid resource directory
    characteristics = struct.unpack_from('<I', data, resource_base)[0]
    timestamp = struct.unpack_from('<I', data, resource_base + 4)[0]
    major = struct.unpack_from('<H', data, resource_base + 8)[0]
    minor = struct.unpack_from('<H', data, resource_base + 10)[0]
    num_named = struct.unpack_from('<H', data, resource_base + 12)[0]
    num_id = struct.unpack_from('<H', data, resource_base + 14)[0]
    print(f"Resource dir: chars={characteristics:#x}, time={timestamp}, ver={major}.{minor}")
    print(f"  Named entries: {num_named}, ID entries: {num_id}")
    print(f"  Total entries: {num_named + num_id}")
