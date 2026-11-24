import hashlib
import json
import re
import time
import socket
import struct
import threading
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CC = shutil.which("x86_64-w64-mingw32-gcc")
WINE = Path.home() / ".local/share/Steam/compatibilitytools.d/GE-Proton11-12/files/bin/wine"
PREFIX = Path.home() / ".cache/aurelia-plus/wine-prefix"
SOURCES = [*sorted((ROOT / "shim").glob("*.c")), *sorted((ROOT / "shim/ifaces").glob("*.c")), ROOT / "shim/generated/ifaces.c", ROOT / "shim/generated/policy.c"]
EXPECTED = [
    "frame_encoding_matches_rust_protocol", "call_frames_match_the_rust_protocol_and_fail_fast_offline", "snapshot_roundtrip_matches_fixture",
    "create_interface_unknown_name_returns_null_and_logs", "user_get_steamid_matches_snapshot",
    "bridge_down_keeps_cached_snapshot_and_does_not_block", "apps_subscription_follows_snapshot",
    "utils_reports_appid_and_country", "auth_ticket_has_inferred_layout", "run_frame_stubs_are_silent_noops",
    "overwatch_first_calls_are_implemented", "callback_queue_answers_ticket_and_stats_requests",
    "ticket_file_override_replaces_the_built_ticket",
    "auth_ticket_identity_argument_is_logged",
    "auth_ticket_works_for_steamuser022_without_identity",
    "trace_logs_implemented_calls_when_enabled",
    "overwatch_post_login_calls_are_implemented", "friends_come_from_the_snapshot", "networking_utils_clock_relay_and_unavailable_sockets", "input_without_controllers_answers_inactive_data", "friend_game_played_variants", "screenshots_are_written_as_png_and_tracked", "utils_apps_applist_and_client_extras", "user_connection_auth_session_and_gameserver_local_state", "unimplemented_methods_answer_by_return_kind", "loopback_sockets_connect_inside_the_process", "player_count_and_clans_answer_without_inventing_data", "matchmaking_servers_without_a_bridge_fail_or_come_up_empty", "remote_storage_files_live_in_the_cloud_dir", "call_results_complete_through_703_and_get_api_call_result", "legacy_steamclient_exports_follow_the_real_dll", "extras_answer_with_the_real_result_types", "protobuf_wire_format_round_trips",
]

SNAPSHOT = {"schema": 1, "account": {"steamid": 76561198000000001, "persona": "Tester", "country": "BR"},
            "apps": [{"appid": 2357570, "name": "Overwatch", "installed": True}],
            "friends": [{"steamid": 76561198000000042, "persona": "Ana", "state": 1, "game_appid": 440}],
            "ticket": {"appid": 2357570, "ownership_ticket_b64": "AQID", "game_connect_token_b64": "AQIDBAUGBwgBTF4CAQAQARESExQ="}}

class FakeBridge:

    def __init__(self, ticket_status=0, stats_reply=None, relay=None, ugc=None, cloudfile=None, gameserver=None, steamids=None, overlay=None, snapshot_extra=None, connected=None, flip_after=None, appsq=None, slots=None):
        self.ticket_status = ticket_status
        self.slots = slots or {}
        self.appsq = appsq
        self.flip_after = flip_after
        self.connected = connected
        self.stats_reply = stats_reply
        self.steamids = list(steamids or [])
        self.mailbox = {}
        self.gameserver = gameserver
        self.cloudfile = cloudfile
        self.snapshot_extra = snapshot_extra or {}
        self.overlay = overlay
        self.ugc = ugc
        self.relay = relay
        self.relayed = []
        self.pushes = []
        self.subscribed = []
        self.token = bytes(range(32))
        self.calls = []
        self.sock = socket.socket()
        self.sock.bind(("127.0.0.1", 0))
        self.sock.listen(8)
        self.port = self.sock.getsockname()[1]
        threading.Thread(target=self._serve, daemon=True).start()

    def _relay(self, slot, p, sid=None):
        if slot == 8:
            self.subscribed.append(struct.unpack_from("<i", p)[0])
            return 0, b""
        if slot == 9:
            if sid is not None and sid in self.mailbox:
                box, self.mailbox[sid] = self.mailbox[sid], []
                return 0, b"".join(struct.pack("<iI", e, len(b)) + b for e, b in box)
            out, self.pushes = b"".join(struct.pack("<iI", e, len(b)) + b for e, b in self.pushes), []
            return 0, out
        flags, emsg, _wait, nlen = p[0], struct.unpack_from("<i", p, 4)[0], struct.unpack_from("<I", p, 8)[0], struct.unpack_from("<H", p, 12)[0]
        job, body = p[14:14 + nlen].decode(), p[14 + nlen:]
        self.relayed.append((emsg, job, body))
        if emsg == 5434 and self.mailbox:
            dest = dict(pb_fields(body)).get(1)
            self.mailbox.setdefault(dest, []).append((5434, body))
        self.relayed_gs = getattr(self, 'relayed_gs', [])
        if flags & 8:
            self.relayed_gs.append(emsg)
        reply = self.relay(emsg, job, body) if self.relay else None
        if not flags & 1:
            return 1, b""
        if reply is None:
            return -5, b""
        return 0, struct.pack("<ii", reply[0], reply[1]) + reply[2]

    @staticmethod
    def _frame(body):
        return struct.pack("<I", len(body)) + body

    def _serve(self):
        while True:
            conn, _ = self.sock.accept()
            threading.Thread(target=self._conn, args=(conn,), daemon=True).start()

    def _conn(self, conn):
        buf = b""
        sid = None
        while True:
            while len(buf) < 4 or len(buf) < 4 + struct.unpack_from("<I", buf)[0]:
                chunk = conn.recv(65536)
                if not chunk:
                    return
                buf += chunk
            n = struct.unpack_from("<I", buf)[0]
            body, buf = buf[4:4 + n], buf[4 + n:]
            tag = body[0]
            if tag == 1:
                if body[3:35] != self.token:
                    conn.close()
                    return
                conn.sendall(self._frame(b"\x02" + struct.pack("<H", 1)))
            elif tag == 3:
                snap = {**SNAPSHOT, **self.snapshot_extra}
                if self.steamids:
                    sid = self.steamids.pop(0)
                    snap = {**snap, "account": {**SNAPSHOT["account"], "steamid": sid}}
                    self.mailbox.setdefault(sid, [])
                conn.sendall(self._frame(b"\x04" + json.dumps(snap).encode()))
            elif tag == 5:
                seq, iface, slot = struct.unpack_from("<IHH", body, 1)
                self.calls.append((iface, slot, body[9:]))
                if iface == 0xFFFF and slot == 12 and self.gameserver:
                    reply = self.gameserver(body[9])
                    conn.sendall(self._frame(b"\x06" + struct.pack("<Ii", seq, 0 if reply is not None else -5) + (reply or b"")))
                    continue
                if iface == 0xFFFF and slot == 11 and self.cloudfile:
                    op, app, nl = struct.unpack_from("<BIH", body, 9)
                    name = body[16:16 + nl].decode()
                    reply = self.cloudfile(op, app, name, body[16 + nl:])
                    conn.sendall(self._frame(b"\x06" + struct.pack("<Ii", seq, 0 if reply is not None else -5) + (reply or b"")))
                    continue
                if iface == 0xFFFF and slot == 13 and self.overlay:
                    reply = self.overlay(body[9], body[9:])
                    conn.sendall(self._frame(b"\x06" + struct.pack("<Ii", seq, 0 if reply is not None else -5) + (reply or b"")))
                    continue
                if iface == 0xFFFF and slot == 10 and self.ugc:
                    op, app, fid = struct.unpack_from("<BIQ", body, 9)
                    reply = self.ugc(op, app, fid)
                    conn.sendall(self._frame(b"\x06" + struct.pack("<Ii", seq, 0 if reply is not None else -5) + (reply or b"")))
                    continue
                if iface == 0xFFFF and slot in self.slots:
                    reply = self.slots[slot](body[9:])
                    conn.sendall(self._frame(b"\x06" + struct.pack("<Ii", seq, 0 if reply is not None else -5) + (reply or b"")))
                    continue
                if iface == 0xFFFF and slot == 16 and self.appsq:
                    reply = self.appsq(body[9], struct.unpack_from("<I", body, 10)[0])
                    conn.sendall(self._frame(b"\x06" + struct.pack("<Ii", seq, 0 if reply is not None else -5) + (reply or b"")))
                    continue
                if iface == 0xFFFF and slot == 15 and self.connected is not None:
                    conn.sendall(self._frame(b"\x06" + struct.pack("<Ii", seq, 0) + bytes([1 if self.connected else 0])))
                    if self.flip_after is not None:
                        self.flip_after -= 1
                        if self.flip_after <= 0:
                            self.connected = True
                    continue
                if iface == 0xFFFF and slot in (7, 8, 9):
                    status, payload = self._relay(slot, body[9:], sid)
                    conn.sendall(self._frame(b"\x06" + struct.pack("<Ii", seq, status) + payload))
                    continue
                if iface == 0xFFFF and slot in (4, 5) and self.stats_reply is not None:
                    reply = self.stats_reply if slot == 4 else b""
                    conn.sendall(self._frame(b"\x06" + struct.pack("<Ii", seq, 1) + reply))
                    continue
                refused = slot == 3 and self.ticket_status < 0
                payload = b"TICKET-FROM-BRIDGE" if slot == 3 and not refused else b""
                status = self.ticket_status if slot == 3 else 0
                conn.sendall(self._frame(b"\x06" + struct.pack("<Ii", seq, status) + payload))

def wine_args(args, cwd, extra_env=None):
    env = {"PATH": "/usr/bin:/bin", "WINEPREFIX": str(PREFIX), "WINEDEBUG": "-all", "HOME": str(Path.home()), **(extra_env or {})}
    return subprocess.run([str(WINE), *map(str, args)], capture_output=True, text=True, env=env, timeout=240, cwd=cwd)

