import argparse
import re
import subprocess
import sys
import tempfile
from pathlib import Path

from . import run

ROOT = run.ROOT

def build(work):
    exe = work / "scenarios.exe"
    subprocess.run([run.CC, "-O1", "-Wall", "-I", str(ROOT / "tests/conformance"), str(ROOT / "tests/conformance/scenarios.c"), "-o", str(exe)], check=True)
    return exe

def collect(path):
    out = {}
    for line in Path(path).read_text(errors="replace").splitlines():
        if line.startswith("S "):
            label, _, value = line[2:].partition(" ")
            out[label] = value
    return out

def normalize(label, value):
    if label.startswith("~"):
        return re.sub(r"=0x?0+(?=\s|$)|=(?!0+(\s|$))[0-9a-f]+", lambda m: "=0" if set(m.group(0)[1:]) <= {"0"} else "=nz", value)
    return value

def diff(ref, shim):
    a, b = collect(ref), collect(shim)
    same = bad = 0
    for label in sorted(set(a) | set(b)):
        va, vb = a.get(label), b.get(label)
        if va is not None and vb is not None:
            va, vb = normalize(label, va), normalize(label, vb)
        if va == vb:
            same += 1
        else:
            bad += 1
            print(f"{label}\n    real: {va}\n    shim: {vb}")
    print(f"\nsame {same}, different {bad}")
    return 0 if bad == 0 else 1

def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["ref", "shim", "diff"])
    ap.add_argument("a")
    ap.add_argument("b", nargs="?")
    ap.add_argument("--appid", type=int, default=480)
    n = ap.parse_args(argv)
    if n.mode == "diff":
        return diff(n.a, n.b)
    work = Path(tempfile.mkdtemp(prefix="scen-"))
    exe = build(work)
    out = Path(n.a).resolve()
    out.write_text("")
    run.attempt(n.mode, exe, out, n.appid, 0, set(), work)
    text = out.read_text()
    print(text[-400:] if "END" not in text else f"{len(collect(out))} results in {out}")
    return 0 if "END" in text else 1

if __name__ == "__main__":
    sys.exit(main())
