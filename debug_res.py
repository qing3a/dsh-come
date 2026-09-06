import struct

def debug_resources(exe_path):
    with open(exe_path, 'rb') as f:
        data = f.read()

    e_lfanew = struct.unpack_from('<I', data, 0x3C)[0]
    coff_offset = e_lfanew + 4
    num_sections = struct.unpack_from('<H', data, coff_offset + 2)[0]
    opt_header_size = struct.unpack_from('<H', data, coff_offset + 16)[0]
    opt_offset = coff_offset + 20
    magic = struct.unpack_from('<H', data, opt_offset)[0]
    
    if magic == 0x10b:
        dd_offset = opt_offset + 96
    else:
        dd_offset = opt_offset + 112
    
    resource_rva = struct.unpack_from('<I', data, dd_offset + 2*8)[0]
    
    section_offset = opt_offset + opt_header_size
    sections = []
    for i in range(num_sections):
        sec_off = section_offset + i * 40
        vaddr = struct.unpack_from('<I', data, sec_off + 12)[0]
        raw_ptr = struct.unpack_from('<I', data, sec_off + 20)[0]
        sections.append({'vaddr': vaddr, 'raw_ptr': raw_ptr})
    
    def rva_to_offset(rva):
        for sec in sections:
            if sec['vaddr'] <= rva < sec['vaddr'] + 0x100000:
                return sec['raw_ptr'] + (rva - sec['vaddr'])
        return None
    
    resource_base = rva_to_offset(resource_rva)
    print(f"Resource base offset: {resource_base:#x}")
    
    def dump_dir(offset, level=0):
        indent = "  " * level
        num_named = struct.unpack_from('<H', data, offset + 12)[0]
        num_id = struct.unpack_from('<H', data, offset + 14)[0]
        total = num_named + num_id
        print(f"{indent}Directory at {offset:#x}: {num_named} named, {num_id} ID entries")
        for i in range(total):
            entry_off = offset + 16 + i * 8
            name_or_id = struct.unpack_from('<I', data, entry_off)[0]
            data_or_subdir = struct.unpack_from('<I', data, entry_off + 4)[0]
            
            if name_or_id & 0x80000000:
                name_offset = resource_base + (name_or_id & 0x7FFFFFFF)
                name_len = struct.unpack_from('<H', data, name_offset)[0]
                name = data[name_offset+2:name_offset+2+name_len*2].decode('utf-16-le', errors='replace')
                key_str = f'"{name}"'
            else:
                key_str = str(name_or_id)
            
            if data_or_subdir & 0x80000000:
                subdir_offset = resource_base + (data_or_subdir & 0x7FFFFFFF)
                print(f"{indent}  [{key_str}] -> subdir at {subdir_offset:#x}")
                dump_dir(subdir_offset, level + 2)
            else:
                data_entry_offset = resource_base + data_or_subdir
                data_rva = struct.unpack_from('<I', data, data_entry_offset)[0]
                data_size = struct.unpack_from('<I', data, data_entry_offset + 4)[0]
                print(f"{indent}  [{key_str}] -> data: RVA={data_rva:#x}, size={data_size}")
    
    dump_dir(resource_base)

debug_resources(r"C:\Users\Administrator\Desktop\dsh-come\dist\dsh-come.exe")
