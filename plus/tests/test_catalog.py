import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from tools.catalog import extract, merge

STEAM = Path.home() / ".local/share/Steam"
DLL = STEAM / "steamclient64.dll"
LSC = STEAM / "compatibilitytools.d/GE-Proton11-12/files/lib/wine/x86_64-unix/lsteamclient.so"
have_refs = DLL.exists() and LSC.exists()

DIS = """00000000000dcdd0 <ISteamUser_SteamUser023_GetSteamID>:
   dcdd0:\tpush   %rbp
   dcde6:\tcall   *0x10(%rdx)
00000000000dce00 <ISteamUser_SteamUser023_BLoggedOn>:
   dce00:\tmov    (%rdi),%rax
   dce0c:\tcall   *0x8(%rax)
00000000000dce40 <ISteamUser_SteamUser023_GetHSteamUser>:
   dce40:\tcall   *(%rax)
00000000000fa3c0 <ISteamMatchmakingServers_SteamMatchMakingServers002_GetServerCount>:
   fa3c7:\tmov    %rdi,%rbx
   fa3ce:\tmov    (%rdi),%rdi
   fa3d5:\tmov    (%rdi),%rdx
   fa3d8:\tmov    0x58(%rdx),%rdx
   fa3dc:\ttest   %rax,%rax
   fa3e5:\tcall   *%rdx
00000000000fa350 <ISteamMatchmakingServers_SteamMatchMakingServers002_IsRefreshing>:
   fa35e:\tmov    (%rdi),%rdi
   fa365:\tmov    (%rdi),%rdx
   fa368:\tmov    0x50(%rdx),%rdx
   fa375:\tcall   *%rdx
00000000000fa400 <ISteamMatchmakingServers_SteamMatchMakingServers002_ByFirstSlot>:
   fa401:\tmov    (%rdi),%rax
   fa404:\tmov    (%rax),%rdx
   fa407:\tcall   *%rdx
00000000000dce80 <ISteamUser_SteamUser023_NoCall>:
   dce80:\tret
00000000000dcf00 <not_an_interface_symbol>:
   dcf00:\tcall   *0x30(%rax)
"""

DIS32 = """00054300 <ISteamUser_SteamUser023_GetAuthSessionTicket>:
   54300:\tpush   %ebp
   54301:\tmov    %esp,%ebp
   54303:\tpush   %ebx
   5430e:\tpush   0x18(%ebx)
   54311:\tpush   0x14(%ebx)
   54314:\tpush   0x10(%ebx)
   54317:\tpush   0xc(%ebx)
   5431a:\tpush   %eax
   5431b:\tcall   *0x34(%edx)
   54327:\tret
000540f0 <ISteamUser_SteamUser023_GetSteamID>:
   540f0:\tpush   %ebp
   540f3:\tpush   %ebx
   54104:\tpush   %eax
   54105:\tpush   %ecx
   54106:\tcall   *0x8(%edx)
   5411a:\tret
00058340 <ISteamUserStats_STEAMUSERSTATS_INTERFACE_VERSION012_SetStat_2>:
   58341:\tpush   %ebp
   58343:\tpush   %ebx
   5834e:\tpush   0xd(%ebx)
   58351:\tpush   0x9(%ebx)
   58354:\tpush   %eax
   58355:\tcall   *0x10(%edx)
   58361:\tret
00021e00 <ISteamFriends_SteamFriends017_GetFriendPersonaName>:
   21e01:\tpush   %ebp
   21e03:\tpush   %esi
   21e04:\tpush   %ebx
   21e08:\tcall   e9e0 <__x86.get_pc_thunk.bx>
   21e1a:\tpush   0x1c(%esi)
   21e1d:\tpush   0x18(%esi)
   21e20:\tpush   %eax
   21e21:\tcall   *0x1c(%edx)
   21e58:\tret
00021f00 <ISteamUser_SteamUser023_BLoggedOn>:
   21f01:\tpush   %ebp
   21f10:\tpush   %eax
   21f11:\tcall   *0x4(%edx)
   21f20:\tret
00022000 <ISteamUser_SteamUser023_Weird>:
   22001:\tpush   %ebp
   22010:\tcall   1000 <memcpy@plt>
   22015:\tpush   %eax
   22016:\tcall   *0x4(%edx)
   22020:\tret
00062020 <ISteamUtils_SteamUtils010_GetAPICallResult>:
   62020:\tpush   %ebp
   62021:\tmov    %esp,%ebp
   62023:\tpush   %edi
   62026:\tcall   e9e0 <__x86.get_pc_thunk.bx>
   62045:\tpush   $0x0
   62047:\tpush   %eax
   62048:\tpush   0x11(%esi)
   6204e:\tcall   694c0 <alloc_callback_wtou>
   62053:\tadd    $0x10,%esp
   62064:\tpush   0x1d(%esi)
   62067:\tpush   0x19(%esi)
   6206a:\tpush   -0x1c(%ebp)
   6206d:\tpush   %edi
   6206e:\tpush   0xd(%esi)
   62071:\tpush   0x9(%esi)
   62074:\tpush   %edx
   62075:\tcall   *0x34(%eax)
   620b2:\tret
00064300 <ISteamMatchmakingServers_SteamMatchMakingServers002_GetServerCount>:
   64301:\tpush   %ebp
   64303:\tpush   %esi
   643eb:\tsub    $0x8,%esp
   643ee:\tpush   %edx
   643ef:\tpush   %eax
   643f0:\tcall   *%esi
   643fd:\tret
00022100 <ISteamUser_SteamUser023_NoCall>:
   22101:\tret
"""

