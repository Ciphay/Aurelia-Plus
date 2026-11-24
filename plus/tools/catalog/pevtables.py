import json
import re
import struct
import sys
from pathlib import Path

class Pe:
    def __init__(self, path):
        self.data = Path(path).read_bytes()
        d = self.data
        pe = struct.unpack_from("<I", d, 0x3C)[0]
        nsec = struct.unpack_from("<H", d, pe + 6)[0]
        opt = struct.unpack_from("<H", d, pe + 20)[0]
        self.base = struct.unpack_from("<I", d, pe + 24 + 28)[0]
        self.sections = []
        first = pe + 24 + opt
        for i in range(nsec):
            at = first + i * 40
            vsize, va, rsize, rptr = struct.unpack_from("<IIII", d, at + 8)
            flags = struct.unpack_from("<I", d, at + 36)[0]
            self.sections.append((self.base + va, vsize, rptr, rsize, flags))

    def offset(self, va):
        for sva, vsize, rptr, rsize, _ in self.sections:
            if sva <= va < sva + min(vsize, rsize):
                return rptr + va - sva
        return None

    def in_text(self, va):
        return any(fl & 0x20000000 and sva <= va < sva + vsize for sva, vsize, _, _, fl in self.sections)

    def data_sections(self):
        return [(sva, self.data[rptr:rptr + rsize]) for sva, _, rptr, rsize, fl in self.sections if not fl & 0x20000000]

def ret_bytes(pe, va):
    off = pe.offset(va)
    code = pe.data[off:off + 160]
    for i, b in enumerate(code):
        if b == 0xC2 and i + 2 < len(code):
            return struct.unpack_from("<H", code, i + 1)[0]
        if b == 0xC3:
            return 0
    return None

def scan(path):
    pe = Pe(path)
    d = pe.data
    type_descriptors = {}
    for m in re.finditer(rb"\.\?AV(CAdapterSteam[A-Za-z0-9_]+)@@\0", d):
        for sva, _, rptr, rsize, _ in pe.sections:
            if rptr <= m.start() < rptr + rsize:
                type_descriptors[sva + m.start() - rptr - 8] = m.group(1).decode()
                break
    cols = {}
    for sva, blob in pe.data_sections():
        for td, name in type_descriptors.items():
            pat = struct.pack("<I", td)
            j = blob.find(pat)
            while j >= 0:
                if j >= 12 and j % 4 == 0 and struct.unpack_from("<I", blob, j - 12)[0] == 0:
                    cols.setdefault(name, []).append(sva + j - 12)
                j = blob.find(pat, j + 4)
    out = {}
    for sva, blob in pe.data_sections():
        for name, col_list in cols.items():
            for col in col_list:
                pat = struct.pack("<I", col)
                j = blob.find(pat)
                while j >= 0 and name not in out:
                    if j % 4 == 0:
                        vt, entries = sva + j + 4, []
                        while True:
                            off = pe.offset(vt + 4 * len(entries))
                            if off is None:
                                break
                            v = struct.unpack_from("<I", d, off)[0]
                            if not pe.in_text(v):
                                break
                            entries.append(v)
                        if entries:
                            out[name] = {"n": len(entries), "args": [ret_bytes(pe, e) for e in entries]}
                    j = blob.find(pat, j + 4)
    return out

def class_key(name):
    return re.sub(r"[^a-z0-9]", "", name.lower())

def interface_key(name):
    return class_key(re.sub(r"_INTERFACE_VERSION|_INTERFACE_V|_V(?=\d)", "", name))

if __name__ == "__main__":
    Path(sys.argv[2]).write_text(json.dumps(scan(sys.argv[1])))
