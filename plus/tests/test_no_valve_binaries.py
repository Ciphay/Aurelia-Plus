import subprocess
import tempfile
import unittest
from pathlib import Path

from tools.guard import find_violations

ROOT = Path(__file__).resolve().parent.parent

class GuardTest(unittest.TestCase):
    def _files(self, spec):
        d = Path(tempfile.mkdtemp())
        paths = []
        for name, data in spec.items():
            p = d / name
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(data)
            paths.append(p)
        return d, paths

    def test_flags_steamclient_and_steam_api_names(self):
        _, paths = self._files({"a/steamclient64.dll": b"x", "b/libsteam_api.so": b"x", "c/steamclient.so": b"x", "ok/readme.md": b"x"})
        names = {Path(v.split(":")[0]).name for v in find_violations(paths)}
        self.assertEqual(names, {"steamclient64.dll", "libsteam_api.so", "steamclient.so"})

    def test_flags_pe_or_elf_that_carries_valve_copyright(self):
        _, paths = self._files({"x.bin": b"MZ" + b"\0" * 8 + b"Valve Corporation", "y.bin": b"\x7fELF" + b"Valve Corporation", "z.txt": b"Valve Corporation"})
        flagged = {Path(v.split(":")[0]).name for v in find_violations(paths)}
        self.assertEqual(flagged, {"x.bin", "y.bin"})

    def test_repository_tracks_no_valve_binaries(self):
        out = subprocess.run(["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True).stdout.split()
        self.assertEqual(find_violations([ROOT / f for f in out]), [])

if __name__ == "__main__":
    unittest.main()
