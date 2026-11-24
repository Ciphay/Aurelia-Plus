import argparse
import collections
import json
import re
import sys
from pathlib import Path

def parse(path):
    ifaces, methods = {}, {}
    for line in open(path, errors="replace"):
        line = line.rstrip("\n")
        if line.startswith("I "):
            _, name, ok = line.split(" ")
            ifaces[name] = ifaces.get(name, False) or ok == "1"
        elif line.startswith("M "):
            _, what, res = (line.split(" ", 2) + [""])[:3]
            if res.startswith("ret="):
                methods[what] = ("ret", res[4:])
            elif res.startswith("crash="):
                methods[what] = ("crash", res[6:])
            else:
                methods[what] = (res.lower() or "?", "")
        elif line.startswith("HANG "):
            methods[line[5:]] = ("hang", "")
    return ifaces, methods

VOLATILE = {"GetServerRealTime", "GetSecondsSinceAppActive", "GetSecondsSinceComputerActive", "GetIPCCallCount", "GetResultTimestamp"}

def ret_kinds(catalog="catalog/catalog.json"):
    out = {}
    for i in json.loads(Path(catalog).read_text())["interfaces"]:
        for s in i.get("slots") or []:
            out[f"{i['name']}.{s['name']}"] = s.get("ret")
    return out

def _is_pointer(value):
    try:
        return int(value, 16) >= 0x10000
    except ValueError:
        return False

def normalize(what, value, kinds):
    try:
        v = int(value, 16)
    except ValueError:
        return value
    if v == 0:
        return "0"
    if what.split(".", 1)[-1] in VOLATILE:
        return "volatile"
    if v == 0xFFFFFFFFFFFFFFFF or v == 0xFFFFFFFF:
        return "invalid"
    if kinds.get(what) in ("call", "qword", "struct8") or v > 0xFFFFFFFF:
        return "non-zero"
    return value

def load_accepted(path, kinds):
    p = Path(path)
    return json.loads(p.read_text()).get("rules", []) if p.exists() else []

def accepted_reason(what, r, s, rules, kinds):
    import fnmatch
    if kinds.get(what) == "void":
        return "void method: the return value has no meaning"
    for rule in rules:
        if fnmatch.fnmatch(what, rule["pattern"]):
            return rule["reason"]
    return None

def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("ref")
    ap.add_argument("shim")
    ap.add_argument("--all", action="store_true")
    ap.add_argument("--accepted", default="tools/conformance/accepted.json", help="accepted differences, with the reason")
    a = ap.parse_args(argv)
    ri, rm = parse(a.ref)
    si, sm = parse(a.shim)
    kinds = ret_kinds()
    accepted = load_accepted(a.accepted, kinds)
    groups = collections.defaultdict(list)
    for name, ok in sorted(ri.items()):
        if ok and not si.get(name, False):
            groups["NO_INTERFACE"].append(name)
    for what, r in sorted(rm.items()):
        s = sm.get(what)
        if s is None:
            continue
        if r[0] == "ret" and s[0] == "ret":
            raw_s = s[1]
            r, s = ("ret", normalize(what, r[1], kinds)), ("ret", normalize(what, s[1], kinds))
            if r[1] == "non-zero" and s[1] != "non-zero" and _is_pointer(raw_s):
                s = ("ret", "non-zero")
        if r == s:
            groups["SAME"].append(what)
        elif r[0] in ("crash", "abort", "hang") and s[0] == "ret":
            groups["REF_FAIL"].append(f"{what}  ref={r[0]} shim={s[1]}")
        elif r[0] in ("crash", "abort", "hang") and s[0] in ("crash", "abort", "hang"):
            groups["REF_FAIL"].append(f"{what}  ref={r[0]} shim={s[0]}")
        elif s[0] in ("crash", "abort", "hang"):
            groups["SHIM_FAIL"].append(f"{what}  ref={r[0]}:{r[1]} shim={s[0]}")
        else:
            why = accepted_reason(what, r, s, accepted, kinds)
            if why:
                groups["ACCEPTED"].append(f"{what}  ref={r[1]} shim={s[1]}  ({why})")
            else:
                groups["VALUE"].append(f"{what}  ref={r[1]} shim={s[1]}")
    total = sum(len(v) for k, v in groups.items() if k != "NO_INTERFACE")
    print(f"compared {total} methods; reference interfaces: {sum(ri.values())}")
    for k in ("NO_INTERFACE", "SHIM_FAIL", "VALUE", "ACCEPTED", "REF_FAIL", "SAME"):
        print(f"{k}: {len(groups[k])}")
    for k in ("NO_INTERFACE", "SHIM_FAIL", "VALUE") + (("ACCEPTED", "REF_FAIL", "SAME") if a.all else ()):
        if groups[k]:
            print(f"\n== {k}")
            print("\n".join(groups[k]))

if __name__ == "__main__":
    sys.exit(main())