def wine(exe, cwd):
    env = {"PATH": "/usr/bin:/bin", "WINEPREFIX": str(PREFIX), "WINEDEBUG": "-all", "HOME": str(Path.home())}
    return subprocess.run([str(WINE), str(exe)], capture_output=True, text=True, env=env, timeout=240, cwd=cwd)

class _Arch:
    ARCH = "win64"
    CC = CC
    DLL = "steamclient64.dll"

    @classmethod
    def sources(cls, out):
        if cls.ARCH == "win64":
            return SOURCES
        gen = Path(out) / "generated32"
        subprocess.run(["python3", "-m", "tools.gen.emit_shim", "--catalog", "catalog/catalog.json", "--abi", "win32", "--out", str(gen)],
                       cwd=ROOT, check=True, capture_output=True)
        subprocess.run(["python3", "-m", "tools.policy.gen_policy", "--policy", "catalog/policy.json", "--out", str(gen / "policy.c")],
                       cwd=ROOT, check=True, capture_output=True)
        return [*sorted((ROOT / "shim").glob("*.c")), *sorted((ROOT / "shim/ifaces").glob("*.c")), gen / "ifaces.c", gen / "policy.c"]

    @classmethod
    def _source_hash(cls):
        h = hashlib.sha256(cls.ARCH.encode())
        files = [*sorted((ROOT / "shim").rglob("*.[ch]")), ROOT / "tests/selftest.c", ROOT / "catalog/catalog.json", ROOT / "catalog/policy.json"]
        for f in files:
            h.update(str(f).encode())
            h.update(f.read_bytes())
        return h.hexdigest()[:20]

    def build_selftest(self, out):
        exe = Path(out) / "selftest.exe"
        cached = Path(tempfile.gettempdir()) / f"aurelia-plus-selftest-{self.ARCH}-{self._source_hash()}.exe"
        if cached.exists():
            shutil.copy2(cached, exe)
            return exe
        gen = ROOT / "shim/generated" if self.ARCH == "win64" else Path(out) / "generated32"
        srcs = self.sources(out)
        cmd = [self.CC, "-Wall", "-Wextra", "-Wno-unused-parameter", "-I", str(ROOT / "shim"), "-I", str(gen),
               str(ROOT / "tests/selftest.c"), *map(str, srcs), "-lws2_32", "-lwinhttp", "-lbcrypt", "-lshell32", "-liphlpapi", "-o", str(exe)]
        r = subprocess.run(cmd, capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr[-3000:])
        shutil.copy2(exe, cached)
        return exe

class _Win64(_Arch):
    pass

class _Win32(_Arch):
    ARCH = "win32"
    CC = shutil.which("i686-w64-mingw32-gcc")
    DLL = "steamclient.dll"

def _both(cases):
    g = globals()
    base = cases.__name__.removesuffix("Cases")
    for arch, suffix in ((_Win64, "UnderWine"), (_Win32, "UnderWine32")):
        ok = bool(arch.CC and WINE.exists())
        g[base + suffix] = unittest.skipUnless(ok, "mingw or wine missing")(type(base + suffix, (cases, arch, unittest.TestCase), {}))
    return cases

@_both
class SelftestCases:
    def test_selftest_passes_every_case(self):
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        run = wine(exe, out)
        passed = re.findall(r"^PASS (\w+)$", run.stdout, re.M)
        self.assertEqual(passed, EXPECTED, run.stdout[-1500:] + run.stderr[-500:])
        self.assertEqual(run.returncode, 0)