class ArgBytesTest(unittest.TestCase):
    def test_argument_bytes_are_pushed_dwords_minus_this(self):
        got = extract.parse_thunk_argbytes_i386(DIS32)
        self.assertEqual(got["ISteamUser_SteamUser023_GetAuthSessionTicket"], 16)
        self.assertEqual(got["ISteamUser_SteamUser023_GetSteamID"], 4)
        self.assertEqual(got["ISteamUserStats_STEAMUSERSTATS_INTERFACE_VERSION012_SetStat_2"], 8)
        self.assertEqual(got["ISteamFriends_SteamFriends017_GetFriendPersonaName"], 8)
        self.assertEqual(got["ISteamUser_SteamUser023_BLoggedOn"], 0)

    def test_conversions_before_the_call_do_not_count_and_no_call_is_unknown(self):
        got = extract.parse_thunk_argbytes_i386(DIS32)
        self.assertEqual(got["ISteamUser_SteamUser023_Weird"], 0)
        self.assertEqual(got["ISteamUtils_SteamUtils010_GetAPICallResult"], 24)
        self.assertEqual(got["ISteamMatchmakingServers_SteamMatchMakingServers002_GetServerCount"], 4)
        self.assertIsNone(got["ISteamUser_SteamUser023_NoCall"])

    def test_add_argbytes_sets_the_field_per_method(self):
        cat = {"schema": 1, "interfaces": [{"name": "SteamUser023", "complete": True,
               "slots": [{"slot": 2, "name": "GetSteamID", "sig": None}, {"slot": 1, "name": "BLoggedOn", "sig": None}]}]}
        out = extract.add_argbytes(cat, extract.parse_thunk_argbytes_i386(DIS32))
        by = {s["name"]: s.get("argbytes32") for s in out["interfaces"][0]["slots"]}
        self.assertEqual(by, {"GetSteamID": 4, "BLoggedOn": 0})
        self.assertNotIn("argbytes32", cat["interfaces"][0]["slots"][0])

@unittest.skipUnless(have_refs and (STEAM / "compatibilitytools.d/GE-Proton11-12/files/lib/wine/i386-unix/lsteamclient.so").exists(), "references missing")
class ArgBytesRealTest(unittest.TestCase):
    def test_real_i386_thunks_give_known_sizes_and_cover_most_methods(self):
        import subprocess
        lsc32 = STEAM / "compatibilitytools.d/GE-Proton11-12/files/lib/wine/i386-unix/lsteamclient.so"
        dis = subprocess.run(["objdump", "-d", "--no-show-raw-insn", str(lsc32)], capture_output=True, text=True, check=True).stdout
        got = extract.parse_thunk_argbytes_i386(dis)
        self.assertEqual(got["ISteamUser_SteamUser023_GetAuthSessionTicket"], 16)
        self.assertEqual(got["ISteamUtils_SteamUtils009_IsAPICallCompleted"], 12)
        known = sum(v is not None for v in got.values())
        self.assertGreater(known / len(got), 0.9, f"{known}/{len(got)}")

