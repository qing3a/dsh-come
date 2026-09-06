"""核对 exe 内嵌图标 vs resources/icon.ico 是否一致（PE 资源解析，无第三方依赖）。"""
import struct, hashlib, sys, os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ICO = os.path.join(ROOT, "resources", "icon.ico")
EXE = os.path.join(ROOT, "dist", "dsh-come.exe")


def parse_ico(data):
    """返回 [(w,h,bpp,size,sha1_of_entry_bytes)]"""
    if len(data) < 6:
        return []
    res, typ, cnt = struct.unpack_from("<HHH", data, 0)
    if res != 0 or typ != 1:
        return []
    out = []
    for i in range(cnt):
        off = 6 + i * 16
        if off + 16 > len(data):
            break
        w, h, cc, rsv, planes, bpp, size, img_off = struct.unpack_from("<BBBBHHII", data, off)
        w = 256 if w == 0 else w
        h = 256 if h == 0 else h
        blob = data[img_off:img_off + size]
        out.append((w, h, bpp, size, hashlib.sha1(blob).hexdigest()[:12], blob))
    return out


def rva_to_off(sections, rva):
    for va, vsz, praw, rawsz in sections:
        if va <= rva < va + max(vsz, rawsz):
            return praw + (rva - va)
    return None


def parse_pe_resources(data):
    """提取 RT_ICON(3) / RT_GROUP_ICON(14) 下所有条目的原始数据。

    PE 规范要点：资源目录里的 OffsetToData（无论指向子目录还是数据条目）
    都是**相对资源目录基址**的偏移，不是相对当前节点——这正是易错点。
    """
    e_lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    coff = e_lfanew + 4
    nsec = struct.unpack_from("<H", data, coff + 2)[0]
    opt_size = struct.unpack_from("<H", data, coff + 16)[0]
    opt = coff + 20
    magic = struct.unpack_from("<H", data, opt)[0]
    # DataDirectory 偏移：PE32 = opt+96，PE32+ = opt+112
    dd = opt + (112 if magic == 0x20B else 96)
    # 资源表是第 3 个数据目录（index 2），每项 8 字节
    res_rva, _res_size = struct.unpack_from("<II", data, dd + 2 * 8)

    sec_off = opt + opt_size
    sections = []
    for i in range(nsec):
        p = sec_off + i * 40
        vsz, va, rawsz, praw = struct.unpack_from("<IIII", data, p + 8)
        sections.append((va, vsz, praw, rawsz))
    base = rva_to_off(sections, res_rva)
    if base is None:
        return [], []

    icons, groups = [], []

    def walk(node_off, path, depth=0):
        if depth > 8:
            return
        named, idc = struct.unpack_from("<HH", data, node_off + 12)
        for i in range(named + idc):
            e = node_off + 16 + i * 8
            name_or_id = struct.unpack_from("<I", data, e)[0]
            off_sub = struct.unpack_from("<I", data, e + 4)[0]
            target = base + (off_sub & 0x7FFFFFFF)
            if off_sub & 0x80000000:  # 子目录
                walk(target, path + [name_or_id], depth + 1)
                continue
            # IMAGE_RESOURCE_DATA_ENTRY: OffsetToData(RVA) Size CodePage Reserved
            data_rva, size = struct.unpack_from("<II", data, target)
            doff = rva_to_off(sections, data_rva)
            if doff is None:
                continue
            blob = data[doff:doff + size]
            if path and path[0] == 3:  # RT_ICON
                icons.append((name_or_id, blob))
            elif path and path[0] == 14:  # RT_GROUP_ICON
                groups.append((name_or_id, blob))

    walk(base, [])
    return icons, groups


def fmt_blob_kind(b):
    return "PNG" if b[:8] == b"\x89PNG\r\n\x1a\n" else ("BMP(DIB)" if len(b) >= 4 else "?")


def main():
    ico = open(ICO, "rb").read()
    ico_entries = parse_ico(ico)
    print(f"=== resources/icon.ico  ({len(ico)} bytes) ===")
    for w, h, bpp, size, sha, blob in ico_entries:
        print(f"  {w}x{h:<4} bpp={bpp:<3} size={size:<8} sha1={sha}  {fmt_blob_kind(blob)}")

    if not os.path.exists(EXE):
        print("exe 不存在:", EXE)
        return
    exe = open(EXE, "rb").read()
    icons, groups = parse_pe_resources(exe)
    print(f"\n=== dist/dsh-come.exe 内嵌资源 ({len(exe)} bytes) ===")
    print(f"  RT_GROUP_ICON 组数: {len(groups)}")
    for gid, g in groups:
        res, typ, cnt = struct.unpack_from("<HHH", g, 0)
        print(f"  组 id={gid}: {cnt} 个条目")
        for i in range(cnt):
            p = 6 + i * 14
            w, h, cc, rsv, planes, bpp, size, nid = struct.unpack_from("<BBBBHHHI", g, p)
            w = 256 if w == 0 else w
            h = 256 if h == 0 else h
            print(f"     {w}x{h:<4} bpp={bpp:<3} size={size:<8} -> RT_ICON id={nid}")
    print(f"  RT_ICON 条目数: {len(icons)}")
    exe_shas = {}
    for nid, b in icons:
        s = hashlib.sha1(b).hexdigest()[:12]
        exe_shas.setdefault(s, []).append(nid)
        print(f"    id={nid:<4} {len(b):<8} bytes sha1={s}  {fmt_blob_kind(b)}")

    ico_shas = {sha for _, _, _, _, sha, _ in ico_entries}
    print("\n=== 一致性 ===")
    missing = [s for s in ico_shas if s not in exe_shas]
    extra = [s for s in exe_shas if s not in ico_shas]
    if not missing and not extra:
        print("  [OK] exe 内嵌图标与 resources/icon.ico 完全一致")
    else:
        if missing:
            print("  [差异] icon.ico 中有、exe 中没有:", missing)
        if extra:
            print("  [差异] exe 中有、icon.ico 中没有:", extra)


if __name__ == "__main__":
    main()
