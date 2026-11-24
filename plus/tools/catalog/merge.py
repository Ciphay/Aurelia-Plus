import copy
import json
from pathlib import Path

def merge_signatures(catalog, sdk_json, overrides_dir):
    if sdk_json is not None:
        raise ValueError("steam_api.json is not used: license not verified")
    out = copy.deepcopy(catalog)
    wanted = {}
    for f in sorted(Path(overrides_dir).glob("*.json")):
        for iface, methods in json.loads(f.read_text()).items():
            wanted.setdefault(iface, {}).update(methods)
    for i in out["interfaces"]:
        for s in i["slots"] or []:
            sig = wanted.get(i["name"], {}).get(s["name"])
            if sig is not None:
                s["sig"] = sig
    return out
