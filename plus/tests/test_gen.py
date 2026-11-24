import json
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from tools.gen.abi_win64 import Win64Abi
from tools.gen.emit_shim import PAD_UNKNOWN, emit_shim

ROOT = Path(__file__).resolve().parent.parent
CATALOG = ROOT / "catalog" / "catalog.json"
CC = shutil.which("x86_64-w64-mingw32-gcc")
WINE = Path.home() / ".local/share/Steam/compatibilitytools.d/GE-Proton11-12/files/bin/wine"
PREFIX = Path.home() / ".cache/aurelia-plus/wine-prefix"

def slot(n, name):
    return {"slot": n, "name": name, "sig": None}

SYNTH = {"schema": 1, "source": {}, "interfaces": [
    {"name": "IfaceA001", "complete": True, "slots": [slot(0, "Foo"), slot(1, "Bar")]},
    {"name": "IfaceB001", "complete": False, "slots": [slot(0, "X"), slot(None, "Y"), slot(5, "Z")]},
    {"name": "IfaceC001", "complete": False, "slots": None},
]}

def emit(catalog, out=None):
    out = Path(out or tempfile.mkdtemp())
    emit_shim(catalog, out, Win64Abi())
    return out

class Win32AbiTest(unittest.TestCase):
    def test_thunk_is_thiscall_with_the_catalogued_argument_words(self):
        from tools.gen.abi_win32 import Win32Abi
        cat = {"schema": 1, "source": {}, "interfaces": [{"name": "IfaceA001", "complete": True, "slots": [
            {"slot": 0, "name": "NoArgs", "sig": None, "argbytes32": 0},
            {"slot": 1, "name": "Three", "sig": None, "argbytes32": 12},
            {"slot": 2, "name": "Unknown", "sig": None}]}]}
        out = Path(tempfile.mkdtemp())
        emit_shim(cat, out, Win32Abi())
        c = (out / "ifaces.c").read_text()
        self.assertIn("__attribute__((thiscall)) t0_0_stub(void *self)", c)
        self.assertIn("__attribute__((thiscall)) t0_1_stub(void *self, uint32_t a0, uint32_t a1, uint32_t a2)", c)
        self.assertIn("__attribute__((thiscall)) t0_2_stub(void *self)", c)
        self.assertIn("ret $12", c)
        self.assertIn("static const short ab_0[3] = { 0, 12, -1 };", c)

    @unittest.skipUnless(shutil.which("i686-w64-mingw32-gcc") and WINE.exists(), "mingw32 or wine missing")
    def test_wrapper_keeps_the_callers_stack_right_when_the_implementation_disagrees(self):
        from tools.gen.abi_win32 import Win32Abi
        cat = {"schema": 1, "source": {}, "interfaces": [{"name": "IfaceA001", "complete": True, "slots": [
            {"slot": 0, "name": "NoArgs", "sig": None, "argbytes32": 0},
            {"slot": 1, "name": "Three", "sig": None, "argbytes32": 12}]}]}
        out = Path(tempfile.mkdtemp())
        emit_shim(cat, out, Win32Abi())
        exe = out / "arity32.exe"
        r = subprocess.run([shutil.which("i686-w64-mingw32-gcc"), "-Wall", "-O1", "-I", str(out), str(ROOT / "tests/arity32.c"), str(out / "ifaces.c"), "-o", str(exe)],
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr[:2000])
        env = {"PATH": "/usr/bin:/bin", "WINEPREFIX": str(PREFIX), "WINEDEBUG": "-all", "HOME": str(Path.home())}
        run = subprocess.run([str(WINE), str(exe)], capture_output=True, text=True, env=env, timeout=240, cwd=out)
        self.assertIn("OK arity", run.stdout, run.stdout[-500:] + run.stderr[-500:])

