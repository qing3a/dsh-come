import struct
import sys

def extract_icons_from_pe(exe_path, out_ico_path):
    with open(exe_path, 'rb') as f:
        data = f.read()

    # Parse DOS header
    e_lfanew = struct.unpack_from('<I', data, 0x3C)[0]
    
    # PE signature
    pe_sig = struct.unpack_from('<I', data, e_lfanew)[0]
    if pe_sig != 0x00004550:  # "PE\0\0"
        raise ValueError("Not a valid PE file")
    
    # COFF header
    coff_offset = e_lfanew + 4
    num_sections = struct.unpack_from('<H', data, coff_offset + 2)[0]
    opt_header_size = struct.unpack_from('<H', data, coff_offset + 16)[0]
    
    # Optional header
    opt_offset = coff_offset + 20
    magic = struct.unpack_from('<H', data, opt_offset)[0]
    
    # Data directory: resource is index 2 (3rd entry)
    if magic == 0x10b:  # PE32
        dd_offset = opt_offset + 96
    else:  # PE32+
        dd_offset = opt_offset + 112
    
    resource_rva = struct.unpack_from('<I', data, dd_offset + 2*8)[0]
    resource_size = struct.unpack_from('<I', data, dd_offset + 2*8 + 4)[0]
    
    # Section headers
    section_offset = opt_offset + opt_header_size
    sections = []
    for i in range(num_sections):
        sec_off = section_offset + i * 40
        name = data[sec_off:sec_off+8].rstrip(b'\x00').decode('ascii', errors='replace')
        vsize = struct.unpack_from('<I', data, sec_off + 8)[0]
        vaddr = struct.unpack_from('<I', data, sec_off + 12)[0]
        raw_size = struct.unpack_from('<I', data, sec_off + 16)[0]
        raw_ptr = struct.unpack_from('<I', data, sec_off + 20)[0]
        sections.append({'name': name, 'vaddr': vaddr, 'vsize': vsize, 'raw_ptr': raw_ptr, 'raw_size': raw_size})
    
    def rva_to_offset(rva):
        for sec in sections:
            if sec['vaddr'] <= rva < sec['vaddr'] + max(sec['vsize'], sec['raw_size']):
                return sec['raw_ptr'] + (rva - sec['vaddr'])
        return None
    
    resource_base = rva_to_offset(resource_rva)
    if resource_base is None:
        raise ValueError("Cannot find resource section")
    
    def read_resource_directory(offset, level=0):
        """Parse a resource directory, return dict of entries."""
        num_named = struct.unpack_from('<H', data, offset + 12)[0]
        num_id = struct.unpack_from('<H', data, offset + 14)[0]
        total = num_named + num_id
        entries = {}
        for i in range(total):
            entry_off = offset + 16 + i * 8
            name_or_id = struct.unpack_from('<I', data, entry_off)[0]
            data_or_subdir = struct.unpack_from('<I', data, entry_off + 4)[0]
            
            if name_or_id & 0x80000000:
                # Named entry
                name_offset = resource_base + (name_or_id & 0x7FFFFFFF)
                name_len = struct.unpack_from('<H', data, name_offset)[0]
                name = data[name_offset+2:name_offset+2+name_len*2].decode('utf-16-le', errors='replace')
                key = name
            else:
                key = name_or_id
            
            if data_or_subdir & 0x80000000:
                # Subdirectory
                subdir_offset = resource_base + (data_or_subdir & 0x7FFFFFFF)
                entries[key] = ('dir', read_resource_directory(subdir_offset, level+1))
            else:
                # Data entry
                data_entry_offset = resource_base + data_or_subdir
                data_rva = struct.unpack_from('<I', data, data_entry_offset)[0]
                data_size = struct.unpack_from('<I', data, data_entry_offset + 4)[0]
                data_off = rva_to_offset(data_rva)
                entries[key] = ('data', data_off, data_size)
        return entries
    
    # Parse full resource tree
    res_tree = read_resource_directory(resource_base)
    
    # RT_ICON = 3, RT_GROUP_ICON = 14
    RT_ICON = 3
    RT_GROUP_ICON = 14
    
    if RT_GROUP_ICON not in res_tree:
        raise ValueError("No GROUP_ICON resource found")
    
    def get_first_data(res_dict):
        """Traverse nested resource dirs to find first data entry."""
        for key, val in res_dict.items():
            if val[0] == 'data':
                return val[1], val[2]
            elif val[0] == 'dir':
                result = get_first_data(val[1])
                if result:
                    return result
        return None

    def get_all_data(res_dict, result_dict=None):
        """Traverse nested resource dirs to collect all data entries by their ID."""
        if result_dict is None:
            result_dict = {}
        for key, val in res_dict.items():
            if val[0] == 'data':
                result_dict[key] = (val[1], val[2])
            elif val[0] == 'dir':
                get_all_data(val[1], result_dict)
        return result_dict

    group_data_info = get_first_data(res_tree[RT_GROUP_ICON][1])
    if group_data_info is None:
        raise ValueError("No GROUP_ICON data found")
    group_data_off, group_data_size = group_data_info
    group_data = data[group_data_off:group_data_off+group_data_size]
    
    # Parse GRPICONDIR
    # WORD reserved, WORD type (1=icon), WORD count
    reserved, icon_type, count = struct.unpack_from('<HHH', group_data, 0)
    print(f"Icon group: {count} icons, type={icon_type}")
    
    # Get RT_ICON resources (all IDs across language subdirs)
    icon_resources = {}
    if RT_ICON in res_tree:
        icon_data_map = get_all_data(res_tree[RT_ICON][1])
        for icon_id, (data_off, data_size) in icon_data_map.items():
            icon_resources[icon_id] = data[data_off:data_off+data_size]
    
    # Build ICO file
    ico_header = struct.pack('<HHH', 0, 1, count)
    icon_entries = b''
    icon_data_blobs = []
    
    offset = 6 + count * 16  # header + directory entries
    
    for i in range(count):
        entry_off = 6 + i * 14  # GRPICONDIRENTRY is 14 bytes
        bWidth, bHeight, bColorCount, bReserved, wPlanes, wBitCount, dwBytesInRes, nID = struct.unpack_from('<BBBBHHIH', group_data, entry_off)
        
        print(f"  Icon {i}: {bWidth}x{bHeight}, {wBitCount}bpp, {dwBytesInRes} bytes, ID={nID}")
        
        if nID in icon_resources:
            icon_blob = icon_resources[nID]
        else:
            print(f"    WARNING: Icon ID {nID} not found in RT_ICON resources!")
            continue
        
        # ICONDIRENTRY in ICO file is 16 bytes
        icon_entries += struct.pack('<BBBBHHII', 
            bWidth, bHeight, bColorCount, bReserved, 
            wPlanes, wBitCount, len(icon_blob), offset)
        
        icon_data_blobs.append(icon_blob)
        offset += len(icon_blob)
    
    # Write ICO file
    with open(out_ico_path, 'wb') as f:
        f.write(ico_header)
        f.write(icon_entries)
        for blob in icon_data_blobs:
            f.write(blob)
    
    print(f"\nSaved ICO to: {out_ico_path}")
    print(f"Total icons: {len(icon_data_blobs)}")
    return out_ico_path

if __name__ == '__main__':
    exe = r"C:\Users\Administrator\Desktop\dsh-come\dist\dsh-come.exe"
    out = r"C:\Users\Administrator\Desktop\dsh-come\extracted_icon.ico"
    extract_icons_from_pe(exe, out)
