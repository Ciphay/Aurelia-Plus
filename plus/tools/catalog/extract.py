import argparse
import hashlib
import json
import re
import struct
import subprocess
from pathlib import Path

SYMBOL = re.compile(r"^(I\w+?)_((?:Steam|STEAM)[A-Za-z_]*?\d{3})_(\w+)$")
HEADER = re.compile(r"^[0-9a-f]+ <(\w+)>:$")
CALL = re.compile(r"call\s+\*(0x[0-9a-f]+)?\(%r[a-z0-9]+\)")
VERSION_NAME = re.compile(rb"\x00(Steam[A-Za-z]+[0-9]{3}|STEAM[A-Z_]+INTERFACE_VERSION_?[0-9]{3}|STEAM[A-Z]+_INTERFACE_V[0-9]{3})(?=\x00)")

def parse_symbol(name):
    m = SYMBOL.match(name)
    return m.groups() if m else None

LOAD64 = re.compile(r"mov\s+(0x[0-9a-f]+)?\(%r[a-z0-9]+\),(%r[a-z0-9]+)$")
CALL_REG64 = re.compile(r"call\s+\*(%r[a-z0-9]+)$")

def parse_thunk_slots(disassembly):
    slots, cur = {}, None
    loaded = {}
    for line in disassembly.splitlines():
        h = HEADER.match(line)
        if h:
            cur = h.group(1) if parse_symbol(h.group(1)) else None
            loaded = {}
            if cur:
                slots[cur] = None
            continue
        if cur and slots[cur] is None:
            ins = line.split("\t", 1)[-1].strip()
            c = CALL.search(line)
            if c:
                slots[cur] = int(c.group(1), 16) // 8 if c.group(1) else 0
                continue
            ld = LOAD64.search(ins)
            if ld:
                loaded[ld.group(2)] = int(ld.group(1), 16) // 8 if ld.group(1) else 0
                continue
            cr = CALL_REG64.search(ins)
            if cr and cr.group(1) in loaded:
                slots[cur] = loaded[cr.group(1)]
    return slots

CALL32 = re.compile(r"call\s+\*((0x[0-9a-f]+)?\(%e[a-z]{2}\)|%e[a-z]{2}\b)")
CALL_ANY = re.compile(r"\bcall\s")
PUSH = re.compile(r"push\s+(\S+)")
PROLOGUE_REGS = {"%ebp", "%ebx", "%esi", "%edi"}
PIC_THUNK = "__x86.get_pc_thunk"

ADJUST_ESP = re.compile(r"add\s+\$0x[0-9a-f]+,%esp")

def parse_thunk_argbytes_i386(disassembly):
    out, cur = {}, None
    pushes, in_prologue = 0, True
    for line in disassembly.splitlines():
        h = HEADER.match(line)
        if h:
            cur = h.group(1) if parse_symbol(h.group(1)) else None
            if cur:
                out[cur] = None
            pushes, in_prologue = 0, True
            continue
        if not cur or out[cur] is not None:
            continue
        ins = line.split("\t", 1)[-1].strip()
        push = PUSH.match(ins)
        if in_prologue and ((push and push.group(1) in PROLOGUE_REGS) or ins.startswith("mov    %esp,%ebp")):
            continue
        in_prologue = False
        if CALL32.search(ins):
            out[cur] = (pushes - 1) * 4 if pushes >= 1 else None
            cur = None
        elif CALL_ANY.search(ins) or ADJUST_ESP.search(ins):
            pushes = 0
        elif push:
            pushes += 1
    return out

STORE_AL = re.compile(r"mov\s+%al,")
STORE_EAX = re.compile(r"mov\s+%eax,")
STORE_EDX = re.compile(r"mov\s+%edx,")
LOAD_LOCAL_EAX = re.compile(r"mov\s+-0x[0-9a-f]+\(%ebp\),%eax")

def _ret_kind(after):
    if any(LOAD_LOCAL_EAX.search(i) for i in after[:2]):
        return "struct8"
    if any(STORE_AL.search(i) for i in after[:2]):
        return "bool"
    if any(STORE_EAX.search(i) for i in after[:2]):
        return "qword" if any(STORE_EDX.search(i) for i in after) else "word"
    return "void"

def parse_thunk_ret_kinds_i386(disassembly):
    out, cur, after = {}, None, None

    def close():
        if cur and after is not None and out.get(cur) is None:
            out[cur] = _ret_kind(after)

    for line in disassembly.splitlines():
        h = HEADER.match(line)
        if h:
            close()
            cur = h.group(1) if parse_symbol(h.group(1)) else None
            after = None
            if cur:
                out[cur] = None
            continue
        if not cur or out[cur] is not None:
            continue
        ins = line.split("\t", 1)[-1].strip()
        if after is None:
            if CALL32.search(ins):
                after = []
            continue
        after.append(ins)
        if len(after) == 4:
            close()
            after = None
            cur = None
    close()
    return out