class ParseTest(unittest.TestCase):
    def test_parse_symbol_splits_type_version_method(self):
        self.assertEqual(extract.parse_symbol("ISteamUser_SteamUser023_BLoggedOn"), ("ISteamUser", "SteamUser023", "BLoggedOn"))
        self.assertEqual(extract.parse_symbol("ISteamApps_STEAMAPPS_INTERFACE_VERSION009_BIsDlcInstalled"),
                         ("ISteamApps", "STEAMAPPS_INTERFACE_VERSION009", "BIsDlcInstalled"))
        self.assertIsNone(extract.parse_symbol("not_an_interface_symbol"))

    def test_thunk_slot_is_first_indirect_call_displacement_over_8(self):
        slots = extract.parse_thunk_slots(DIS)
        self.assertEqual(slots["ISteamUser_SteamUser023_GetSteamID"], 2)
        self.assertEqual(slots["ISteamUser_SteamUser023_BLoggedOn"], 1)
        self.assertEqual(slots["ISteamUser_SteamUser023_GetHSteamUser"], 0)
        self.assertIsNone(slots["ISteamUser_SteamUser023_NoCall"])
        self.assertEqual(slots["ISteamMatchmakingServers_SteamMatchMakingServers002_GetServerCount"], 11)
        self.assertEqual(slots["ISteamMatchmakingServers_SteamMatchMakingServers002_IsRefreshing"], 10)
        self.assertEqual(slots["ISteamMatchmakingServers_SteamMatchMakingServers002_ByFirstSlot"], 0)
        self.assertNotIn("not_an_interface_symbol", slots)

    def test_build_marks_duplicate_slots_null_and_incomplete(self):
        cat = extract.build_catalog(["IfA001", "IfB001", "IfC001"], {
            "IfA001": {"M0": 0, "M1": 1},
            "IfB001": {"X": 3, "Y": 3, "Z": 0},
        }, source={"dll": "d", "sha256": "s"})
        by = {i["name"]: i for i in cat["interfaces"]}
        self.assertTrue(by["IfA001"]["complete"])
        self.assertFalse(by["IfB001"]["complete"])
        self.assertEqual({s["name"]: s["slot"] for s in by["IfB001"]["slots"]}, {"X": None, "Y": None, "Z": 0})
        self.assertIsNone(by["IfC001"]["slots"])
        self.assertFalse(by["IfC001"]["complete"])

class MergeTest(unittest.TestCase):
    def test_override_sets_signature_and_beats_existing(self):
        cat = {"schema": 1, "interfaces": [{"name": "SteamUser023", "complete": True,
               "slots": [{"slot": 2, "name": "GetSteamID", "sig": {"ret": "old", "args": []}}]}]}
        d = Path(tempfile.mkdtemp())
        (d / "user.json").write_text(json.dumps({"SteamUser023": {"GetSteamID": {"ret": "uint64", "args": ["void*"]}}}))
        out = merge.merge_signatures(cat, None, d)
        self.assertEqual(out["interfaces"][0]["slots"][0]["sig"], {"ret": "uint64", "args": ["void*"]})

@unittest.skipUnless(have_refs, "reference binaries missing")
class RealReferenceTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.cat = extract.extract_catalog(DLL, LSC)

    def test_extract_finds_interfaces(self):
        names = [i["name"] for i in self.cat["interfaces"]]
        self.assertTrue(150 <= len(names) <= 300, len(names))
        self.assertIn("SteamUser023", names)
        self.assertIn("STEAMAPPS_INTERFACE_VERSION009", names)

    def test_known_user_slots(self):
        u = next(i for i in self.cat["interfaces"] if i["name"] == "SteamUser023")
        slot = {s["name"]: s["slot"] for s in u["slots"]}
        self.assertEqual((slot["GetHSteamUser"], slot["BLoggedOn"], slot["GetSteamID"]), (0, 1, 2))

    def test_slots_are_unique_in_every_interface(self):
        for i in self.cat["interfaces"]:
            vals = [s["slot"] for s in (i["slots"] or []) if s["slot"] is not None]
            self.assertEqual(len(vals), len(set(vals)), i["name"])

    def test_source_records_hash_of_reference_dll(self):
        self.assertEqual(self.cat["source"]["sha256"], hashlib.sha256(DLL.read_bytes()).hexdigest())

    def test_catalog_has_no_valve_code(self):
        blob = json.dumps(self.cat)
        self.assertNotIn("Valve Corporation", blob)
        for i in self.cat["interfaces"]:
            self.assertEqual(set(i), {"name", "complete", "slots"})

if __name__ == "__main__":
    unittest.main()

