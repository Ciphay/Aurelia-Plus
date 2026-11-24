import json
import tempfile
import unittest
from pathlib import Path

from tools.policy import gen_policy

ROOT = Path(__file__).resolve().parent.parent

class GenPolicyTest(unittest.TestCase):
    def test_each_reviewed_method_gets_one_registration_with_the_right_function(self):
        c = gen_policy.generate({"SteamUtils": {"zero": ["IsOverlayEnabled"], "true": ["InitFilterText"]},
                                 "SteamUser": {"struct8": ["GetSteamID"], "empty": ["GetName"], "void": ["LogOff"]}})
        self.assertIn('shim_register_family("SteamUtils", "IsOverlayEnabled", (void *)p_zero);', c)
        self.assertIn('shim_register_family("SteamUtils", "InitFilterText", (void *)p_true);', c)
        self.assertIn('shim_register_family("SteamUser", "GetSteamID", (void *)p_struct8);', c)
        self.assertIn('shim_register_family("SteamUser", "GetName", (void *)p_empty);', c)
        self.assertIn('shim_register_family("SteamUser", "LogOff", (void *)p_void);', c)

    def test_hidden_struct_kinds_are_registered_with_their_functions(self):
        c = gen_policy.generate({"SteamInput": {"hidden2": ["GetDigitalActionData"], "hidden16": ["GetAnalogActionData"], "hidden40": ["GetMotionData"]}})
        self.assertIn('"GetDigitalActionData", (void *)p_hidden2);', c)
        self.assertIn('"GetAnalogActionData", (void *)p_hidden16);', c)
        self.assertIn('"GetMotionData", (void *)p_hidden40);', c)
        self.assertIn("memset(ret, 0, 40)", c)

    def test_constant_return_kinds_get_their_own_function(self):
        c = gen_policy.generate({"SteamMatchGameSearch": {"ret:7": ["SearchForGameSolo"], "ret:-1": ["Other"]}})
        self.assertIn("p_ret_7(void *self)", c)
        self.assertIn("p_ret_m1(void *self)", c)
        self.assertIn('"SearchForGameSolo", (void *)p_ret_7);', c)
        self.assertIn('"Other", (void *)p_ret_m1);', c)

    def test_unknown_kind_is_rejected(self):
        with self.assertRaises(ValueError):
            gen_policy.generate({"X": {"maybe": ["M"]}})

    def test_generation_is_deterministic(self):
        pol = {"B": {"zero": ["z", "a"]}, "A": {"true": ["m"]}}
        self.assertEqual(gen_policy.generate(pol), gen_policy.generate(json.loads(json.dumps(pol))))
        self.assertLess(gen_policy.generate(pol).index('"A"'), gen_policy.generate(pol).index('"B"'))

class SurfaceCoverageTest(unittest.TestCase):

    def test_no_method_of_the_catalog_is_left_unreviewed(self):
        from tools.policy import status
        catalog = json.loads((ROOT / "catalog/catalog.json").read_text())
        policy = json.loads((ROOT / "catalog/policy.json").read_text())
        res = status.classify(catalog, policy, status.implemented_methods(ROOT / "shim/ifaces"))
        pending = {t: sorted(m for m, k in methods.items() if k == "pending") for t, methods in res.items()}
        pending = {t: m for t, m in pending.items() if m}
        self.assertEqual(pending, {}, f"methods without review: {pending}")

    def test_generated_policy_matches_the_json(self):
        policy = json.loads((ROOT / "catalog/policy.json").read_text())
        self.assertEqual((ROOT / "shim/generated/policy.c").read_text(), gen_policy.generate(policy))

if __name__ == "__main__":
    unittest.main()
