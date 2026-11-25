import argparse
import json
from pathlib import Path

KINDS = {"zero": "p_zero", "true": "p_true", "empty": "p_empty", "void": "p_void", "struct8": "p_struct8",
         "hidden2": "p_hidden2", "hidden16": "p_hidden16", "hidden40": "p_hidden40"}

HEADER = '''
#include <stdint.h>
#include <string.h>

#include "shim.h"

static __attribute__((unused)) uint64_t SHIM_TC p_zero(void *self) { (void)self; return 0; }
static __attribute__((unused)) uint64_t SHIM_TC p_true(void *self) { (void)self; return 1; }
static __attribute__((unused)) const char *SHIM_TC p_empty(void *self) { (void)self; return ""; }
static __attribute__((unused)) void SHIM_TC p_void(void *self) { (void)self; }
static __attribute__((unused)) uint64_t *SHIM_TC p_struct8(void *self, uint64_t *ret) { (void)self; *ret = 0; return ret; }

#ifdef _WIN64
static __attribute__((unused)) uint64_t SHIM_TC p_hidden2(void *self) { (void)self; return 0; }
#else
static __attribute__((unused)) void *SHIM_TC p_hidden2(void *self, void *ret) { (void)self; memset(ret, 0, 2); return ret; }
#endif
static __attribute__((unused)) void *SHIM_TC p_hidden16(void *self, void *ret) { (void)self; memset(ret, 0, 16); return ret; }
static __attribute__((unused)) void *SHIM_TC p_hidden40(void *self, void *ret) { (void)self; memset(ret, 0, 40); return ret; }

void register_policies(void) {
'''

def _func(kind):
    if kind in KINDS:
        return KINDS[kind]
    if kind.startswith("ret:") and kind[4:].lstrip("-").isdigit():
        return "p_ret_" + kind[4:].replace("-", "m")
    raise ValueError(f"unknown policy: {kind}")

def generate(policy):
    consts = sorted({k for kinds in policy.values() for k in kinds if k.startswith("ret:")})
    head = HEADER.split("void register_policies(void) {")[0]
    for k in consts:
        fn = _func(k)
        head += f"static __attribute__((unused)) uint64_t SHIM_TC {fn}(void *self) {{ (void)self; return (uint64_t)(int64_t)({k[4:]}); }}\n"
    lines = [head + "\nvoid register_policies(void) {\n"]
    for prefix in sorted(policy):
        for kind in sorted(policy[prefix]):
            fn = _func(kind)
            for method in sorted(set(policy[prefix][kind])):
                lines.append(f'    shim_register_family("{prefix}", "{method}", (void *){fn});\n')
    lines.append("}\n")
    fns = sorted({_func(kind) for prefix in policy for kind in policy[prefix]} | {"p_hidden2", "p_hidden16", "p_hidden40"})
    lines.append("\nint shim_is_policy_stub(const void *fn) {\n    const void *const all[] = {" + ", ".join(f"(const void *){f}" for f in fns) + "};\n")
    lines.append("    for (unsigned i = 0; i < sizeof all / sizeof *all; i++) if (all[i] == fn) return 1;\n    return 0;\n}\n")
    return "".join(lines)

def main(argv=None):
    ap = argparse.ArgumentParser(prog="python -m tools.policy.gen_policy")
    ap.add_argument("--policy", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args(argv)
    Path(a.out).parent.mkdir(parents=True, exist_ok=True)
    Path(a.out).write_text(generate(json.loads(Path(a.policy).read_text())))

if __name__ == "__main__":
    main()