PE_NM = """10001000 T _winISteamUser_SteamUser023_GetSteamID
10001100 T _winISteamUser_SteamUser023_BLoggedOn
10001200 T _winISteamUser_SteamUser023_GetAuthSessionTicket
10001300 T _winISteamUserStats_STEAMUSERSTATS_INTERFACE_VERSION012_GetStat_2
1013ff84 D _winISteamUser_SteamUser023_vtable
1013ff94 D _winISteamUserStats_STEAMUSERSTATS_INTERFACE_VERSION012_vtable
10149540 d .data$winISteamUser_SteamUser023_type_info
"""
PE_DIS = """10001000 <_winISteamUser_SteamUser023_GetSteamID>:
10001000:\tpush   %ebp
10001010:\tpop    %ebp
10001011:\tret    $0x4
10001100 <_winISteamUser_SteamUser023_BLoggedOn>:
10001100:\tpush   %ebp
10001110:\tret
10001200 <_winISteamUser_SteamUser023_GetAuthSessionTicket>:
10001200:\tpush   %ebp
10001230:\tret    $0x10
10001300 <_winISteamUserStats_STEAMUSERSTATS_INTERFACE_VERSION012_GetStat_2>:
10001300:\tret    $0x8
"""

class VersionNameTest(unittest.TestCase):
    def test_name_pattern_covers_underscored_and_v_style_versions(self):
        data = b"\x00STEAMHTMLSURFACE_INTERFACE_VERSION_005\x00junk\x00STEAMINVENTORY_INTERFACE_V003\x00STEAMVIDEO_INTERFACE_V007\x00SteamUser023\x00STEAMAPPS_INTERFACE_VERSION008\x00"
        got = {m.group(1).decode() for m in extract.VERSION_NAME.finditer(data)}
        self.assertEqual(got, {"STEAMHTMLSURFACE_INTERFACE_VERSION_005", "STEAMINVENTORY_INTERFACE_V003", "STEAMVIDEO_INTERFACE_V007",
                               "SteamUser023", "STEAMAPPS_INTERFACE_VERSION008"})

class PeVtableTest(unittest.TestCase):
    @staticmethod
    def memory(vtables):
        mem = {}
        for base, words in vtables.items():
            for i, w in enumerate(words):
                mem[base + 4 * i] = w
        return lambda addr, n: [mem.get(addr + 4 * i, 0) for i in range(n)]

    def test_vtables_give_slot_order_names_and_ret_sizes(self):
        read = self.memory({0x1013ff84: [0x10001100, 0x10001000, 0x10001200], 0x1013ff94: [0x10001300]})
        got = extract.parse_pe_vtables(PE_NM, PE_DIS, read)
        self.assertEqual(got["SteamUser023"], [("BLoggedOn", 0), ("GetSteamID", 4), ("GetAuthSessionTicket", 16)])
        self.assertEqual(got["STEAMUSERSTATS_INTERFACE_VERSION012"], [("GetStat_2", 8)])

    def test_vtable_stops_at_the_first_word_that_is_not_a_known_method(self):
        read = self.memory({0x1013ff84: [0x10001000, 0x10001100, 0xDEADBEEF, 0x10001200], 0x1013ff94: [0x10001300]})
        got = extract.parse_pe_vtables(PE_NM, PE_DIS, read)
        self.assertEqual([m for m, _ in got["SteamUser023"]], ["GetSteamID", "BLoggedOn"])

@unittest.skipUnless((STEAM / "compatibilitytools.d/GE-Proton11-12/files/lib/wine/i386-windows/lsteamclient.dll").exists(), "reference missing")
class PeVtableRealTest(unittest.TestCase):
    def test_real_windows_vtables_match_the_known_msvc_layouts(self):
        pe = STEAM / "compatibilitytools.d/GE-Proton11-12/files/lib/wine/i386-windows/lsteamclient.dll"
        vt = extract.extract_pe_vtables(pe)
        mms = dict(vt["SteamMatchMakingServers002"])
        names = [m for m, _ in vt["SteamMatchMakingServers002"]]
        self.assertEqual(names[:3], ["RequestInternetServerList", "RequestLANServerList", "RequestFriendsServerList"])
        self.assertEqual(mms["ReleaseRequest"], 4)
        self.assertEqual(mms["GetServerDetails"], 8)
        user = dict(vt["SteamUser023"])
        self.assertEqual(user["GetAuthSessionTicket"], 16)
        self.assertGreater(len(vt), 150)

