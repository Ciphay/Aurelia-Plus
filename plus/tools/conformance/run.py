import argparse
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
CC = "x86_64-w64-mingw32-gcc"
PROTON = Path.home() / ".local/share/Steam/steamapps/common/Proton - Experimental/proton"
STEAM = Path.home() / ".local/share/Steam"
ABORTS = Path(__file__).resolve().parent / "ref_aborts.txt"

def load_aborts():
    return {l.strip() for l in ABORTS.read_text().splitlines() if l.strip()} if ABORTS.exists() else set()

def save_aborts(names):
    ABORTS.write_text("".join(n + "\n" for n in sorted(names)))

def build(work):
    exe = work / "probe.exe"
    subprocess.run([CC, "-O1", "-Wall", "-I", str(ROOT / "tests/conformance"), str(ROOT / "tests/conformance/probe.c"), "-o", str(exe)], check=True)
    return exe

def steam_up():
    r = subprocess.run(["pgrep", "-f", "ubuntu12_32/steam( |$)|steamwebhelper"], capture_output=True, text=True)
    return bool(r.stdout.strip())

def ensure_steam():
    log = STEAM / "logs/connection_log.txt"
    for _ in range(3):
        if not steam_up():
            subprocess.Popen([str(STEAM / "steam.sh"), "-silent", "-nofriendsui"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        for _ in range(90):
            time.sleep(2)
            tail = [l for l in (log.read_text(errors="replace").splitlines() if log.exists() else []) if "[Logged " in l]
            if steam_up() and tail and "[Logged On" in tail[-1]:
                time.sleep(5)
                return True
    raise SystemExit("could not start the real Steam")

def attempt(mode, exe, part, appid, start, skip, work):
    env = {**os.environ, "PROBE_START": str(start), "PROBE_SKIP": ",".join(sorted(skip))}
    if mode == "ref":
        ensure_steam()
        compat = STEAM / "steamapps/compatdata/999001"
        compat.mkdir(parents=True, exist_ok=True)
        env.update(STEAM_COMPAT_DATA_PATH=str(compat), STEAM_COMPAT_CLIENT_INSTALL_PATH=str(STEAM), SteamAppId=str(appid), SteamGameId=str(appid))
        cmd = ["python3", str(PROTON), "run", str(exe), r"C:\Program Files (x86)\Steam\steamclient64.dll", "Z:" + str(part), str(appid)]
        return subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=1200)
    dll = ROOT / "run/shimsteam/legacycompat/steamclient64.dll"
    win = lambda p: "Z:" + str(p).replace("/", "\\")
    env.update(AURELIA_PLUS_PROBE_EXE=str(exe), AURELIA_SHIM_LOG=str(work / "shim.log"),
               AURELIA_PLUS_GAME_ENV=f"PROBE_DLL={win(dll)} PROBE_OUT={win(part)} PROBE_APPID={appid} PROBE_START={start} PROBE_HANG_MS={os.environ.get('PROBE_HANG_MS', '800')} PROBE_ONLY={os.environ.get('PROBE_ONLY', '')} PROBE_SKIP={','.join(sorted(skip))}")
    return subprocess.run(["bash", str(ROOT / "launch.sh"), str(appid)], env=env, capture_output=True, text=True, timeout=1800)

def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["ref", "shim"])
    ap.add_argument("out")
    ap.add_argument("--appid", type=int, default=480)
    ap.add_argument("--start", type=int, default=0, help="resume from this index and append to the output file")
    a = ap.parse_args(argv)
    work = Path(tempfile.mkdtemp(prefix="conf-"))
    exe = build(work)
    out = Path(a.out)
    if not a.start:
        out.write_text("")
    known = load_aborts() if a.mode == "ref" else set()
    start, skip, tries = a.start, set(known), 0
    while tries < 400:
        tries += 1
        part = work / f"part{tries}.txt"
        part.write_text("")
        attempt(a.mode, exe, part, a.appid, start, skip, work)
        text = part.read_text()
        for _ in range(6):
            if "NOPIPE" not in text:
                break
            print("client not ready; waiting", flush=True)
            if a.mode == "ref":
                ensure_steam()
            else:
                time.sleep(10)
            part.write_text("")
            attempt(a.mode, exe, part, a.appid, start, skip, work)
            text = part.read_text()
        with out.open("a") as f:
            f.write("".join(l + "\n" for l in text.splitlines() if not l.startswith("C ")))
        if "\nEND" in "\n" + text:
            print(f"finished after {tries} attempts")
            return 0
        calls = [l for l in text.splitlines() if l.startswith("C ")]
        if not calls:
            print("the probe did not start:", text[-300:])
            return 1
        last = calls[-1].split(" ", 2)
        done = [l for l in text.splitlines() if l.startswith("M ") and l.split(" ")[1] == last[2]]
        hang = "HANG " + last[2] in text
        if not done and not hang:
            with out.open("a") as f:
                f.write(f"M {last[2]} ABORT\n")
        skip.add(last[2])
        if a.mode == "ref":
            known.add(last[2])
            save_aborts(known)
        start = int(last[1]) + 1
        print(f"resuming after {last[2]} (index {last[1]})", flush=True)
    return 1

if __name__ == "__main__":
    sys.exit(main())
