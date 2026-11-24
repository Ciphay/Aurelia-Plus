import re
from pathlib import Path

NAME = re.compile(r"^(lib)?steamclient(64)?\.(dll|so)$|^(lib)?steam_api(64)?\.(dll|so)$", re.I)
MAGIC = (b"MZ", b"\x7fELF")
NEEDLE = b"Valve Corporation"

def find_violations(files):
    out = []
    for f in map(Path, files):
        if not f.is_file():
            continue
        if NAME.match(f.name):
            out.append(f"{f}: Valve binary name")
            continue
        with f.open("rb") as fh:
            head = fh.read(4)
            if head.startswith(MAGIC) and NEEDLE in head + fh.read():
                out.append(f"{f}: binary with Valve copyright")
    return out
