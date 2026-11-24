import json
import subprocess
import sys
import tempfile
from pathlib import Path

from tests import test_shim as T

def main():
    appid = int(sys.argv[1]) if len(sys.argv) > 1 else 480
    arch = T._Win32 if len(sys.argv) > 2 and sys.argv[2] == "win32" else T._Win64
    case = sys.argv[3] if len(sys.argv) > 3 else "live"

    class Runner(arch, T.unittest.TestCase):
        def runTest(self):
            pass

    r = Runner()
    out = Path(tempfile.mkdtemp())
    exe = r.build_selftest(out)
    import os
    binary = T.ROOT.parent / "aurelia/target/release/aurelia"
    if not binary.exists():
        binary = T.ROOT.parent / "aurelia/target/debug/aurelia"
    bridge = subprocess.Popen([str(binary), "--json", "bridge", "serve", "--app", str(appid)],
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env={**os.environ, "AURELIA_NO_DAEMON": "1"})
    try:
        info = json.loads(bridge.stdout.readline())
        run = T.wine_args([exe], out, {"SELFTEST_NET": case, "AURELIA_SHIM_LOG": "/tmp/aurelia-live-shim.log", **({k: __import__("os").environ[k] for k in ("LIVE_BUY", "LIVE_VARIANT", "LIVE_KEEP", "LIVE_APP") if k in __import__("os").environ}), "AURELIA_BRIDGE_PORT": str(info["port"]), "AURELIA_BRIDGE_TOKEN": info["token"]})
        print(run.stdout)
        print(run.stderr[-600:])
        return 0 if f"PASS net_{case}" in run.stdout else 1
    finally:
        bridge.terminate()

if __name__ == "__main__":
    sys.exit(main())
