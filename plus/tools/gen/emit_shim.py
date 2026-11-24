import json
from pathlib import Path

PAD_UNKNOWN = 128

HEADER = """
#ifndef AURELIA_IFACES_H
#define AURELIA_IFACES_H
#include <stdint.h>

typedef struct { void **vtable; } IfaceObj;
typedef struct {
    const char *name;
    int n_slots;
    void **vtable;
    const char *const *slot_names;
    IfaceObj *object;
    void **impl;
    const short *argbytes;
} IfaceDesc;

extern const IfaceDesc g_ifaces[];
extern const int g_n_ifaces;

void shim_arity_mismatch(int iface, int slot, int delta);

uint64_t shim_callresult_fail(void);

void shim_unimplemented(const char *iface, int slot, const char *method);

int shim_register(const char *iface, const char *method, void *fn);
#endif
"""

REGISTER = """int shim_register(const char *iface, const char *method, void *fn) {
    for (int i = 0; i < g_n_ifaces; i++) {
        const IfaceDesc *d = &g_ifaces[i];
        if (strcmp(d->name, iface)) continue;
        for (int s = 0; s < d->n_slots; s++) {
            if (d->slot_names[s] && !strcmp(d->slot_names[s], method)) { d->impl[s] = fn; return 0; }
        }
        return -1;
    }
    return -1;
}
"""

def _size(iface):
    slots = iface["slots"]
    if slots is None:
        return PAD_UNKNOWN
    known = [s["slot"] for s in slots if s["slot"] is not None]
    return max(1, (max(known) + 1) if known else 0, len(slots))

def _c(text):
    return json.dumps(text)

def emit_shim(catalog, out_dir, abi):
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    body = ['', '#include <string.h>', '#include "ifaces.h"', ""]
    table = []
    for k, iface in enumerate(catalog["interfaces"]):
        n = _size(iface)
        by_slot = {s["slot"]: s for s in (iface["slots"] or []) if s["slot"] is not None}
        wrapped = getattr(abi, "wraps_calls", False)
        for s in range(n):
            m = by_slot.get(s)
            body.append(abi.thunk(f"t{k}_{s}", iface["name"], s, m["name"] if m else None, m["sig"] if m else None, m.get("argbytes32") if m else None, m.get("ret") if m else None).rstrip("\n"))
        body.append(f"static void *vt_{k}[{n}] = {{ " + ", ".join(f"(void *)t{k}_{s}" for s in range(n)) + " };")
        sizes = [by_slot[s].get("argbytes32", -1) if s in by_slot else -1 for s in range(n)]
        body.append(f"static const short ab_{k}[{n}] = {{ " + ", ".join(str(x) for x in sizes) + " };")
        if wrapped:
            body.append(f"void *impl_{k}[{n}] = {{ " + ", ".join(f"(void *){abi.impl_name(f't{k}_{s}')}" for s in range(n)) + " };")
        names = [_c(by_slot[s]["name"]) if s in by_slot else "NULL" for s in range(n)]
        body.append(f"static const char *const sn_{k}[{n}] = {{ " + ", ".join(names) + " };")
        body.append(f"static IfaceObj obj_{k} = {{ vt_{k} }};")
        body.append("")
        impl, ab = (f"impl_{k}" if wrapped else f"vt_{k}", f"ab_{k}")
        table.append(f'    {{ {_c(iface["name"])}, {n}, vt_{k}, sn_{k}, &obj_{k}, {impl}, {ab} }},')
    body.append("const IfaceDesc g_ifaces[] = {")
    body.extend(table)
    body.append("};")
    body.append(f"const int g_n_ifaces = {len(table)};")
    body.append("")
    body.append(REGISTER)
    (out / "ifaces.h").write_text(HEADER)
    (out / "ifaces.c").write_text("\n".join(body))

def main(argv=None):
    import argparse
    ap = argparse.ArgumentParser(prog="python -m tools.gen.emit_shim")
    ap.add_argument("--catalog", required=True)
    ap.add_argument("--abi", choices=["win64", "win32"], required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args(argv)
    if a.abi == "win64":
        from tools.gen.abi_win64 import Win64Abi as Abi
    else:
        from tools.gen.abi_win32 import Win32Abi as Abi
    emit_shim(json.loads(Path(a.catalog).read_text()), a.out, Abi())

if __name__ == "__main__":
    main()