class DefaultBehaviorTest(unittest.TestCase):
    CAT = {"schema": 1, "source": {}, "interfaces": [{"name": "IfaceA001", "complete": True, "slots": [
        {"slot": 0, "name": "Plain", "sig": None, "argbytes32": 4, "ret": "word"},
        {"slot": 1, "name": "Async", "sig": None, "argbytes32": 8, "ret": "call"},
        {"slot": 2, "name": "Who", "sig": None, "argbytes32": 4, "ret": "struct8"}]}]}

    def test_win64_unimplemented_methods_answer_by_return_kind(self):
        c = (emit(self.CAT) / "ifaces.c").read_text()
        self.assertRegex(c, r"t0_1\(void \*self\) \{[^}]*return shim_callresult_fail\(\);")
        self.assertRegex(c, r"t0_2\(void \*self, uint64_t \*ret\) \{[^}]*\*ret = 0; return ret;")

    def test_win32_unimplemented_methods_answer_by_return_kind(self):
        from tools.gen.abi_win32 import Win32Abi
        out = Path(tempfile.mkdtemp())
        emit_shim(self.CAT, out, Win32Abi())
        c = (out / "ifaces.c").read_text()
        self.assertRegex(c, r"t0_1_stub\(void \*self, uint32_t a0, uint32_t a1\) \{[^}]*return shim_callresult_fail\(\);")
        self.assertRegex(c, r"t0_2_stub\(void \*self, uint32_t a0\) \{[^}]*\*\(uint64_t \*\)\(uintptr_t\)a0 = 0; return a0;")

    def test_header_declares_the_failed_call_result_hook(self):
        self.assertIn("shim_callresult_fail(", (emit(self.CAT) / "ifaces.h").read_text())

class SyntheticTest(unittest.TestCase):
    def test_vtable_sizes(self):
        c = (emit(SYNTH) / "ifaces.c").read_text()
        self.assertRegex(c, r'\{ "IfaceA001", 2, ')
        self.assertRegex(c, r'\{ "IfaceB001", 6, ')
        self.assertRegex(c, r'\{ "IfaceC001", %d, ' % PAD_UNKNOWN)

    def test_method_names_land_in_their_slots(self):
        c = (emit(SYNTH) / "ifaces.c").read_text()
        self.assertRegex(c, r'sn_0\[2\] = \{ "Foo", "Bar" \}')
        self.assertRegex(c, r'sn_1\[6\] = \{ "X", NULL, NULL, NULL, NULL, "Z" \}')

    def test_header_declares_the_runtime_contract(self):
        h = (emit(SYNTH) / "ifaces.h").read_text()
        for needle in ("typedef struct", "g_ifaces[]", "g_n_ifaces", "shim_unimplemented(", "shim_register("):
            self.assertIn(needle, h)

@unittest.skipUnless(CC and CATALOG.exists(), "mingw or catalog missing")
class RealCatalogTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.catalog = json.loads(CATALOG.read_text())
        cls.out = emit(cls.catalog)

    def test_emitted_c_compiles_with_mingw(self):
        r = subprocess.run([CC, "-c", "-Wall", "-Werror", "-I", str(self.out), str(self.out / "ifaces.c"), "-o", str(self.out / "ifaces.o")],
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr[:2000])

    def test_every_catalog_interface_is_emitted(self):
        c = (self.out / "ifaces.c").read_text()
        for i in self.catalog["interfaces"]:
            self.assertIn('{ "%s", ' % i["name"], c)

    def test_generation_is_deterministic(self):
        other = emit(self.catalog)
        for f in ("ifaces.c", "ifaces.h"):
            self.assertEqual((self.out / f).read_bytes(), (other / f).read_bytes())

    @unittest.skipUnless(WINE.exists(), "GE-Proton wine missing")
    def test_conformance_calls_every_slot_under_wine(self):
        exe = self.out / "conformance.exe"
        r = subprocess.run([CC, "-Wall", "-I", str(self.out), str(ROOT / "tests/conformance.c"), str(self.out / "ifaces.c"), "-o", str(exe)],
                           capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr[:2000])
        env = {"PATH": "/usr/bin:/bin", "WINEPREFIX": str(PREFIX), "WINEDEBUG": "-all", "HOME": str(Path.home())}
        run = subprocess.run([str(WINE), str(exe)], capture_output=True, text=True, env=env, timeout=240, cwd=self.out)
        m = re.search(r"OK interfaces=(\d+) calls=(\d+) unimplemented=(\d+) redirected=1", run.stdout)
        self.assertIsNotNone(m, run.stdout[-500:] + run.stderr[-500:])
        interfaces, calls, unimpl = map(int, m.groups())
        self.assertEqual(interfaces, len(self.catalog["interfaces"]))
        self.assertEqual(calls, unimpl)
        self.assertGreater(calls, 5000)

if __name__ == "__main__":
    unittest.main()
