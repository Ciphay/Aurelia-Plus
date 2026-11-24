import argparse
import os
import re
import statistics
import subprocess
import sys
import time
from pathlib import Path

STEP = re.compile(r"bridge: startup (.+?): (\d+) ms")

def run_once(aurelia, appid, timeout):
    p = subprocess.Popen([aurelia, "play", str(appid), "--plus"], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1)
    steps = {}
    deadline = time.time() + timeout
    for line in p.stdout:
        m = STEP.search(line)
        if m:
            steps[m.group(1)] = int(m.group(2))
        if "startup total" in line or time.time() > deadline:
            break
    time.sleep(3)
    subprocess.run([aurelia, "stop", str(appid)], capture_output=True, text=True, timeout=120)
    try:
        p.wait(timeout=60)
    except subprocess.TimeoutExpired:
        p.kill()
    return steps

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("appid", type=int)
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--cold", action="store_true")
    ap.add_argument("--aurelia", default=os.environ.get("AURELIA_BIN") or str(Path(__file__).resolve().parents[2] / "aurelia/target/release/aurelia"))
    ap.add_argument("--timeout", type=int, default=120)
    a = ap.parse_args()
    rows = []
    for i in range(a.runs):
        if a.cold or i == 0:
            subprocess.run([a.aurelia, "kill"], capture_output=True, text=True)
            time.sleep(1)
        steps = run_once(a.aurelia, a.appid, a.timeout)
        rows.append(steps)
        print(f"round {i + 1}: " + ", ".join(f"{k}={v}" for k, v in steps.items()), flush=True)
        time.sleep(5)
    names = sorted({k for r in rows for k in r})
    print("\nstep | p50 | p95 | rounds")
    for n in names:
        vals = sorted(r[n] for r in rows if n in r)
        if vals:
            p95 = vals[min(len(vals) - 1, int(round(0.95 * (len(vals) - 1))))]
            print(f"{n} | {int(statistics.median(vals))} | {p95} | {vals}")

if __name__ == "__main__":
    sys.exit(main())