class CatalogFromPeTest(unittest.TestCase):
    def test_slots_follow_the_windows_vtable_and_carry_ret_sizes(self):
        vt = {"IfaceA001": [("Foo", 4), ("Bar", 0)], "IfaceB001": [("X", 12)]}
        cat = extract.build_catalog_from_pe(["IfaceA001", "IfaceB001", "IfaceC001"], vt, source={"dll": "d"})
        by = {i["name"]: i for i in cat["interfaces"]}
        self.assertTrue(by["IfaceA001"]["complete"])
        self.assertEqual([(s["slot"], s["name"], s["argbytes32"]) for s in by["IfaceA001"]["slots"]], [(0, "Foo", 4), (1, "Bar", 0)])
        self.assertEqual(by["IfaceB001"]["slots"][0]["sig"], None)
        self.assertIsNone(by["IfaceC001"]["slots"])
        self.assertFalse(by["IfaceC001"]["complete"])

DIS_RET = """00001000 <ISteamUser_SteamUser013_BLoggedOn>:
   1000:\tcall   *0x4(%edx)
   1003:\tmov    %al,0x8(%ebx)
   1006:\txor    %eax,%eax
00001100 <ISteamUser_SteamUser023_GetAuthSessionTicket>:
   1100:\tcall   *0x34(%edx)
   1103:\tmov    %eax,0x8(%ebx)
   1106:\txor    %eax,%eax
   1108:\tmov    -0x4(%ebp),%ebx
00001200 <ISteamMatchmaking_SteamMatchMaking007_RequestLobbyList>:
   1200:\tcall   *0x10(%edx)
   1203:\tmov    %eax,0x8(%ebx)
   1206:\txor    %eax,%eax
   1208:\tmov    %edx,0xc(%ebx)
00001300 <ISteamUser_SteamUser013_GetSteamID>:
   1300:\tcall   *0x8(%edx)
   1303:\tmov    -0x10(%ebp),%eax
   1306:\tmov    -0xc(%ebp),%edx
   1309:\tmov    %eax,(%ebx)
00001400 <ISteamUser_SteamUser013_TerminateGameConnection>:
   1400:\tcall   *0x10(%edx)
   1403:\tmov    -0x4(%ebp),%ebx
   1406:\txor    %eax,%eax
   1408:\tleave
"""

class RetKindTest(unittest.TestCase):
    def test_return_kind_comes_from_what_the_thunk_stores_after_the_call(self):
        got = extract.parse_thunk_ret_kinds_i386(DIS_RET)
        self.assertEqual(got["ISteamUser_SteamUser013_BLoggedOn"], "bool")
        self.assertEqual(got["ISteamUser_SteamUser023_GetAuthSessionTicket"], "word")
        self.assertEqual(got["ISteamMatchmaking_SteamMatchMaking007_RequestLobbyList"], "qword")
        self.assertEqual(got["ISteamUser_SteamUser013_GetSteamID"], "struct8")
        self.assertEqual(got["ISteamUser_SteamUser013_TerminateGameConnection"], "void")

    def test_add_ret_kinds_sets_the_field_per_method(self):
        cat = {"schema": 1, "interfaces": [{"name": "SteamUser013", "complete": True,
               "slots": [{"slot": 1, "name": "BLoggedOn", "sig": None}, {"slot": 2, "name": "GetSteamID", "sig": None}]}]}
        out = extract.add_ret_kinds(cat, extract.parse_thunk_ret_kinds_i386(DIS_RET))
        self.assertEqual([s["ret"] for s in out["interfaces"][0]["slots"]], ["bool", "struct8"])
        self.assertNotIn("ret", cat["interfaces"][0]["slots"][0])

    def test_qword_is_an_async_call_unless_it_is_a_known_handle_method(self):
        cat = {"schema": 1, "interfaces": [{"name": "SteamTest001", "complete": True, "slots": [
            {"slot": 0, "name": "RequestLobbyList", "sig": None}, {"slot": 1, "name": "GetFileTimestamp", "sig": None}]}]}
        kinds = {"ISteamTest_SteamTest001_RequestLobbyList": "qword", "ISteamTest_SteamTest001_GetFileTimestamp": "qword"}
        out = extract.add_ret_kinds(cat, kinds)
        self.assertEqual([s["ret"] for s in out["interfaces"][0]["slots"]], ["call", "qword"])