@_both
class BridgeTicketCases:
    def test_ticket_comes_from_the_bridge_and_cancel_reaches_it(self):
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge()
        run = wine_args([exe], out, {"SELFTEST_BRIDGE": "1", "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn("PASS ticket_is_created_by_the_bridge", run.stdout, run.stdout[-800:] + run.stderr[-300:])
        calls = [c for c in bridge.calls if c[1] not in (4, 8, 9, 11, 14)]
        slots = [(iface, slot) for iface, slot, _ in calls]
        self.assertEqual(slots, [(0xFFFF, 3), (0xFFFF, 2)])
        appid, handle = struct.unpack("<II", calls[0][2][:8])
        self.assertEqual(appid, 2357570)
        self.assertEqual(struct.unpack("<I", calls[1][2][:4])[0], handle)

    def test_refusal_from_a_reachable_bridge_does_not_fall_back_to_the_snapshot_reserve(self):
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge(ticket_status=-3)
        run = wine_args([exe], out, {"SELFTEST_BRIDGE_REFUSE": "1", "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn("PASS bridge_refusal_does_not_fall_back", run.stdout, run.stdout[-800:] + run.stderr[-300:])
        self.assertEqual([(i, sl) for i, sl, _ in bridge.calls if sl not in (4, 8, 9, 11, 14, 15)], [(0xFFFF, 3)])

    def test_a_refusal_while_steam_is_offline_falls_back_to_the_snapshot_reserve(self):
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge(ticket_status=-3, connected=False, snapshot_extra={"online": False})
        run = wine_args([exe], out, {"SELFTEST_TICKET_OFFLINE": "1", "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn("PASS ticket_offline_uses_the_snapshot_reserve", run.stdout, run.stdout[-800:] + run.stderr[-300:])

def stats_reply():
    def name(n):
        raw = n.encode()
        return struct.pack("<H", len(raw)) + raw
    out = struct.pack("<II", 0xABCD, 3)
    out += struct.pack("<IIII", 1, 1, 42, 5) + name("kills")
    out += struct.pack("<IIII", 4, 3, 0, 0) + name("rate")
    out += struct.pack("<IIII", 2, 2, struct.unpack("<I", struct.pack("<f", 0.5))[0], struct.unpack("<I", struct.pack("<f", 0.5))[0]) + name("accuracy")
    out += struct.pack("<I", 2)
    out += struct.pack("<III", 3, 0, 1700000000) + name("ACH_FIRST")
    out += struct.pack("<III", 3, 1, 0) + name("ACH_SECOND")
    out += struct.pack("<I", 1)
    out += name("ACH_FIRST") + name("First") + name("Win once") + b"\x00" + struct.pack("<f", 12.5)
    return out

@_both
class UserStatsCases:
    def test_stats_come_from_the_bridge_and_changes_go_back(self):
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge(stats_reply=stats_reply())
        run = wine_args([exe], out, {"SELFTEST_STATS": "1", "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn("PASS user_stats_roundtrip", run.stdout, run.stdout[-800:] + run.stderr[-300:])
        stores = [p for i, sl, p in bridge.calls if (i, sl) == (0xFFFF, 5)]
        self.assertEqual(len(stores), 1)
        crc, n = struct.unpack_from("<II", stores[0])
        changes = sorted(struct.unpack_from("<II", stores[0], 8 + 8 * k) for k in range(n))
        self.assertEqual([c for c in changes if c[0] != 4], sorted([(1, 43), (2, struct.unpack("<I", struct.pack("<f", 0.75))[0]), (3, 0b11)]))
        self.assertEqual(_overlay_requests(bridge), [("achievement", {"api_name": "ACH_SECOND"})])

def echo_relay(emsg, job, body):
    if job == "Test.Echo#1":
        return 147, 1, body
    if emsg == 6602:
        return 6603, 1, b""
    return None

class NetCase:

    def run_net(self, name, relay, setup=None, env=None):
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge(relay=relay)
        if setup:
            setup(bridge)
        run = wine_args([exe], out, {"SELFTEST_NET": name, "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex(), **(env or {})})
        self.assertIn(f"PASS net_{name}", run.stdout, run.stdout[-1200:] + run.stderr[-400:])
        return run.stdout, bridge

@_both
class RelayCases(NetCase):
    def test_relay_calls_pushes_and_async_results(self):
        holder = {}

        def relay(emsg, job, body):
            if job == "Test.Push#1":
                holder["b"].pushes.append((6613, b"abc"))
                return 147, 1, b""
            return echo_relay(emsg, job, body)

        _, bridge = self.run_net("cm", relay, lambda b: holder.update(b=b))
        self.assertIn(6613, bridge.subscribed)
        self.assertIn((5432, "", bytes([0x08, 0x96, 0x01, 0x12, 0x02]) + b"hi"), bridge.relayed)

def pb_varint(n, v):
    return pb_raw(n << 3) + pb_raw(v & 0xFFFFFFFFFFFFFFFF)

def pb_raw(v):
    out = b""
    while v >= 0x80:
        out += bytes([v & 0x7F | 0x80])
        v >>= 7
    return out + bytes([v])

def pb_len(n, b):
    return pb_raw(n << 3 | 2) + pb_raw(len(b)) + b

def pb_fixed64(n, v):
    return pb_raw(n << 3 | 1) + struct.pack("<Q", v)

def pb_fields(b):
    out, o = [], 0

    def rv():
        nonlocal o
        v = shift = 0
        while True:
            c = b[o]
            o += 1
            v |= (c & 0x7F) << shift
            if not c & 0x80:
                return v
            shift += 7

    while o < len(b):
        key = rv()
        wt = key >> 3, key & 7
        if wt[1] == 0:
            out.append((wt[0], rv()))
        elif wt[1] == 1:
            out.append((wt[0], struct.unpack_from("<Q", b, o)[0])); o += 8
        elif wt[1] == 5:
            out.append((wt[0], struct.unpack_from("<I", b, o)[0])); o += 4
        else:
            n = rv()
            out.append((wt[0], b[o:o + n])); o += n
    return out

def leaderboard_relay(emsg, job, body):
    f = pb_fields(body)
    if emsg == 5416:
        name = dict(f)[5].decode()
        if name == "Nope":
            return 5417, 1, pb_varint(1, 9)
        return 5417, 1, pb_varint(1, 1) + pb_varint(2, 7) + pb_varint(3, 5) + pb_varint(4, 2) + pb_varint(5, 1) + pb_len(6, name.encode())
    if emsg == 5418:
        entry = pb_fixed64(1, 76561198000000042) + pb_varint(2, 1) + pb_varint(3, 100) + pb_len(4, struct.pack("<ii", 7, 8)) + pb_fixed64(5, 2**64 - 1)
        entry2 = pb_fixed64(1, 76561198000000043) + pb_varint(2, 2) + pb_varint(3, 90)
        return 5419, 1, pb_varint(1, 1) + pb_varint(2, 5) + pb_len(3, entry) + pb_len(3, entry2)
    if emsg == 5414:
        return 5415, 1, pb_varint(1, 1) + pb_varint(2, 6) + pb_varint(3, 1) + pb_varint(4, 5) + pb_varint(5, 2)
    if emsg == 5592:
        return 5593, 1, pb_varint(1, 1) + pb_varint(2, 777)
    if emsg == 7901:
        acc = struct.unpack("<q", struct.pack("<d", 0.25))[0]
        total = pb_varint(1, 0) + pb_len(2, pb_varint(1, 1) + pb_varint(2, 1000)) + pb_len(2, pb_varint(1, 2) + pb_varint(2, acc))
        days = b"".join(pb_len(4, pb_varint(1, 20773 - i) + pb_len(2, pb_varint(1, 1) + pb_varint(2, 30 - 10 * i))) for i in range(3))
        return 7902, 1, pb_varint(1, 1) + pb_len(4, total) + days
    return None

@_both
class LeaderboardCases(NetCase):
    def test_boards_scores_players_and_global_stats_use_the_relay(self):
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge(stats_reply=stats_reply(), relay=leaderboard_relay)
        run = wine_args([exe], out, {"SELFTEST_NET": "leaderboards", "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn("PASS net_leaderboards", run.stdout, run.stdout[-1200:] + run.stderr[-400:])
        sent = {e: dict(pb_fields(b)) for e, _, b in bridge.relayed}
        self.assertEqual(sent[5414][2], 7)
        self.assertEqual(sent[5414][3], 150)
        self.assertEqual(sent[5414][4], struct.pack("<iii", 1, 2, 3))
        self.assertEqual(sent[5414][5], 1)
        self.assertEqual(sent[7901][2], 3)

@_both
class AppTicketCases(NetCase):
    def test_encrypted_app_ticket_comes_from_steam(self):
        def relay(emsg, job, body):
            if emsg == 5526:
                return 5527, 1, pb_varint(1, 2357570) + pb_varint(2, 1) + pb_len(3, b"TICKET")
            return None

        _, bridge = self.run_net("appticket", relay)
        sent = dict(pb_fields(bridge.relayed[0][2]))
        self.assertEqual((bridge.relayed[0][0], sent[1], sent[2]), (5526, 2357570, b"hello"))

@_both
class StoreUrlCases(NetCase):
    def test_store_auth_url_comes_from_the_econ_service(self):
        def relay(emsg, job, body):
            if job == "Econ.ClientGetItemShopOverlayAuthURL#1":
                return 147, 1, pb_len(1, b"https://store.steampowered.com/itemshop/setoverlay?cookie=abc&auth=def")
            return None

        _, bridge = self.run_net("storeurl", relay)
        self.assertEqual(dict(pb_fields(bridge.relayed[0][2]))[1], b"https://store.steampowered.com/")

def kv_blob(d):
    return b"\x00\x00" + b"".join(b"\x01" + k.encode() + b"\0" + v.encode() + b"\0" for k, v in d.items()) + b"\x08\x08"

def kv_parse(blob):
    parts = blob.split(b"\0")
    out, i = {}, 2
    while i + 1 < len(parts) and parts[i][:1] == b"\x01":
        out[parts[i][1:].decode()] = parts[i + 1].decode()
        i += 2
    return out

ME, OTHER, BETO = 76561198000000001, 76561198000000042, 76561198000000043
LOBBY, LISTED = 0x1860000000001234, 0x1860000000005678

@_both
class LobbyCases(NetCase):
    def test_lobby_lifecycle_pushes_search_and_join_use_the_relay(self):
        holder = {}
        state = {"data": {}, "member": {}}

        def lobby_data_push():
            members = pb_len(9, pb_fixed64(1, ME) + pb_len(2, b"Tester") + pb_len(3, kv_blob(state["member"])))
            return (pb_varint(1, 2357570) + pb_fixed64(2, LOBBY) + pb_varint(3, 1) + pb_varint(4, 4) + pb_varint(5, 2) + pb_varint(6, 0)
                    + pb_fixed64(7, ME) + pb_len(8, kv_blob(state["data"])) + members)

        def relay(emsg, job, body):
            f = pb_fields(body)
            fd = dict(f)
            if emsg == 6601:
                return 6602, 1, pb_varint(1, 2357570) + pb_fixed64(2, LOBBY) + pb_varint(3, 1)
            if emsg == 6609:
                if fd.get(3) == 0:
                    state["data"] = kv_parse(fd[7])
                else:
                    state["member"] = kv_parse(fd[7])
                holder["b"].pushes.append((6612, lobby_data_push()))
                return 6610, 1, b""
            if emsg == 6613:
                holder["b"].pushes.append((6614, pb_varint(1, 2357570) + pb_fixed64(2, fd[2]) + pb_fixed64(3, ME) + pb_len(4, fd[4])))
                holder["b"].pushes.append((6619, pb_varint(1, 2357570) + pb_fixed64(2, fd[2]) + pb_fixed64(3, OTHER) + pb_len(4, b"Ana")))
                return None
            if emsg == 6607:
                one = pb_fixed64(1, LISTED) + pb_varint(2, 4) + pb_varint(3, 2) + pb_varint(4, 0) + pb_len(5, kv_blob({"name": "Beto's Room", "map": "dust"})) + pb_varint(6, 2)
                full = pb_fixed64(1, LISTED + 1) + pb_varint(2, 4) + pb_varint(3, 2) + pb_varint(4, 0) + pb_len(5, kv_blob({"name": "Full"})) + pb_varint(6, 4)
                return 6608, 1, pb_varint(1, 2357570) + pb_varint(3, 1) + pb_len(4, one) + pb_len(4, full)
            if emsg == 6603:
                if fd[2] == LISTED:
                    holder["b"].pushes.append((800, pb_fixed64(1, ME) + pb_fixed64(2, 0x0186000000007777) + pb_fixed64(3, BETO) + pb_varint(4, 5) + pb_fixed64(7, 2357570)))
                if fd[2] == 0x1860000000009999:
                    holder["b"].pushes.append((6620, pb_varint(1, 2357570) + pb_fixed64(2, LISTED) + pb_fixed64(3, ME) + pb_len(4, b"Tester")))
                if fd[2] != LISTED:
                    return 6604, 1, pb_varint(1, 2357570) + pb_fixed64(2, fd[2]) + pb_varint(3, 2)
                members = pb_len(9, pb_fixed64(1, BETO) + pb_len(2, b"Beto")) + pb_len(9, pb_fixed64(1, ME) + pb_len(2, b"Tester"))
                return 6604, 1, (pb_varint(1, 2357570) + pb_fixed64(2, LISTED) + pb_varint(3, 1) + pb_varint(4, 4) + pb_varint(5, 2) + pb_varint(6, 0)
                                 + pb_fixed64(7, BETO) + pb_len(8, kv_blob({"name": "Beto's Room"})) + members)
            return None

        _, bridge = self.run_net("lobbies", relay, lambda b: holder.update(b=b))
        sent = {e: dict(pb_fields(b)) for e, _, b in bridge.relayed}
        self.assertEqual(sent[6601][2], 4)
        self.assertEqual(sent[6601][3], 2)
        flt = [dict(pb_fields(v)) for k, v in pb_fields(next(b for e, _, b in bridge.relayed if e == 6607)) if k == 6]
        self.assertEqual([(x[1], x[2], x[3], x[4]) for x in flt], [(b"map", b"dust", 0, 0), (b"lv", b"5", 2, 1), (b"", b"2", 0, 2), (b"skill", b"10", 0, 3), (b"", b"3", 0, 4)])
        self.assertIn(6605, [e for e, _, _ in bridge.relayed])
        self.assertEqual(sorted(bridge.subscribed), [800, 6612, 6614, 6618, 6619, 6620])

@_both
class InventoryCases(NetCase):
    def test_items_consume_promos_and_definitions_use_the_inventory_service(self):
        items = ('[{"accountid":"1","itemid":"111","quantity":"2","itemdefid":"100","state":""},'
                 '{"accountid":"1","itemid":"222","quantity":"1","itemdefid":"200","state":"NoTrade"}]')
        defs = '[{"itemdefid":"100","name":"Sword","type":"item","price":"1;USD999,BRL4600"},{"itemdefid":"200","name":"Shield"}]'

        def inv(items_json):
            return pb_len(1, b"etag") + pb_len(3, items_json.encode()) + pb_len(4, defs.encode())

        def relay(emsg, job, body):
            if job == "Inventory.GetInventory#1":
                return 147, 1, inv(items)
            if job == "Inventory.ConsumeItem#1":
                return 147, 1, inv(items.replace('"quantity":"2"', '"quantity":"1"'))
            if job == "Inventory.AddPromoItem#1":
                return 147, 1, inv('[{"itemid":"333","quantity":"1","itemdefid":"300"}]')
            if job in ("Inventory.SafeModifyItems#1", "Inventory.ConsumePlaytime#1", "Inventory.SplitItemStack#1", "Inventory.CombineItemStacks#1", "Inventory.InspectItem#1"):
                return 147, 1, inv(items)
            if job == "Inventory.GetUserPurchaseInfo#1":
                return 147, 1, pb_varint(1, 7)
            if job == "Inventory.PurchaseInit#1":
                return 147, 1, pb_varint(1, 555) + pb_varint(2, 777)
            if job == "Inventory.PurchaseFinalize#1":
                return 147, 1, inv('[{"itemid":"900","quantity":"2","itemdefid":"100"}]')
            if job == "Inventory.GetEligiblePromoItemDefIDs#1":
                return 147, 1, pb_len(1, bytes([7, 9]))
            return None

        _, bridge = self.run_net("inventory", relay, env={"AURELIA_SHIM_NO_BROWSER": "1", "AURELIA_SHIM_PURCHASE_POLL_MS": "200"})
        by_job = {j: dict(pb_fields(b)) for _, j, b in bridge.relayed if j}
        self.assertEqual((by_job["Inventory.ConsumeItem#1"][2], by_job["Inventory.ConsumeItem#1"][3]), (111, 1))
        self.assertEqual(by_job["Inventory.AddPromoItem#1"][2], 300)
        updates = [v for k, v in pb_fields(next(b for _, j, b in bridge.relayed if j == "Inventory.SafeModifyItems#1")) if k == 3]
        decoded = [dict(pb_fields(u)) for u in updates]
        self.assertEqual([(d[1], d.get(3)) for d in decoded], [(111, b"color"), (111, b"level"), (222, b"old")])
        self.assertEqual(decoded[0][6], b"blue")
        self.assertEqual(decoded[1][5], 7)
        self.assertEqual(decoded[2][2], 1)
        self.assertEqual(by_job["Inventory.ConsumePlaytime#1"][2], 55)
        self.assertEqual((by_job["Inventory.SplitItemStack#1"][2], by_job["Inventory.SplitItemStack#1"][3]), (111, 1))
        self.assertEqual((by_job["Inventory.CombineItemStacks#1"][2], by_job["Inventory.CombineItemStacks#1"][3], by_job["Inventory.CombineItemStacks#1"][4]), (111, 222, 1))
        self.assertEqual(by_job["Inventory.InspectItem#1"][3], b"token")
        init = pb_fields(next(b for _, j, b in bridge.relayed if j == "Inventory.PurchaseInit#1"))
        lines = [dict(pb_fields(v)) for k, v in init if k == 3]
        self.assertEqual([(l[1], l[2]) for l in lines], [(100, 2)])
        self.assertEqual(by_job["Inventory.PurchaseFinalize#1"][3], 555)

@_both
class UgcCases(NetCase):
    def test_subscriptions_state_download_and_subscribe_use_the_aurelia_slot(self):
        subscribed = [111, 222]

        def ugc(op, app, fid):
            if op == 1:
                return struct.pack("<I", len(subscribed)) + b"".join(struct.pack("<Q", i) for i in subscribed)
            if op == 2:
                if fid == 111:
                    path = b"/tmp/ws/111"
                    return b"\x01" + struct.pack("<QIH", 5000, 1700, len(path)) + path
                return b"\x00" + struct.pack("<QIH", 0, 0, 0)
            if op == 3:
                return struct.pack("<i", 1)
            if op == 4:
                subscribed.append(fid)
                return struct.pack("<i", 1)
            return struct.pack("<i", 1)

        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        app = {"appid": 480, "install_dir": "", "cloud_dir": "", "cloud_quota": 0, "build_id": 1, "beta_name": "public",
               "betas": [{"name": "public", "description": "", "build_id": 1, "time_updated": 0, "private": False},
                         {"name": "beta", "description": "x", "build_id": 2, "time_updated": 0, "private": False},
                         {"name": "internal", "description": "y", "build_id": 3, "time_updated": 0, "private": True}]}
        bridge = FakeBridge(ugc=ugc, snapshot_extra={"app": app})
        run = wine_args([exe], out, {"SELFTEST_NET": "ugc", "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn("PASS net_ugc", run.stdout, run.stdout[-1200:] + run.stderr[-400:])
        ops = [struct.unpack_from("<BIQ", p)[0] for i, sl, p in bridge.calls if (i, sl) == (0xFFFF, 10)]
        self.assertIn(3, ops)
        self.assertIn(4, ops)
        branch = [p[13:] for i, sl, p in bridge.calls if (i, sl) == (0xFFFF, 10) and p[0] == 11]
        self.assertEqual(branch, [b"beta"], "the private branch and the unknown one never reach Aurelia")

@_both
class UgcPublishCases(NetCase):
    def test_a_game_creates_updates_and_deletes_an_item_through_the_aurelia_slot(self):
        polls = []

        def ugc(op, app, fid):
            if op == 6:
                return struct.pack("<iQB", 1, 555, 0)
            if op == 7:
                return b""
            if op == 8:
                polls.append(1)
                if len(polls) < 2:
                    return struct.pack("<BQQiQB", 3, 50, 100, 0, 555, 0)
                return struct.pack("<BQQiQB", 5, 100, 100, 1, 555, 0)
            if op == 9:
                return struct.pack("<i", 1)
            return struct.pack("<i", 1)

        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge(ugc=ugc)
        run = wine_args([exe], out, {"SELFTEST_NET": "ugcpublish", "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn("PASS net_ugcpublish", run.stdout, run.stdout[-1200:] + run.stderr[-400:])
        sent = [(struct.unpack_from("<BIQ", p), p[13:]) for i, sl, p in bridge.calls if (i, sl) == (0xFFFF, 10) and p[0] in (6, 7, 9, 10)]
        self.assertEqual(sent[-1][1], b"/srv/workshop", "BInitWorkshopForGameServer sends the server folder as a Linux path")
        self.assertEqual(len([s for s in sent if s[0][0] == 10]), 1, "without a folder it does not call the bridge")
        sent = [s for s in sent if s[0][0] != 10]
        ops = [s[0][0] for s in sent]
        self.assertEqual(ops, [6, 7, 9])
        spec = json.loads(sent[1][1])
        self.assertEqual(spec["item"], 555)
        self.assertEqual((spec["title"], spec["description"], spec["visibility"]), ("My mod", 'Descri"ption', 2))
        self.assertEqual(spec["tags"], ["map", "coop"])
        self.assertEqual(spec["kv_tags"], [["mode", "cooperative"]])
        self.assertEqual(spec["content"], "/tmp/content", "the Wine path becomes the Linux one")
        self.assertEqual(spec["change_note"], "first version")
        self.assertEqual(spec["previews"], [{"file": "/tmp/a.png", "type": 0}, {"video": "dQw4w9WgXcQ", "type": 1}, {"file": "/tmp/b.png", "type": 0, "replaces": 2}])
        self.assertEqual((spec["previews_to_remove"], spec["descriptors_add"], spec["descriptors_remove"]), ([4], [5], [3]))
        self.assertEqual((spec["game_branch_min"], spec["game_branch_max"]), ("1.0", "2.0"))
        self.assertNotIn("metadata", spec, "what the game did not provide is not sent")
        self.assertGreaterEqual(len(polls), 2)

@_both
class LegacyWorkshopCases(NetCase):
    def test_a_game_publishes_updates_and_deletes_a_cloud_file_through_the_ucm_messages(self):
        def fixed64(n, v):
            return bytes([(n << 3) | 1]) + struct.pack("<Q", v)

        def relay(emsg, job, body):
            if emsg == 7311:
                return 7312, 1, pb_varint(1, 1) + fixed64(2, 777)
            if emsg in (7325, 7315):
                return emsg + 1, 1, pb_varint(1, 1)
            return None

        _, bridge = self.run_net("legacy_publish", relay)
        sent = {e: pb_fields(b) for e, j, b in bridge.relayed if e in (7311, 7315, 7325)}
        publish = sent[7311]
        self.assertEqual([v for k, v in publish if k == 2], [b"mod.zip"])
        self.assertEqual([v for k, v in publish if k == 3], [b"cover.png"])
        self.assertEqual([v for k, v in publish if k in (1, 4, 9, 10)], [2357570, 480, 1, 2], "game app, consumer app, Workshop file, private")
        self.assertEqual([v for k, v in publish if k == 8], [b"map", b"coop"])
        update = sent[7325]
        self.assertEqual([v for k, v in update if k == 2], [777])
        self.assertEqual([v for k, v in update if k == 3], [b"mod2.zip"])
        self.assertEqual([v for k, v in update if k == 5], [b"New title"])
        self.assertEqual([v for k, v in update if k == 15], [b"v2"])
        self.assertEqual(sorted(k for k, v in update if k in (9, 11, 13, 14)), [9, 11, 13, 14], "only what was provided is marked to change")
        self.assertEqual([v for k, v in sent[7315] if k == 1], [777])

@_both
class LegacyWorkshopQueryCases(NetCase):
    def test_a_game_lists_reads_votes_and_downloads_through_the_published_file_service(self):
        import http.server
        import threading

        class Files(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                body = b"mod-content"
                self.send_response(200 if self.path == "/castle.zip" else 404)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *args):
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Files)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.shutdown)
        url = f"http://127.0.0.1:{server.server_port}/castle.zip".encode()

        def relay(emsg, job, body):
            if job == "PublishedFile.GetDetails#1":
                return 147, 1, pb_len(1, published_file(777) + pb_len(10, url) + pb_varint(50, 3))
            if job == "PublishedFile.GetUserFiles#1":
                return 147, 1, pb_varint(1, 7) + pb_len(3, published_file(777)) + pb_len(3, published_file(778))
            if job == "PublishedFile.QueryFiles#1":
                return 147, 1, pb_varint(1, 99) + b"".join(pb_len(2, published_file(700 + i)) for i in range(11))
            if job == "PublishedFile.GetUserVoteSummary#1":
                return 147, 1, pb_len(1, pb_fixed64(1, 777) + pb_varint(2, 1))
            if job == "PublishedFile.Vote#1":
                return 147, 1, b""
            if job == "PublishedFile.Publish#1":
                return 147, 1, pb_varint(1, 888)
            return None

        _, bridge = self.run_net("legacy_workshop", relay)
        sent = {}
        for _, j, b in bridge.relayed:
            if j:
                sent.setdefault(j, []).append(pb_fields(b))
        user = dict(sent["PublishedFile.GetUserFiles#1"][0])
        self.assertEqual((user[6], user[2], user[25]), (b"myfiles", 2357570, 0))
        self.assertIn(19, user, "GetUserFiles asks for the votes through its own field (19), not the QueryFiles one")
        self.assertNotIn(17, user, "GetUserFiles field 17 is 'total only'")
        shared = dict(sent["PublishedFile.GetUserFiles#1"][1])
        self.assertEqual((shared[10], shared[1]), (b"map", 76561198000000042))
        query = dict(sent["PublishedFile.QueryFiles#1"][0])
        self.assertEqual((query[1], query[2], query[3], query[5], query[6], query[14]), (3, 11, 1, 2357570, b"map", 7), "trending over 7 days; start 10 with 1 per page is page 11")
        self.assertEqual(dict(sent["PublishedFile.Vote#1"][0]), {1: 777, 2: 0})
        video = dict(sent["PublishedFile.Publish#1"][0])
        self.assertEqual((video[7], video[9], video[10], video[4], video[13]), (4, b"channel", b"dQw4w9WgXcQ", b"cover.png", b"map"))

@_both
class UgcWriteCases(NetCase):
    def test_playtime_tracking_goes_to_the_published_file_service(self):
        def relay(emsg, job, body):
            if job and job.startswith("PublishedFile."):
                return 147, 1, b""
            return None

        _, bridge = self.run_net("ugcwrite", relay)
        sent = {j: pb_fields(b) for _, j, b in bridge.relayed if j and j.startswith("PublishedFile.")}
        start = sent["PublishedFile.StartPlaytimeTracking#1"]
        self.assertEqual([v for k, v in start if k == 2], [111, 222])
        self.assertEqual([v for k, v in sent["PublishedFile.StopPlaytimeTracking#1"] if k == 2], [111])
        self.assertIn("PublishedFile.StopPlaytimeTrackingForAllAppItems#1", sent)
        self.assertEqual(len([1 for _, j, _ in bridge.relayed if j == "PublishedFile.StopPlaytimeTrackingForAllAppItems#1"]), 1, "with nothing tracked it does not call Steam")
        self.assertEqual(len([1 for _, j, _ in bridge.relayed if j == "PublishedFile.StartPlaytimeTracking#1"]), 1, "the empty list does not call Steam")

def _overlay_requests(bridge):
    out = []
    for i, sl, p in bridge.calls:
        if (i, sl) == (0xFFFF, 13) and p[0] == 1:
            kl = struct.unpack_from("<H", p, 5)[0]
            kind = p[7:7 + kl].decode()
            jl = struct.unpack_from("<H", p, 7 + kl)[0]
            out.append((kind, json.loads(p[9 + kl:9 + kl + jl] or b"{}")))
    return out

@_both
class OverlayCases(NetCase):
    @staticmethod
    def _handler(capable=False, updates=None):
        queue = list(updates or [])

        def handler(op, body):
            if op == 1:
                return struct.pack("<H", 1) + b"1"
            if op == 2:
                batch, queue[:] = list(queue), []
                return struct.pack("<I", len(batch)) + b"".join(struct.pack("<BH", t, len(i)) + i for t, i in batch)
            if op == 3:
                return bytes([1 if capable else 0])
            return b""
        return handler

    def _run(self, name, handler):
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge(overlay=handler)
        run = wine_args([exe], out, {"SELFTEST_NET": name, "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn(f"PASS net_{name}", run.stdout, run.stdout[-1200:] + run.stderr[-400:])
        return bridge

    def test_overlay_requests_reach_the_bridge_and_overlay_stays_disabled_until_capable(self):
        bridge = self._run("overlay_requests", self._handler(capable=False))
        reqs = _overlay_requests(bridge)
        kinds = [k for k, _ in reqs]
        self.assertEqual(kinds, ["store", "web_page", "achievements", "user", "invite", "invite", "remote_play_invite", "played_with", "web_page", "web_page"])
        self.assertEqual(reqs[0][1], {"target": {"app_id": 1234, "flag": 0}})
        self.assertEqual(len(reqs[1][1]["url"]), 2048)
        self.assertEqual(reqs[3][1], {"dialog": "chat", "steam_id": 76561198000000042})
        self.assertEqual(reqs[4][1], {"lobby": 109775240000000001})
        self.assertEqual(reqs[5][1], {"connect": "+connect 1.2.3.4"})
        self.assertEqual(reqs[6][1], {"lobby": 109775240000000002})
        self.assertEqual(reqs[7][1], {"steam_id": 76561198000000077})
        self.assertEqual(reqs[1][1]["mode"], "default")
        self.assertEqual(reqs[8][1], {"url": "https://example.test/modal", "mode": "modal"})
        self.assertEqual(reqs[9][1], {"url": "https://example.test/old", "mode": "default"})

    def test_is_overlay_enabled_follows_the_launcher_declaration(self):
        self._run("overlay_capable", self._handler(capable=True))

    def test_accepted_lobby_invite_posts_game_lobby_join_requested(self):
        sent = []

        def handler(op, body):
            if op == 1:
                return struct.pack("<H", 1) + b"1"
            if op == 2 and not sent:
                sent.append(1)
                return struct.pack("<I", 1) + struct.pack("<BH", 6, 1) + b"9" + struct.pack("<QQ", 109775240000000001, 76561198000000002)
            if op == 2:
                return struct.pack("<I", 0)
            return bytes([0]) if op == 3 else b""
        self._run("overlay_join", handler)

    def test_the_on_screen_keyboard_round_trips_through_the_launcher(self):
        answered = []

        def handler(op, body):
            if op == 1:
                return struct.pack("<H", 1) + b"1"
            if op == 2:
                if answered:
                    return struct.pack("<I", 0)
                answered.append(1)
                return struct.pack("<I", 1) + struct.pack("<BH", 7, 1) + b"1" + struct.pack("<BH", 1, 3) + b"Ana"
            if op == 3:
                return bytes([1])
            return b""
        bridge = self._run("gamepad_text", handler)
        reqs = _overlay_requests(bridge)
        self.assertEqual([k for k, _ in reqs], ["gamepad_text"])
        self.assertEqual(reqs[0][1], {"mode": 0, "line_mode": 1, "max_chars": 16, "description": "Your name", "existing": "John"})

    def test_coplay_friend_lobbies_and_cloud_changes_come_from_the_aurelia(self):
        def handler(op, body):
            if op == 6:
                return struct.pack("<I", 2) + struct.pack("<QII", 76561198000000011, 480, 1700000500) + struct.pack("<QII", 76561198000000022, 440, 1700000000)
            if op == 7:
                return struct.pack("<I", 1) + struct.pack("<QQ", 76561198000000033, 109775240000000001)
            if op == 8:
                return struct.pack("<I", 2) + struct.pack("<H", 9) + b"save1.dat" + struct.pack("<H", 15) + b"profile/ana.cfg"
            return b""
        asked = []

        def relay(emsg, job, body):
            asked.append(emsg)
            return None
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge(overlay=handler, relay=relay)
        run = wine_args([exe], out, {"SELFTEST_NET": "overlay_data", "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn("PASS net_overlay_data", run.stdout, run.stdout[-1200:] + run.stderr[-400:])
        lobby_asks = [b for e, j, b in bridge.relayed if 109775240000000001 in [v for k, v in pb_fields(b) if k == 2]]
        self.assertEqual(len(lobby_asks), 1, "the friend's lobby was requested from Steam once")

    def test_the_floating_keyboard_types_into_the_game_window(self):
        sent = []

        def handler(op, body):
            if op == 1:
                return struct.pack("<H", 1) + b"1"
            if op == 2:
                if sent:
                    return struct.pack("<I", 0)
                sent.append(1)
                return (struct.pack("<I", 2) + struct.pack("<BH", 8, 1) + b"1" + struct.pack("<BH", 0, 2) + b"Ab"
                        + struct.pack("<BH", 8, 1) + b"1" + struct.pack("<BH", 1, 0))
            if op == 3:
                return bytes([1])
            return b""
        bridge = self._run("gamepad_floating", handler)
        reqs = _overlay_requests(bridge)
        self.assertEqual([k for k, _ in reqs], ["gamepad_text"])
        self.assertEqual(reqs[0][1], {"floating": True, "mode": 0, "left": 10, "top": 20, "width": 300, "height": 100})

    def test_shown_and_hidden_updates_post_game_overlay_activated(self):
        self._run("overlay_shown", self._handler(updates=[(4, b"1"), (5, b"1")]))

@_both
class DlcUpdateCases(NetCase):
    def test_dlc_ownership_change_posts_dlc_installed_and_updates_the_answers(self):
        polls = []

        def handler(op, body):
            if op == 5:
                polls.append(1)
                owned = 1 if len(polls) > 1 else 0
                return struct.pack("<I", 1) + struct.pack("<IBB", 777, owned, owned)
            return struct.pack("<I", 0) if op == 2 else b"\x00"
        app = {"appid": 2357570, "install_dir": "", "cloud_dir": "", "cloud_quota": 0, "build_id": 1, "beta_name": "public",
               "dlc": [{"appid": 777, "owned": False, "installed": False, "name": "Extra Pack"}]}
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge(overlay=handler, snapshot_extra={"app": app})
        run = wine_args([exe], out, {"SELFTEST_NET": "dlc_updates", "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn("PASS net_dlc_updates", run.stdout, run.stdout[-1200:] + run.stderr[-400:])
        self.assertGreaterEqual(len(polls), 2)

@_both
class MicroTxnCaptureCases(NetCase):

    def test_auth_request_becomes_a_microtxn_request_and_the_denial_goes_back_to_steam(self):
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        push = (ROOT / "tests/fixtures/microtxn_5504_overwatch.bin").read_bytes()
        answered = []

        def overlay(op, body):
            if op == 1:
                answered.append("request")
                return struct.pack("<H", 1) + b"7"
            if op == 2 and answered and "deny" not in answered:
                answered.append("deny")
                return struct.pack("<I", 1) + struct.pack("<BH", 2, 1) + b"7"
            return struct.pack("<I", 0) if op == 2 else b"\x00"

        bridge = FakeBridge(overlay=overlay, relay=lambda emsg, job, body: (5506, 1, struct.pack("<I", 2) + b"\x00"))

        def push_after_subscription():
            for _ in range(200):
                if 5504 in bridge.subscribed:
                    bridge.pushes.append((5504, push))
                    return
                time.sleep(0.05)
        threading.Thread(target=push_after_subscription, daemon=True).start()
        log = out / "shim.log"
        run = wine_args([exe], out, {"SELFTEST_NET": "microtxn_capture", "AURELIA_BRIDGE_PORT": str(bridge.port),
                                     "AURELIA_BRIDGE_TOKEN": bridge.token.hex(), "AURELIA_SHIM_LOG": "Z:" + str(log).replace("/", "\\")})
        self.assertIn("PASS net_microtxn_capture", run.stdout, run.stdout[-1200:] + run.stderr[-400:] + (log.read_text() if log.exists() else ""))
        self.assertIn(5504, bridge.subscribed)
        self.assertIn("microtxn: push 5504 (366 bytes): 01004d657373616765", log.read_text())
        reqs = _overlay_requests(bridge)
        self.assertEqual([k for k, _ in reqs], ["microtxn"])
        info = reqs[0][1]
        self.assertEqual((info["order_id"], info["trans_id"]), ("506252350422904163", "410551019262160112"))
        self.assertEqual((info["currency"], info["total"], info["total_minor"], info["sandbox"], info["refundable"]), ("BRL", "23.95", 2395, False, False))
        self.assertEqual(info["url"], "https://checkout.steampowered.com/checkout/approvetxn/410551019262160112/?returnurl=steam")
        self.assertEqual(info["window"], {"width": 1020, "height": 440})
        self.assertEqual(info["items"], [{"name": "500 Coins", "qty": 1, "price": "23.95", "amount_minor": 2395, "item_id": "847133"}])
        authorize = [(e, b) for e, _, b in bridge.relayed if e == 5505]
        self.assertEqual(len(authorize), 1, "the launcher's response becomes a 5505 for Steam")
        self.assertEqual(authorize[0][1], struct.pack("<QI", 410551019262160112, 2))

class FakeSourceServer:

    CHALLENGE = b"\x11\x22\x33\x44"

    def __init__(self):
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.bind(("127.0.0.1", 0))
        self.port = self.sock.getsockname()[1]
        threading.Thread(target=self._serve, daemon=True).start()

    @staticmethod
    def _cstr(s):
        return s.encode() + b"\0"

    def _info(self):
        body = b"\xff\xff\xff\xffI" + bytes([17]) + self._cstr("Test Server") + self._cstr("de_test") + self._cstr("tf") + self._cstr("Team Fortress")
        body += struct.pack("<H", 440) + bytes([5, 24, 1]) + b"dl" + bytes([0, 1]) + self._cstr("1.0.2")
        body += bytes([0x80 | 0x10 | 0x20]) + struct.pack("<H", 27030) + struct.pack("<Q", 85568392926520363) + self._cstr("alltalk,test")
        return body

    def _serve(self):
        while True:
            data, addr = self.sock.recvfrom(2048)
            if not data.startswith(b"\xff\xff\xff\xff"):
                continue
            kind, rest = data[4:5], data[5:]
            if kind == b"T":
                if rest.endswith(self.CHALLENGE):
                    self.sock.sendto(self._info(), addr)
                else:
                    self.sock.sendto(b"\xff\xff\xff\xffA" + self.CHALLENGE, addr)
            elif kind in (b"U", b"V"):
                if rest != self.CHALLENGE:
                    self.sock.sendto(b"\xff\xff\xff\xffA" + self.CHALLENGE, addr)
                elif kind == b"U":
                    players = b"".join(bytes([i]) + self._cstr(n) + struct.pack("<if", sc, 12.5) for i, (n, sc) in enumerate([("Ana", 7), ("Beto", 3)]))
                    self.sock.sendto(b"\xff\xff\xff\xffD" + bytes([2]) + players, addr)
                else:
                    rules = self._cstr("mp_friendlyfire") + self._cstr("0") + self._cstr("sv_cheats") + self._cstr("0")
                    self.sock.sendto(b"\xff\xff\xff\xffE" + struct.pack("<H", 2) + rules, addr)

@_both
class ServerListCases(NetCase):
    def test_master_list_a2s_ping_players_and_rules(self):
        server = FakeSourceServer()
        live = (pb_varint(1, 0x7F000001) + pb_varint(2, server.port) + pb_len(4, pb_raw(1 << 3 | 5) + struct.pack("<I", 0x7F000001))
                + pb_fixed64(6, 1) + pb_varint(7, 5) + pb_varint(8, 3) + pb_len(18, b"de_test") + pb_len(20, b"master name"))
        dead = pb_varint(1, 0x7F000001) + pb_varint(2, 1) + pb_len(18, b"de_test")
        default = pb_varint(12, 440) + pb_varint(13, 24) + pb_len(16, b"tf")

        holder = {}
        writes = []
        vdf = ('"Filters"\n{\n\t"favorites"\n\t{\n\t\t"1"\n\t\t{\n\t\t\t"name"\t\t"x"\n\t\t\t"address"\t\t"127.0.0.1:%d"\n\t\t\t"LastPlayed"\t\t"5"\n\t\t\t"appid"\t\t"440"\n\t\t}\n\t}\n'
               '\t"history"\n\t{\n\t\t"1"\n\t\t{\n\t\t\t"address"\t\t"127.0.0.1:1"\n\t\t\t"appid"\t\t"440"\n\t\t}\n\t}\n}\n') % server.port

        def cloudfile(op, app, name, data):
            if op == 1:
                return vdf.encode() if (app, name) == (7, "serverbrowser_hist.vdf") else b""
            writes.append((app, name, data.decode()))
            return b""

        def relay(emsg, job, body):
            if emsg == 6403:
                return 6404, 1, pb_len(1, live) + pb_len(1, dead) + pb_len(3, default)
            if emsg == 815:
                friend = pb_fixed64(1, 76561198000000042) + pb_varint(3, 440) + pb_varint(4, 0x7F000001) + pb_varint(5, server.port) + pb_varint(20, server.port)
                holder["b"].pushes.append((766, pb_len(2, friend)))
            return None

        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge(relay=relay, cloudfile=cloudfile)
        holder["b"] = bridge
        run = wine_args([exe], out, {"SELFTEST_NET": "servers", "SELFTEST_SERVER_PORT": str(server.port),
                                      "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn("PASS net_servers", run.stdout, run.stdout[-1200:] + run.stderr[-400:])
        self.assertTrue(any(w[1] == "serverbrowser_hist.vdf" and "127.0.0.1:27031" in w[2] and '"favorites"' in w[2] for w in writes), writes)
        self.assertIn(815, [e for e, _, _ in bridge.relayed])
        query = dict(pb_fields(bridge.relayed[0][2]))
        self.assertEqual((bridge.relayed[0][0], query[1], query[4]), (6403, 440, b"\\appid\\440\\map\\de_test"))

def published_file(fid, title="Magic Castle"):
    votes = pb_raw(1 << 3 | 5) + struct.pack("<f", 0.9) + pb_varint(2, 30) + pb_varint(3, 2)
    return (pb_varint(1, 1) + pb_varint(2, fid) + pb_fixed64(3, 76561198000000042) + pb_varint(4, 480) + pb_varint(5, 480) + pb_len(7, b"castle.zip")
            + pb_varint(8, 4096) + pb_varint(9, 100) + pb_len(11, b"http://p/1.png") + pb_fixed64(14, 555) + pb_len(16, title.encode())
            + pb_len(17, "A castle mod".encode()) + pb_varint(19, 1700000000) + pb_varint(20, 1700000100) + pb_varint(21, 0) + pb_varint(34, 0)
            + pb_varint(36, 304) + pb_varint(49, 2) + pb_len(52, pb_len(1, b"mod") + pb_len(3, b"Mod")) + pb_len(52, pb_len(1, b"fun"))
            + pb_len(53, pb_varint(1, 11)) + pb_len(53, pb_varint(1, 12)) + pb_len(54, pb_len(1, b"version") + pb_len(2, b"1.2"))
            + pb_len(55, votes) + pb_len(58, b"meta!"))

@_both
class UgcQueryCases(NetCase):
    def test_workshop_queries_details_and_result_getters(self):
        def relay(emsg, job, body):
            f = pb_fields(body)
            if job == "PublishedFile.QueryFiles#1":
                return 147, 1, pb_varint(1, 99) + pb_len(2, published_file(777)) + pb_len(2, published_file(778, "Other")) + pb_len(3, b"CURSOR")
            if job == "PublishedFile.GetDetails#1":
                return 147, 1, pb_len(1, published_file(777)) + pb_len(1, published_file(778, "Other"))
            if job == "PublishedFile.GetUserFiles#1":
                return 147, 1, pb_varint(1, 1) + pb_len(3, published_file(779))
            return None

        _, bridge = self.run_net("ugcquery", relay)
        sent = {j: pb_fields(b) for _, j, b in bridge.relayed if j}
        q = dict(sent["PublishedFile.QueryFiles#1"])
        self.assertEqual((q[1], q[5], q[6], q[11]), (12, 480, b"mod", b"castle"))
        user = dict(sent["PublishedFile.GetUserFiles#1"])
        self.assertEqual((user[6], user[2]), (b"mysubscriptions", 480))
        self.assertEqual(len([1 for _, j, _b in bridge.relayed if j == "PublishedFile.GetUserFiles#1"]), 1)

@_both
class GameServerCases(NetCase):
    def test_gameserver_logon_a2s_answers_and_license_state(self):
        ops = []

        def gameserver(op):
            ops.append(op)
            return struct.pack("<QI", 90297557588031489, 0x01020304) if op == 1 else b""

        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge(gameserver=gameserver, relay=lambda e, j, b: None)
        run = wine_args([exe], out, {"SELFTEST_NET": "gameserver", "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn("PASS net_gameserver", run.stdout, run.stdout[-1200:] + run.stderr[-400:])
        self.assertEqual(ops, [1, 2])
        self.assertEqual(sorted(set(bridge.relayed_gs)), [908, 4331])
        stype = dict(pb_fields(next(b for e, _, b in bridge.relayed if e == 908)))
        self.assertEqual((stype[1], stype[4], stype[5], stype[7]), (480, 27015, b"spacewar", 27016))

@_both
class WebTicketCases(NetCase):
    def test_web_api_ticket_is_created_with_type_5_and_the_identity(self):
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge()
        run = wine_args([exe], out, {"SELFTEST_NET": "webticket", "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn("PASS net_webticket", run.stdout, run.stdout[-1200:] + run.stderr[-400:])
        creates = [p for i, sl, p in bridge.calls if (i, sl) == (0xFFFF, 3)]
        self.assertEqual(len(creates), 1)
        appid, handle, ttype, n = struct.unpack_from("<IIIH", creates[0])
        self.assertEqual((appid, ttype, creates[0][14:14 + n]), (2357570, 5, b"str:my-service"))

@_both
class OfflineCases(NetCase):
    def test_a_game_opened_offline_sees_no_session_until_the_bridge_reports_the_connection(self):
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge(snapshot_extra={"online": False}, connected=False, flip_after=1)
        run = wine_args([exe], out, {"SELFTEST_NET": "offline", "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn("PASS net_offline", run.stdout, run.stdout[-1200:] + run.stderr[-400:])

    def test_a_snapshot_without_the_field_means_online(self):
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge()
        run = wine_args([exe], out, {"SELFTEST_BRIDGE": "1", "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn("PASS ticket_is_created_by_the_bridge", run.stdout, run.stdout[-800:])

@_both
class AccountQueryCases(NetCase):
    def test_purchase_time_and_own_license_answers_come_from_the_account(self):
        asked = []

        def appsq(op, app):
            asked.append((op, app))
            return struct.pack("<I", {550: 1_300_000_000, 440: 1_400_000_000}.get(app, 0)) if op == 1 else None

        def ugc(op, app, fid):
            return struct.pack("<I", 1 if (op == 12 and fid == 777) else 0)
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge(appsq=appsq, ugc=ugc)
        run = wine_args([exe], out, {"SELFTEST_NET": "account_queries", "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn("PASS net_account_queries", run.stdout, run.stdout[-1200:] + run.stderr[-400:])
        self.assertIn((1, 550), asked)
        self.assertIn((1, 999), asked)

@_both
class LegacySignatureCases(NetCase):
    def test_the_old_forms_of_publish_update_video_and_beta_info_work(self):
        def fixed64(n, v):
            return bytes([(n << 3) | 1]) + struct.pack("<Q", v)

        def relay(emsg, job, body):
            if emsg == 7311:
                return 7312, 1, pb_varint(1, 1) + fixed64(2, 777)
            if emsg == 7325:
                return 7326, 1, pb_varint(1, 1)
            if job == "PublishedFile.Publish#1":
                return 147, 1, pb_varint(1, 888)
            return None
        app = {"appid": 2357570, "install_dir": "", "cloud_dir": "", "cloud_quota": 0, "build_id": 1, "beta_name": "public", "dlc": [],
               "betas": [{"name": "public", "description": "", "build_id": 1, "time_updated": 1, "private": False},
                         {"name": "beta", "description": "next", "build_id": 99, "time_updated": 1700000000, "private": False}]}
        _, bridge = self.run_net("legacy_signatures", relay, setup=lambda b: b.snapshot_extra.update({"app": app}))
        sent = [(e, j, pb_fields(b)) for e, j, b in bridge.relayed]
        publishes = [f for e, j, f in sent if e == 7311]
        self.assertEqual(len(publishes), 2)
        first = publishes[0]
        self.assertEqual([v for k, v in first if k == 2], [b"save.dat"])
        self.assertEqual([v for k, v in first if k in (1, 4, 9, 10)], [2357570, 480, 0, 2], "PublishFile: outside the Workshop, private")
        self.assertEqual([v for k, v in first if k == 8], [b"map", b"coop"])
        second = publishes[1]
        self.assertEqual([v for k, v in second if k in (9, 10)], [1, 0], "legacy PublishWorkshopFile: Workshop, public")
        update = [f for e, j, f in sent if e == 7325][0]
        self.assertEqual([v for k, v in update if k == 2], [777])
        self.assertEqual([v for k, v in update if k == 3], [b"mod2.zip"])
        self.assertEqual([v for k, v in update if k == 5], [b"New"])
        self.assertEqual(sorted(k for k, v in update if k in (9, 10, 11, 12, 13, 14)), [9, 11], "only file and title marked to change")
        video = [dict(f) for e, j, f in sent if j == "PublishedFile.Publish#1"][0]
        self.assertEqual((video[7], video[9], video[10], video[4]), (4, b"channel", b"dQw4w9WgXcQ", b"cover.png"))

def overlay_request_body(body):
    n = struct.unpack_from("<H", body, 5)[0]
    kind = body[7:7 + n].decode()
    m = struct.unpack_from("<H", body, 7 + n)[0]
    return kind, json.loads(body[9 + n:9 + n + m] or b"{}")

@_both
class CommunityCases(NetCase):
    def test_followers_file_share_officers_group_chat_parties_favorites_market_timeline_and_browser(self):
        def f64(n, v):
            return bytes([(n << 3) | 1]) + struct.pack("<Q", v)

        def relay(emsg, job, body):
            if emsg == 7515:
                return 7516, 1, pb_varint(1, 1) + pb_varint(2, 42)
            if emsg == 7517:
                return 7518, 1, pb_varint(1, 1) + pb_varint(2, 1)
            if emsg == 7519:
                return 7520, 1, pb_varint(1, 1) + pb_varint(2, 5) + f64(3, 76561198000000001) + f64(3, 76561198000000002)
            if emsg == 5232:
                return 5233, 1, pb_varint(1, 1) + f64(2, 9999)
            if emsg == 5531:
                return 5532, 1, pb_varint(1, 1) + f64(2, 103582791429521408) + pb_varint(3, 3)
            if job == "Parties.CreateBeacon#1":
                return 147, 1, pb_varint(1, 555)
            if job == "Parties.ChangeNumOpenSlots#1":
                return 147, 1, pb_varint(1, 555)
            if job == "Parties.JoinParty#1":
                return 147, 1, pb_varint(1, 777) + pb_len(2, b"+connect 5.6.7.8")
            return None

        chat = {"polls": 0, "sent": []}

        def clan(body):
            op = body[0]
            if op == 1:
                return struct.pack("<iIQ", 1, 3, 76561198000000100)
            if op == 3:
                chat["sent"].append(body[9:])
                return struct.pack("<i", 1)
            if op == 4:
                return struct.pack("<I", 2) + struct.pack("<QQ", 76561198000000001, 76561198000000002)
            if op == 5:
                chat["polls"] += 1
                text = b"welcome"
                if chat["polls"] == 1:
                    return struct.pack("<I", 1) + struct.pack("<QQIH", 103582791429521408, 76561198000000002, 1795000000, len(text)) + text
                return struct.pack("<I", 0)
            return struct.pack("<i", 1)

        web = []

        def web_slot(body):
            web.append(body[0])
            if body[0] == 3:
                return bytes([0, 0, 0, 0]) + struct.pack("<IIii", 128, 1795600000, 15, 0)
            return struct.pack("<i", 1)

        overlay = []

        def overlay_handler(op, body):
            if op == 1:
                overlay.append(overlay_request_body(body))
                return b"\x00\x00"
            return struct.pack("<I", 0) if op == 2 else b"\x00"

        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge(relay=relay, slots={17: web_slot, 18: clan}, overlay=overlay_handler)
        run = wine_args([exe], out, {"SELFTEST_NET": "community", "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn("PASS net_community", run.stdout, run.stdout[-1200:] + run.stderr[-400:])
        self.assertEqual(chat["sent"], [b"hello group"])
        self.assertIn(1, web)
        self.assertIn(3, web)
        kinds = [k for k, _ in overlay]
        self.assertIn("timeline", kinds)
        timeline = dict(overlay)["timeline"]
        self.assertEqual((timeline["kind"], timeline["title"], timeline["priority"], timeline["offset"]), ("instant", "Ace", 3, -2.5))
        self.assertEqual(dict(overlay)["web_page"]["url"], "https://store.steampowered.com/app/480")

@_both
class OtherStatsCases(NetCase):
    def test_another_players_stats_come_from_the_steam_and_the_server_can_change_and_store_them(self):
        def name(n):
            raw = n.encode()
            return struct.pack("<H", len(raw)) + raw

        def reply(kills):
            out = struct.pack("<II", 0xBEEF, 1) + struct.pack("<IIII", 1, 1, kills, 0) + name("kills")
            out += struct.pack("<I", 2)
            out += struct.pack("<III", 3, 0, 1700000000) + name("ACH_ONE")
            out += struct.pack("<III", 3, 1, 0) + name("ACH_TWO")
            return out
        seen = []

        def other(body):
            op, gs = body[0], body[1]
            player = struct.unpack_from("<Q", body, 2)[0]
            seen.append((op, gs, player, body[10:]))
            if op == 1:
                return reply(42 if not gs else 7)
            return struct.pack("<i", 1)
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge(slots={19: other})
        run = wine_args([exe], out, {"SELFTEST_NET": "other_stats", "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn("PASS net_other_stats", run.stdout, run.stdout[-1200:] + run.stderr[-400:])
        gets = [s for s in seen if s[0] == 1]
        self.assertEqual([(g[1], g[2]) for g in gets], [(0, 76561198000000042), (1, 76561198000000077)])
        store = [s for s in seen if s[0] == 2][0]
        self.assertEqual((store[1], store[2]), (1, 76561198000000077))
        crc, n = struct.unpack_from("<II", store[3])
        changes = sorted(struct.unpack_from("<II", store[3], 8 + 8 * i) for i in range(n))
        self.assertEqual((crc, changes), (0xBEEF, [(1, 50), (3, 0b11)]), "new stat and the achievements field with both bits")

@_both
class AppDependencyCases(NetCase):
    def test_app_dependencies_use_relationship_one_and_ignore_the_other_relationships(self):
        sent = []

        def relay(emsg, job, body):
            if job in ("PublishedFile.AddAppRelationship#1", "PublishedFile.RemoveAppRelationship#1"):
                sent.append((job, dict(pb_fields(body))))
                return 147, 1, b""
            if job == "PublishedFile.GetAppRelationships#1":
                return 147, 1, pb_len(3, pb_varint(1, 550) + pb_varint(2, 1)) + pb_len(3, pb_varint(1, 730) + pb_varint(2, 3))
            return None
        _, bridge = self.run_net("app_dependencies", relay)
        self.assertEqual([(j, f[1], f[2], f[3]) for j, f in sent], [("PublishedFile.AddAppRelationship#1", 777, 550, 1), ("PublishedFile.RemoveAppRelationship#1", 777, 550, 1)])

@_both
class P2PCases(NetCase):
    def _run_pair(self, role_a, role_b, extra_env=None):
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge(steamids=[76561198000000222, 76561198000000111])
        env = {"PATH": "/usr/bin:/bin", "WINEPREFIX": str(PREFIX), "WINEDEBUG": "-all", "HOME": str(Path.home()),
               "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex(), "AURELIA_SHIM_P2P_LOCAL": "1", "AURELIA_P2P_DEBUG": "1", **(extra_env or {})}
        def start(name):
            return subprocess.Popen([str(WINE), str(exe)], env={**env, "SELFTEST_NET": name}, cwd=out, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        b = start(role_b)
        for _ in range(300):
            if len(bridge.steamids) < 2:
                break
            time.sleep(0.2)
        time.sleep(4)
        a = start(role_a)
        try:
            out_a, err_a = a.communicate(timeout=120)
            out_b, err_b = b.communicate(timeout=120)
        except subprocess.TimeoutExpired:
            for proc in (a, b):
                proc.kill()
            out_a, err_a = a.communicate()
            out_b, err_b = b.communicate()
            self.fail(f"timed out\nA:{out_a[-600:]}\n{err_a[-1500:]}\nB:{out_b[-600:]}\n{err_b[-1500:]}")
        self.p2p_stderr = (err_a, err_b)
        return out_a, out_b, bridge

    def test_two_players_exchange_reliable_and_unreliable_packets(self):
        out_a, out_b, bridge = self._run_pair("p2p_a", "p2p_b")
        logs = "\nA:" + self.p2p_stderr[0][-3000:] + "\nB:" + self.p2p_stderr[1][-3000:]
        self.assertIn("PASS net_p2p_a", out_a, out_a[-800:] + logs)
        self.assertIn("PASS net_p2p_b", out_b, out_b[-800:] + logs)
        kinds = {e for e, _, _ in bridge.relayed}
        self.assertEqual(kinds, {5434})

    def test_sockets_connect_accept_fragment_and_close(self):
        out_a, out_b, bridge = self._run_pair("sockets_a", "sockets_b")
        self.assertIn("PASS net_sockets_a", out_a, out_a[-800:] + "\nA:" + self.p2p_stderr[0][-2500:] + "\nB:" + self.p2p_stderr[1][-2500:])
        self.assertIn("PASS net_sockets_b", out_b, out_b[-800:] + "\nA:" + self.p2p_stderr[0][-2500:] + "\nB:" + self.p2p_stderr[1][-2500:])

    def test_messages_session_request_accept_and_channels(self):
        out_a, out_b, bridge = self._run_pair("messages_a", "messages_b")
        self.assertIn("PASS net_messages_a", out_a, out_a[-800:])
        self.assertIn("PASS net_messages_b", out_b, out_b[-800:])

    def test_ip_sockets_connect_and_exchange_inside_one_process(self):
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        run = wine_args([exe], out, {"SELFTEST_NET": "ipsockets", "AURELIA_SHIM_P2P_LOCAL": "1"})
        self.assertIn("PASS net_ipsockets", run.stdout, run.stdout[-800:] + run.stderr[-300:])

    def test_socket_pair_in_memory_and_message_allocation(self):
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        run = wine_args([exe], out, {"SELFTEST_NET": "socketpair"})
        self.assertIn("PASS net_socketpair", run.stdout, run.stdout[-800:] + run.stderr[-300:])

@_both
class RichPresenceCases:
    def test_presence_pairs_reach_the_bridge_and_read_back(self):
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        bridge = FakeBridge()
        run = wine_args([exe], out, {"SELFTEST_PRESENCE": "1", "AURELIA_BRIDGE_PORT": str(bridge.port), "AURELIA_BRIDGE_TOKEN": bridge.token.hex()})
        self.assertIn("PASS rich_presence_goes_to_the_bridge", run.stdout, run.stdout[-800:] + run.stderr[-300:])
        sets = [p for i, sl, p in bridge.calls if (i, sl) == (0xFFFF, 6)]
        self.assertGreaterEqual(len(sets), 3)
        last = sets[2]
        n = struct.unpack_from("<H", last)[0]
        o, pairs = 2, {}
        for _ in range(n):
            kl = struct.unpack_from("<H", last, o)[0]; o += 2; k = last[o:o + kl].decode(); o += kl
            vl = struct.unpack_from("<H", last, o)[0]; o += 2; v = last[o:o + vl].decode(); o += vl
            pairs[k] = v
        self.assertEqual(pairs, {"status": "In Game", "steam_display": "#Menu"})

@_both
class CrashLogCases:
    def test_access_violation_is_logged_with_module_offset_and_stack(self):
        out = Path(tempfile.mkdtemp())
        exe = self.build_selftest(out)
        log = out / "crash.log"
        wine_args([exe], out, {"SELFTEST_CRASH": "1", "AURELIA_SHIM_LOG": str(log)})
        text = log.read_text() if log.exists() else ""
        self.assertRegex(text, r"EXCEPTION c0000005 at selftest\.exe\+0x[0-9a-f]+ .* write", text)
        self.assertIn("accessed address 0000000000000000" if self.ARCH == "win64" else "accessed address 00000000", text)
        self.assertRegex(text, r"stack\+\d+: selftest\.exe\+0x", text)

@_both
class HttpCases:
    def test_http_requests_run_through_winhttp_against_a_local_server(self):
        import http.server
        seen = {}

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_GET(self):
                seen["get"] = (self.path, self.headers.get("X-Test"))
                self.send_response(200)
                self.send_header("X-Reply", "yes")
                self.send_header("Content-Length", "5")
                self.end_headers()
                self.wfile.write(b"hello")

            def do_POST(self):
                n = int(self.headers.get("Content-Length", 0))
                seen["post"] = (self.headers.get("Content-Type"), self.rfile.read(n))
                self.send_response(201)
                self.send_header("Content-Length", "0")
                self.end_headers()

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            out = Path(tempfile.mkdtemp())
            exe = self.build_selftest(out)
            run = wine_args([exe], out, {"SELFTEST_HTTP": "1", "HTTP_TEST_URL": f"http://127.0.0.1:{server.server_address[1]}"})
        finally:
            server.shutdown()
        self.assertIn("PASS http_requests_complete_through_winhttp", run.stdout, run.stdout[-800:] + run.stderr[-300:])
        self.assertEqual(seen["get"], ("/hello?q=a%20b", "abc"))
        self.assertEqual(seen["post"], ("text/plain", b"payload"))

@_both
class DllLoadCases:
    def test_built_dll_exports_every_reference_name_and_creates_the_client(self):
        out = Path(tempfile.mkdtemp())
        b = subprocess.run(["bash", str(ROOT / "shim/build.sh"), str(out), self.ARCH], capture_output=True, text=True)
        self.assertEqual(b.returncode, 0, b.stderr[-3000:])
        self.assertTrue((out / self.DLL).exists())
        loader = out / "loadtest.exe"
        r = subprocess.run([self.CC, "-Wall", str(ROOT / "tests/loadtest.c"), "-o", str(loader)], capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr[-2000:])
        shutil.copy(ROOT / "shim/exports.def", out / "exports.def")
        run = wine_args([loader, self.DLL, "exports.def"], out)
        self.assertIn("OK exports=43", run.stdout, run.stdout[-800:] + run.stderr[-500:])

if __name__ == "__main__":
    unittest.main()
