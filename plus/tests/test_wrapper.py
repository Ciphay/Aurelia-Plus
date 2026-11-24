import os
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

def run_wrapper(wrapper_env, inherited):
    work = Path(tempfile.mkdtemp())
    (work / "wrapper.env").write_text(wrapper_env)
    env = {"PATH": "/usr/bin:/bin", "AURELIA_PLUS_WORK": str(work), **inherited}
    return subprocess.run(["bash", str(ROOT / "launch-wrapper.sh"), "echo", "run", "game.exe"], capture_output=True, text=True, env=env)

class WrapperEnvTest(unittest.TestCase):
    def test_probe_variables_inherited_from_the_daemon_never_replace_the_game(self):
        r = run_wrapper("export AURELIA_PLUS_STEAM_DIR=/x\n", {"AURELIA_PLUS_PROBE_EXE": "bad.exe"})
        self.assertEqual(r.stdout.strip(), "run game.exe", r.stderr)

    def test_probe_from_wrapper_env_still_replaces_the_last_argument(self):
        r = run_wrapper("export AURELIA_PLUS_STEAM_DIR=/x\nexport AURELIA_PLUS_PROBE_EXE=probe.exe\n", {})
        self.assertEqual(r.stdout.strip(), "run probe.exe", r.stderr)

    def test_shim_debug_variables_inherited_from_the_daemon_do_not_reach_the_game(self):
        work = Path(tempfile.mkdtemp())
        (work / "wrapper.env").write_text("export AURELIA_PLUS_STEAM_DIR=/x\n")
        env = {"PATH": "/usr/bin:/bin", "AURELIA_PLUS_WORK": str(work), "AURELIA_SHIM_TRACE": "1", "AURELIA_SHIM_TICKET_FILE": "old.bin"}
        show = 'echo "${AURELIA_SHIM_TRACE:-unset} ${AURELIA_SHIM_TICKET_FILE:-unset}"'
        r = subprocess.run(["bash", str(ROOT / "launch-wrapper.sh"), "sh", "-c", show], capture_output=True, text=True, env=env)
        self.assertEqual(r.stdout.strip(), "unset unset", r.stderr)

    def test_winedebug_comes_only_from_wrapper_env(self):
        work = Path(tempfile.mkdtemp())
        (work / "wrapper.env").write_text("export AURELIA_PLUS_STEAM_DIR=/x\nexport AURELIA_PLUS_WINEDEBUG=+seh\n")
        show = 'echo "${WINEDEBUG:-unset}"'
        env = {"PATH": "/usr/bin:/bin", "AURELIA_PLUS_WORK": str(work)}
        r = subprocess.run(["bash", str(ROOT / "launch-wrapper.sh"), "sh", "-c", show], capture_output=True, text=True, env=env)
        self.assertEqual(r.stdout.strip(), "+seh", r.stderr)
        (work / "wrapper.env").write_text("export AURELIA_PLUS_STEAM_DIR=/x\n")
        env["AURELIA_PLUS_WINEDEBUG"] = "+all"
        r = subprocess.run(["bash", str(ROOT / "launch-wrapper.sh"), "sh", "-c", show], capture_output=True, text=True, env=env)
        self.assertEqual(r.stdout.strip(), "unset", r.stderr)

    def test_game_arguments_come_only_from_wrapper_env_and_follow_the_command(self):
        work = Path(tempfile.mkdtemp())
        (work / "wrapper.env").write_text("export AURELIA_PLUS_STEAM_DIR=/x\nexport AURELIA_PLUS_GAME_ARGS='-novid -windowed'\n")
        env = {"PATH": "/usr/bin:/bin", "AURELIA_PLUS_WORK": str(work)}
        r = subprocess.run(["bash", str(ROOT / "launch-wrapper.sh"), "echo", "run", "game.exe"], capture_output=True, text=True, env=env)
        self.assertEqual(r.stdout.strip(), "run game.exe -novid -windowed", r.stderr)
        (work / "wrapper.env").write_text("export AURELIA_PLUS_STEAM_DIR=/x\n")
        env["AURELIA_PLUS_GAME_ARGS"] = "-bad"
        r = subprocess.run(["bash", str(ROOT / "launch-wrapper.sh"), "echo", "run", "game.exe"], capture_output=True, text=True, env=env)
        self.assertEqual(r.stdout.strip(), "run game.exe", r.stderr)

    def test_extra_environment_comes_only_from_wrapper_env(self):
        work = Path(tempfile.mkdtemp())
        (work / "wrapper.env").write_text("export AURELIA_PLUS_STEAM_DIR=/x\nexport AURELIA_PLUS_GAME_ENV='DXVK_LOG_LEVEL=debug PROTON_LOG=1'\n")
        show = 'echo "${DXVK_LOG_LEVEL:-unset} ${PROTON_LOG:-unset}"'
        env = {"PATH": "/usr/bin:/bin", "AURELIA_PLUS_WORK": str(work)}
        r = subprocess.run(["bash", str(ROOT / "launch-wrapper.sh"), "sh", "-c", show], capture_output=True, text=True, env=env)
        self.assertEqual(r.stdout.strip(), "debug 1", r.stderr)
        (work / "wrapper.env").write_text("export AURELIA_PLUS_STEAM_DIR=/x\n")
        env["AURELIA_PLUS_GAME_ENV"] = "DXVK_LOG_LEVEL=bad"
        r = subprocess.run(["bash", str(ROOT / "launch-wrapper.sh"), "sh", "-c", show], capture_output=True, text=True, env=env)
        self.assertEqual(r.stdout.strip(), "unset unset", r.stderr)

if __name__ == "__main__":
    unittest.main()
