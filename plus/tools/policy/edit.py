import json
from pathlib import Path

from tools.policy import gen_policy

ROOT = Path(__file__).resolve().parent.parent.parent
POLICY = ROOT / "catalog/policy.json"

def review(prefix, **kinds):
    pol = json.loads(POLICY.read_text())
    entry = pol.setdefault(prefix, {})
    for kind, names in kinds.items():
        entry[kind] = sorted(set(entry.get(kind, [])) | set(names))
    POLICY.write_text(json.dumps(pol, indent=1, sort_keys=True) + "\n")
    (ROOT / "shim/generated/policy.c").write_text(gen_policy.generate(pol))
