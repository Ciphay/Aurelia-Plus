import json
import re
import sys
from pathlib import Path

from . import pevtables

KNOWN_SINGLE = {"SteamApps001": [{"name": "GetAppData", "ret": "word", "argbytes32": 16}]}

def family_and_version(name):
    m = re.match(r"^(.*?)(?:_INTERFACE_V(?:ERSION)?_?)?(\d+)$", name)
    return (m.group(1), int(m.group(2))) if m else (name, -1)

def align(known, new_args):
    a, b = [s.get("argbytes32") for s in known], new_args
    n, m = len(a), len(b)
    best = [[0] * (m + 1) for _ in range(n + 1)]
    for i in range(n - 1, -1, -1):
        for j in range(m - 1, -1, -1):
            same = a[i] is None or b[j] is None or a[i] == b[j]
            best[i][j] = max(best[i + 1][j], best[i][j + 1], best[i + 1][j + 1] + 1 if same else 0)
    pairs, i, j = [None] * m, 0, 0
    while i < n and j < m:
        same = a[i] is None or b[j] is None or a[i] == b[j]
        if same and best[i][j] == best[i + 1][j + 1] + 1:
            pairs[j] = known[i]
            i += 1
            j += 1
        elif best[i + 1][j] >= best[i][j + 1]:
            i += 1
        else:
            j += 1
    return pairs

def fill(catalog, shapes):
    by_class = {pevtables.class_key(k[len("CAdapter"):]): v for k, v in shapes.items()}
    with_slots = {i["name"]: i for i in catalog["interfaces"] if i.get("slots")}
    report = []
    for iface in catalog["interfaces"]:
        if iface.get("slots"):
            continue
        shape = by_class.get(pevtables.interface_key(iface["name"]))
        if not shape:
            report.append((iface["name"], "the official client does not have this version: left as is"))
            continue
        if iface["name"] in KNOWN_SINGLE and shape["n"] == len(KNOWN_SINGLE[iface["name"]]):
            names = [dict(s) for s in KNOWN_SINGLE[iface["name"]]]
            source = "public SDK"
        else:
            fam, ver = family_and_version(iface["name"])
            near = sorted((abs(v - ver), -v, n) for n, i in with_slots.items() for f, v in [family_and_version(n)] if f == fam and v != ver)
            neighbour = with_slots[near[0][2]] if near else None
            pairs = align(neighbour["slots"], shape["args"]) if neighbour else [None] * shape["n"]
            names = []
            for k, (pair, arg) in enumerate(zip(pairs, shape["args"])):
                if pair:
                    names.append({"name": pair["name"], "ret": pair.get("ret", "word"), "argbytes32": arg if arg is not None else pair.get("argbytes32", 0)})
                else:
                    names.append({"name": f"Unknown{k}", "ret": "word", "argbytes32": arg or 0})
            source = neighbour["name"] if neighbour else "none"
        iface["slots"] = [{"argbytes32": s["argbytes32"], "name": s["name"], "ret": s["ret"], "sig": None, "slot": n} for n, s in enumerate(names)]
        iface["complete"] = True
        iface["derived_from"] = source
        report.append((iface["name"], f"{shape['n']} slots (names from {source}; {sum(1 for s in names if s['name'].startswith('Unknown'))} unnamed)"))
    return report

if __name__ == "__main__":
    path = Path(sys.argv[1])
    catalog = json.loads(path.read_text())
    for line in fill(catalog, pevtables.scan(sys.argv[2])):
        print(*line, sep="  ")
    path.write_text(json.dumps(catalog, indent=1, sort_keys=True) + "\n")