NON_CALL_QWORDS = {
    "GetActionSetHandle", "GetAnalogActionHandle", "GetCurrentActionSet", "GetDigitalActionHandle", "GetControllerForGamepadIndex",
    "GetCachedUGCHandle", "FileWriteStreamOpen", "GetFileTimestamp", "GetLocalTimestamp", "GetConnectionUserData", "GetBeaconByIndex",
    "CreatePublishedFileUpdateRequest", "CreateQueryAllUGCRequest", "CreateQueryUGCDetailsRequest", "CreateQueryUserUGCRequest",
    "StartItemUpdate", "StartRangeTimelineEvent",
}

def add_ret_kinds(catalog, kinds):
    import copy
    out = copy.deepcopy(catalog)
    by = {}
    for sym, kind in kinds.items():
        parsed = parse_symbol(sym)
        if parsed and kind:
            if kind == "qword" and parsed[2].split("_")[0] not in NON_CALL_QWORDS:
                kind = "call"
            by[(parsed[1], parsed[2])] = kind
    for i in out["interfaces"]:
        for s in i["slots"] or []:
            k = by.get((i["name"], s["name"]))
            if k:
                s["ret"] = k
    return out

def add_argbytes(catalog, argbytes):
    import copy
    out = copy.deepcopy(catalog)
    by = {}
    for sym, n in argbytes.items():
        parsed = parse_symbol(sym)
        if parsed and n is not None:
            by[(parsed[1], parsed[2])] = n
    for i in out["interfaces"]:
        for s in i["slots"] or []:
            n = by.get((i["name"], s["name"]))
            if n is not None:
                s["argbytes32"] = n
    return out

PE_VTABLE = re.compile(r"^_?win(I\w+?)_((?:Steam|STEAM)[A-Za-z_]*?\d{3})_vtable$")
PE_METHOD_PREFIX = "_win"
RET_IMM = re.compile(r"\bret\s+\$(0x[0-9a-f]+)")
RET_PLAIN = re.compile(r"\bret\s*$")
PE_HEADER = re.compile(r"^([0-9a-f]+) <(\w+)>:$")

def parse_pe_vtables(nm_text, disassembly, read_u32s):
    vtables, funcs = {}, {}
    for line in nm_text.splitlines():
        parts = line.split()
        if len(parts) != 3 or parts[1] not in ("T", "D"):
            continue
        addr, kind, name = int(parts[0], 16), parts[1], parts[2]
        m = PE_VTABLE.match(name)
        if kind == "D" and m:
            vtables[m.group(2)] = (m.group(1), addr)
        elif kind == "T":
            funcs[addr] = name
    ret_bytes, cur, got = {}, None, False
    for line in disassembly.splitlines():
        h = PE_HEADER.match(line)
        if h:
            cur, got = h.group(2), False
            continue
        if cur and not got:
            ins = line.split("\t", 1)[-1].strip()
            r = RET_IMM.search(ins)
            if r:
                ret_bytes[cur], got = int(r.group(1), 16), True
            elif RET_PLAIN.search(ins):
                ret_bytes[cur], got = 0, True
    out = {}
    for version, (typ, addr) in vtables.items():
        prefix = f"{PE_METHOD_PREFIX}{typ}_{version}_"
        methods, n = [], 0
        while True:
            words = read_u32s(addr + 4 * n, 1)
            fn = funcs.get(words[0]) if words else None
            if not fn or not fn.startswith(prefix):
                break
            methods.append((fn[len(prefix):], ret_bytes.get(fn, 0)))
            n += 1
        out[version] = methods
    return out

def _pe_sections(path):
    text = subprocess.run(["objdump", "-h", str(path)], capture_output=True, text=True, check=True).stdout
    secs, lines = [], text.splitlines()
    for i, line in enumerate(lines):
        p = line.split()
        if len(p) >= 7 and p[0].isdigit() and p[1].startswith("."):
            secs.append((int(p[3], 16), int(p[2], 16), int(p[5], 16)))
    return secs

def extract_pe_vtables(pe, nm="i686-w64-mingw32-nm", objdump="i686-w64-mingw32-objdump"):
    nm_text = subprocess.run([nm, str(pe)], capture_output=True, text=True, check=True).stdout
    dis = subprocess.run([objdump, "-d", "--no-show-raw-insn", str(pe)], capture_output=True, text=True, check=True).stdout
    data = Path(pe).read_bytes()
    secs = _pe_sections(pe)

    def read_u32s(addr, n):
        for vma, size, off in secs:
            if vma <= addr < vma + size:
                o = off + addr - vma
                return list(struct.unpack_from("<%dI" % n, data, o))
        return []

    return parse_pe_vtables(nm_text, dis, read_u32s)

