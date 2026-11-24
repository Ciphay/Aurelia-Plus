import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

class LaunchTest(unittest.TestCase):
    def test_launch_aborts_when_the_bridge_reports_an_error(self):
        work = Path(tempfile.mkdtemp())
        fake = work / "fake-aurelia"
        fake.write_text('#!/usr/bin/env bash\necho \'{"error":"Steam did not deliver a game connect token","type":"unknown"}\'\nexit 1\n')
        fake.chmod(fake.stat().st_mode | stat.S_IEXEC)
        marker = work / "play-was-called"
        play = work / "fake-play"
        env = {"PATH": "/usr/bin:/bin", "HOME": str(Path.home()), "AURELIA_BIN": str(fake), "AURELIA_PLUS_WORK": str(work)}
        r = subprocess.run(["bash", str(ROOT / "launch.sh"), "2357570"], capture_output=True, text=True, env=env, timeout=120)
        self.assertNotEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("bridge", r.stderr.lower())
        self.assertFalse(marker.exists())

    def test_launch_installs_both_shim_dlls_for_proton_to_copy(self):
        work = Path(tempfile.mkdtemp())
        fake = work / "fake-aurelia"
        fake.write_text("#!/usr/bin/env bash\nexit 1\n")
        fake.chmod(fake.stat().st_mode | stat.S_IEXEC)
        env = {"PATH": "/usr/bin:/bin", "HOME": str(Path.home()), "AURELIA_BIN": str(fake), "AURELIA_PLUS_WORK": str(work)}
        subprocess.run(["bash", str(ROOT / "launch.sh"), "2357570"], capture_output=True, text=True, env=env, timeout=300)
        legacy = work / "shimsteam/legacycompat"
        self.assertTrue((legacy / "steamclient64.dll").stat().st_size > 100000)
        self.assertTrue((legacy / "steamclient.dll").stat().st_size > 100000)

    def test_launch_does_not_rebuild_the_shim_when_the_sources_did_not_change(self):
        work = Path(tempfile.mkdtemp())
        fake = work / "fake-aurelia"
        fake.write_text("#!/usr/bin/env bash\nexit 1\n")
        fake.chmod(fake.stat().st_mode | stat.S_IEXEC)
        env = {"PATH": "/usr/bin:/bin", "HOME": str(Path.home()), "AURELIA_BIN": str(fake), "AURELIA_PLUS_WORK": str(work)}
        run = lambda: subprocess.run(["bash", str(ROOT / "launch.sh"), "2357570"], capture_output=True, text=True, env=env, timeout=300)
        run()
        dlls = [work / "shimsteam/legacycompat/steamclient64.dll", work / "shimsteam/legacycompat/steamclient.dll"]
        first = [d.stat().st_mtime_ns for d in dlls]
        run()
        self.assertEqual(first, [d.stat().st_mtime_ns for d in dlls], "the second run rebuilt the shim without any code change")

    def test_launch_rebuilds_the_shim_when_a_source_changes(self):
        work = Path(tempfile.mkdtemp())
        fake = work / "fake-aurelia"
        fake.write_text("#!/usr/bin/env bash\nexit 1\n")
        fake.chmod(fake.stat().st_mode | stat.S_IEXEC)
        src = work / "shim-copy"
        subprocess.run(["cp", "-r", str(ROOT / "shim"), str(src)], check=True)
        launch = work / "launch.sh"
        launch.write_text((ROOT / "launch.sh").read_text())
        (work / "prepare.sh").write_text((ROOT / "prepare.sh").read_text())
        (work / "catalog").symlink_to(ROOT / "catalog")
        (work / "tools").symlink_to(ROOT / "tools")
        (work / "winrt-textinput").symlink_to(ROOT / "winrt-textinput")
        (work / "steamurl").symlink_to(ROOT / "steamurl")
        (work / "shim").symlink_to(src)
        env = {"PATH": "/usr/bin:/bin", "HOME": str(Path.home()), "AURELIA_BIN": str(fake), "AURELIA_PLUS_WORK": str(work / "w")}
        run = lambda: subprocess.run(["bash", str(launch), "2357570"], capture_output=True, text=True, env=env, timeout=300)
        run()
        dll = work / "w/shimsteam/legacycompat/steamclient64.dll"
        before = dll.stat().st_mtime_ns
        (src / "core.c").write_text((src / "core.c").read_text() + "\n/* change */\n")
        run()
        self.assertNotEqual(before, dll.stat().st_mtime_ns, "a source changed and the shim was not rebuilt")

if __name__ == "__main__":
    unittest.main()
