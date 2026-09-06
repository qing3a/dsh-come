import struct

def extract_icons_from_pe(exe_path, out_ico_path):
    with open(exe_path, 'rb') as f:
        data = f.read()

    e_lfanew = struct.unpack_from('<I', data, 0x3C)[0]
    coff_offset = e_lfanew + 4
    num_sections = struct.unpack_from('<H', data, coff_offset + 2)[0]
    opt_header_size = struct.unpack_from('<H', data, coff_offset + 16)[0]
    opt_offset = coff_offset + 20
    magic = struct.unpack_from('<H', data, opt_offset)[0]
    
    dd_offset = opt_offset + (96 if magic == 0x10b else 112)
    resource_rva = struct.unpack_from('<I', data, dd_offset + 2*8)[0]
    
    section_offset = opt_offset + opt_header_size
    sections = []
    for i in range(num_sections):
        sec_off = section_offset + i * 40
        vaddr = struct.unpack_from('<I', data, sec_off + 12)[0]
        vsize = struct.unpack_from('<I', data, sec_off + 8)[0]
        raw_ptr = struct.unpack_from('<I', data, sec_off + 20)[0]
        raw_size = struct.unpack_from('<I', data, sec_off + 16)[0]
        sections.append({'vaddr': vaddr, 'vsize': vsize, 'raw_ptr': raw_ptr, 'raw_size': raw_size})
    
    def rva_to_offset(rva):
        for sec in sections:
            sec_end = sec['vaddr'] + max(sec['vsize'], sec['raw_size'])
            if sec['vaddr'] <= rva < sec_end:
                return sec['raw_ptr'] + (rva - sec['vaddr'])
        return None
    
    resource_base = rva_to_offset(resource_rva)
    print(f"Resource base: {resource_base:#x}")
    
    def read_dir(offset):
        num_named = struct.unpack_from('<H', data, offset + 12)[0]
        num_id = struct.unpack_from('<H', data, offset + 14)[0]
        total = num_named + num_id
        entries = {}
        for i in range(total):
            entry_off = offset + 16 + i * 8
            name_or_id = struct.unpack_from('<I', data, entry_off)[0]
            data_or_subdir = struct.unpack_from('<I', data, entry_off + 4)[0]
            
            if name_or_id & 0x80000000:
                name_offset = resource_base + (name_or_id & 0x7FFFFFFF)
                name_len = struct.unpack_from('<H', data, name_offset)[0]
                key = data[name_offset+2:name_offset+2+name_len*2].decode('utf-16-le', errors='replace')
            else:
                key = name_or_id
            
            if data_or_subdir & 0x80000000:
                subdir_offset = resource_base + (data_or_subdir & 0x7FFFFFFF)
                entries[key] = ('dir', read_dir(subdir_offset))
            else:
                data_entry_offset = resource_base + data_or_subdir
                data_rva = struct.unpack_from('<I', data, data_entry_offset)[0]
                data_size = struct.unpack_from('<I', data, data_entry_offset + 4)[0]
                data_off = rva_to_offset(data_rva)
                entries[key] = ('data', data_off, data_size)
        return entries
    
    res_tree = read_dir(resource_base)
    print(f"Top-level resource types: {list(res_tree.keys())}")
    
    RT_ICON = 3
    RT_GROUP_ICON = 14
    
    # Find group icon data (traverse type -> id -> language)
    def find_first_data(node):
        if node[0] == 'data':
            return node[1], node[2]
        if node[0] == 'dir':
            for k, v in node[1].items():
                result = find_first_data(v)
                if result:
                    return result
        return None
    
    group_data_info = find_first_data(res_tree[RT_GROUP_ICON])
    group_data_off, group_data_size = group_data_info
    group_data = data[group_data_off:group_data_off+group_data_size]
    
    reserved, icon_type, count = struct.unpack_from('<HHH', group_data, 0)
    print(f"\nIcon group: {count} icons, type={icon_type}")
    
    # Collect all RT_ICON data by their ID (middle level key)
    icon_resources = {}
    if RT_ICON in res_tree and res_tree[RT_ICON][0] == 'dir':
        for icon_id, icon_node in res_tree[RT_ICON][1].items():
            # icon_node is ('dir', {lang_id: ('data', off, size)})
            if icon_node[0] == 'dir':
                for lang_id, lang_node in icon_node[1].items():
                    if lang_node[0] == 'data':
                        d_off, d_size = lang_node[1], lang_node[2]
                        icon_resources[icon_id] = data[d_off:d_off+d_size]
                        print(f"  Found RT_ICON ID={icon_id}, lang={lang_id}, size={d_size}")
            elif icon_node[0] == 'data':
                d_off, d_size = icon_node[1], icon_node[2]
                icon_resources[icon_id] = data[d_off:d_off+d_size]
                print(f"  Found RT_ICON ID={icon_id} (direct data), size={d_size}")
    
    # Build ICO file
    ico_header = struct.pack('<HHH', 0, 1, count)
    icon_entries = b''
    icon_data_blobs = []
    offset = 6 + count * 16
    
    for i in range(count):
        entry_off = 6 + i * 14
        bWidth, bHeight, bColorCount, bReserved, wPlanes, wBitCount, dwBytesInRes, nID = struct.unpack_from('<BBBBHHIH', group_data, entry_off)
        
        w_display = bWidth if bWidth != 0 else 256
        h_display = bHeight if bHeight != 0 else 256
        print(f"  Icon {i}: {w_display}x{h_display}, {wBitCount}bpp, {dwBytesInRes} bytes, ID={nID}")
        
        if nID in icon_resources:
            icon_blob = icon_resources[nID]
            print(f"    -> Matched! blob size={len(icon_blob)}")
        else:
            print(f"    -> NOT FOUND in RT_ICON! Available IDs: {list(icon_resources.keys())}")
            continue
        
        icon_entries += struct.pack('<BBBBHHII', 
            bWidth, bHeight, bColorCount, bReserved, 
            wPlanes, wBitCount, len(icon_blob), offset)
        icon_data_blobs.append(icon_blob)
        offset += len(icon_blob)
    
    with open(out_ico_path, 'wb') as f:
        f.write(ico_header)
        f.write(icon_entries)
        for blob in icon_data_blobs:
            f.write(blob)
    
    print(f"\nSaved ICO to: {out_ico_path}")
    print(f"Total icons extracted: {len(icon_data_blobs)}")
    return out_ico_path

if __name__ == '__main__':
    exe = r"C:\Users\Administrator\Desktop\dsh-come\dist\dsh-come.exe"
    out = r"C:\Users\Administrator\Desktop\dsh-come\extracted_icon.ico"
    extract_icons_from_pe(exe, out)