def dll_interface_names(dll):
    return sorted({m.group(1).decode() for m in VERSION_NAME.finditer(Path(dll).read_bytes())})

def build_catalog(names, method_slots, source):
    interfaces = []
    for name in sorted(names):
        methods = method_slots.get(name)
        if methods is None:
            interfaces.append({"name": name, "complete": False, "slots": None})
            continue
        vals = [v for v in methods.values() if v is not None]
        dup = {v for v in vals if vals.count(v) > 1}
        slots = [{"slot": None if (v is None or v in dup) else v, "name": m, "sig": None} for m, v in methods.items()]
        slots.sort(key=lambda s: (s["slot"] is None, s["slot"] if s["slot"] is not None else 0, s["name"]))
        got = [s["slot"] for s in slots]
        interfaces.append({"name": name, "complete": got == list(range(len(got))), "slots": slots})
    return {"schema": 1, "source": source, "interfaces": interfaces}

def build_catalog_from_pe(names, pe_vtables, source):
    interfaces = []
    for name in sorted(names):
        methods = pe_vtables.get(name)
        if not methods:
            interfaces.append({"name": name, "complete": False, "slots": None})
            continue
        slots = [{"slot": i, "name": m, "sig": None, "argbytes32": n} for i, (m, n) in enumerate(methods)]
        interfaces.append({"name": name, "complete": True, "slots": slots})
    return {"schema": 1, "source": source, "interfaces": interfaces}

def _sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()

def _dis(path, objdump):
    return subprocess.run([objdump, "-d", "--no-show-raw-insn", str(path)], capture_output=True, text=True, check=True).stdout

def add_extra_interfaces(cat, path=None):
    import json as _json
    path = Path(path) if path else Path(__file__).resolve().parent.parent.parent / "catalog" / "extra_interfaces.json"
    if not path.exists():
        return cat
    have = {i["name"] for i in cat["interfaces"]}
    for name, spec in _json.loads(path.read_text()).items():
        if name in have:
            continue
        slots = [{"argbytes32": s.get("argbytes32", 0), "name": s["name"], "ret": s["ret"], "sig": None, "slot": n} for n, s in enumerate(spec["slots"])]
        cat["interfaces"].append({"complete": spec.get("complete", True), "name": name, "slots": slots})
    cat["interfaces"].sort(key=lambda i: i["name"])
    return cat

def extract_catalog(dll, lsteamclient, objdump="objdump", lsteamclient32=None):
    dis = _dis(lsteamclient, objdump)
    by_version = {}
    for sym, slot in parse_thunk_slots(dis).items():
        _, version, method = parse_symbol(sym)
        by_version.setdefault(version, {})[method] = slot
    source = {"dll": Path(dll).name, "sha256": _sha256(dll), "lsteamclient": Path(lsteamclient).name, "lsteamclient_sha256": _sha256(lsteamclient)}
    if lsteamclient32:
        source["lsteamclient32"] = Path(lsteamclient32).name
        source["lsteamclient32_sha256"] = _sha256(lsteamclient32)
    cat = build_catalog(dll_interface_names(dll), by_version, source)
    add_extra_interfaces(cat)
    return add_argbytes(cat, parse_thunk_argbytes_i386(_dis(lsteamclient32, objdump))) if lsteamclient32 else cat

def main(argv=None):
    ap = argparse.ArgumentParser(prog="python -m tools.catalog.extract")
    ap.add_argument("--dll", required=True)
    ap.add_argument("--lsteamclient")
    ap.add_argument("--lsteamclient-pe", help="lsteamclient.dll i386 (PE): Windows vtables, source of the slots and argument sizes")
    ap.add_argument("--lsteamclient32", help="lsteamclient.so i386: argument size of each method (32-bit thiscall)")
    ap.add_argument("--out", required=True)
    a = ap.parse_args(argv)
    if a.lsteamclient_pe:
        source = {"dll": Path(a.dll).name, "sha256": _sha256(a.dll), "lsteamclient_pe": Path(a.lsteamclient_pe).name,
                  "lsteamclient_pe_sha256": _sha256(a.lsteamclient_pe)}
        pe = extract_pe_vtables(a.lsteamclient_pe)
        cat = build_catalog_from_pe(sorted(set(dll_interface_names(a.dll)) | set(pe)), pe, source)
        if a.lsteamclient32:
            source["lsteamclient32"] = Path(a.lsteamclient32).name
            source["lsteamclient32_sha256"] = _sha256(a.lsteamclient32)
            cat = add_ret_kinds(cat, parse_thunk_ret_kinds_i386(_dis(a.lsteamclient32, "objdump")))
    else:
        cat = extract_catalog(a.dll, a.lsteamclient, lsteamclient32=a.lsteamclient32)
    Path(a.out).write_text(json.dumps(cat, indent=1, sort_keys=True) + "\n")

if __name__ == "__main__":
    main()
