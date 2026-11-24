import argparse
import json
import socket
import struct
import subprocess
import sys
import time
from pathlib import Path

AURELIA = Path(__file__).resolve().parents[2] / "aurelia/target/debug/aurelia"

def varint(v):
    out = b""
    while v >= 0x80:
        out += bytes([v & 0x7F | 0x80])
        v >>= 7
    return out + bytes([v])

def f_varint(n, v):
    return varint(n << 3) + varint(v & 0xFFFFFFFFFFFFFFFF)

def f_bytes(n, b):
    b = b.encode() if isinstance(b, str) else b
    return varint(n << 3 | 2) + varint(len(b)) + b

def f_fixed64(n, v):
    return varint(n << 3 | 1) + struct.pack("<Q", v)

def pbdump(b, depth=0):
    out, o = [], 0
    pad = "  " * depth
    while o < len(b):
        key, n = _rv(b, o)
        if n is None:
            out.append(pad + f"?? {b[o:].hex()}")
            break
        o = n
        fld, wt = key >> 3, key & 7
        if wt == 0:
            v, o = _rv(b, o)
            out.append(pad + f"{fld}: {v}")
        elif wt == 1:
            out.append(pad + f"{fld}: fixed64 {struct.unpack_from('<Q', b, o)[0]}")
            o += 8
        elif wt == 5:
            out.append(pad + f"{fld}: fixed32 {struct.unpack_from('<I', b, o)[0]}")
            o += 4
        elif wt == 2:
            ln, o = _rv(b, o)
            chunk = b[o:o + ln]
            o += ln
            txt = chunk.decode("utf-8", "replace")
            if chunk and all(32 <= c < 127 or c in (9, 10) for c in chunk):
                out.append(pad + f'{fld}: "{txt}"')
            else:
                nested = pbdump(chunk, depth + 1) if chunk and _parses(chunk) else None
                out.append(pad + f"{fld}: " + (f"{{\n{nested}\n{pad}}}" if nested else f"bytes[{ln}] {chunk[:48].hex()}"))
        else:
            out.append(pad + f"?? wt{wt}")
            break
    return "\n".join(out)

def _rv(b, o):
    v = shift = 0
    while o < len(b):
        c = b[o]
        o += 1
        v |= (c & 0x7F) << shift
        if not c & 0x80:
            return v, o
        shift += 7
    return None, None

def _parses(b):
    o = 0
    while o < len(b):
        key, o2 = _rv(b, o)
        if o2 is None or key & 7 in (3, 4, 6, 7) or key >> 3 == 0:
            return False
        o, wt = o2, key & 7
        if wt == 0:
            _, o = _rv(b, o)
            if o is None:
                return False
        elif wt == 1:
            o += 8
        elif wt == 5:
            o += 4
        else:
            ln, o = _rv(b, o)
            if o is None:
                return False
            o += ln
        if o > len(b):
            return False
    return True

class Bridge:
    def __init__(self, appid, serve=True, port=None, token=None):
        self.proc = None
        if serve:
            self.proc = subprocess.Popen([str(AURELIA), "--json", "bridge", "serve", "--app", str(appid)], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            line = self.proc.stdout.readline()
            if not line:
                raise SystemExit("the bridge did not start: " + self.proc.stderr.read()[:600])
            info = json.loads(line)
            port, token = info["port"], bytes.fromhex(info["token"])
        self.port, self.token = port, token
        self.seq = 0
        self.sock = self._connect()

    def _connect(self):
        s = socket.create_connection(("127.0.0.1", self.port), timeout=5)
        body = b"\x01" + struct.pack("<H", 1) + self.token
        s.sendall(struct.pack("<I", len(body)) + body)
        self._read(s)
        return s

    @staticmethod
    def _read(s):
        hdr = b""
        while len(hdr) < 4:
            hdr += s.recv(4 - len(hdr))
        n = struct.unpack("<I", hdr)[0]
        body = b""
        while len(body) < n:
            body += s.recv(n - len(body))
        return body

    def call(self, slot, payload=b"", timeout=70):
        self.seq += 1
        body = b"\x05" + struct.pack("<IHH", self.seq, 0xFFFF, slot) + payload
        self.sock.settimeout(timeout)
        self.sock.sendall(struct.pack("<I", len(body)) + body)
        while True:
            r = self._read(self.sock)
            if r[0] == 6 and struct.unpack_from("<I", r, 1)[0] == self.seq:
                return struct.unpack_from("<i", r, 5)[0], r[9:]

    def relay(self, emsg, body, job="", reply=True, wait_ms=15000, routed=True, gs=False):
        p = bytes([(1 if reply else 0) | 2 | (4 if routed else 0) | (8 if gs else 0), 0, 0, 0]) + struct.pack("<iIH", emsg, wait_ms, len(job)) + job.encode() + body
        status, out = self.call(7, p)
        if status != 0 or len(out) < 8:
            return status, None, None, b""
        e, r = struct.unpack_from("<ii", out)
        return status, e, r, out[8:]

    def close(self):
        if self.proc:
            self.proc.terminate()
            try:
                self.proc.wait(8)
            except subprocess.TimeoutExpired:
                self.proc.kill()

def _top(body):
    out, o = [], 0
    while o < len(body):
        key, o = _rv(body, o)
        fld, wt = key >> 3, key & 7
        if wt == 0:
            v, o = _rv(body, o)
        elif wt == 1:
            v = struct.unpack_from("<Q", body, o)[0]; o += 8
        elif wt == 5:
            v = struct.unpack_from("<I", body, o)[0]; o += 4
        else:
            n, o = _rv(body, o)
            v = body[o:o + n]; o += n
        out.append((fld, v))
    return out

def struct_unpack_fixed64(body, field):
    o = 0
    while o < len(body):
        key, o = _rv(body, o)
        fld, wt = key >> 3, key & 7
        if wt == 1:
            if fld == field:
                return struct.unpack_from("<Q", body, o)[0]
            o += 8
        elif wt == 0:
            _, o = _rv(body, o)
        elif wt == 5:
            o += 4
        else:
            n, o = _rv(body, o)
            o += n
    raise KeyError(field)

def show(label, res):
    status, emsg, eresult, body = res
    print(f"== {label}: status={status} emsg={emsg} eresult={eresult} body={len(body)}B")
    if body:
        print(pbdump(body))

def scenario(b, appid, name, args):
    if name == "lb-find":
        show("FindOrCreate", b.relay(5416, f_varint(1, appid) + f_varint(2, 0) + f_varint(3, 0) + f_varint(4, 0) + f_bytes(5, args[0])))
    elif name == "lb-create":
        show("FindOrCreate", b.relay(5416, f_varint(1, appid) + f_varint(2, int(args[1])) + f_varint(3, int(args[2])) + f_varint(4, 1) + f_bytes(5, args[0])))
    elif name == "lb-entries":
        show("GetLBEntries", b.relay(5418, f_varint(1, appid) + f_varint(2, int(args[0])) + f_varint(3, int(args[2])) + f_varint(4, int(args[3])) + f_varint(5, int(args[1]))))
    elif name == "lobby-list":
        show("GetLobbyList", b.relay(6607, f_varint(1, appid) + f_varint(3, 50) + f_varint(4, 0)))
    elif name == "lobby-filter":
        flt = f_bytes(1, args[2]) + f_bytes(2, args[3]) + f_varint(3, int(args[1])) + f_varint(4, int(args[0]))
        res = b.relay(6607, f_varint(1, appid) + f_varint(3, 50) + f_varint(4, 0) + f_bytes(6, flt))
        n = sum(1 for k, _ in _top(res[3]) if k == 4)
        print(f"== type {args[0]} cmp {args[1]} {args[2]}={args[3]}: eresult={res[2]} lobbies={n}")
    elif name == "lobby-numeric":
        def kv(d):
            return b"\x00\x00" + b"".join(b"\x01" + k.encode() + b"\0" + v.encode() + b"\0" for k, v in d.items()) + b"\x08\x08"
        res = b.relay(6601, f_varint(1, appid) + f_varint(2, 4) + f_varint(3, 2) + f_varint(4, 0) + f_bytes(7, kv({"lvprobe": "5", "tagprobe": "zz-probe"})) + f_bytes(8, "probe"))
        lobby = struct_unpack_fixed64(res[3], 2)
        time.sleep(2)
        for ftype, cmp_, val in ((0, 0, "5"), (1, 0, "5"), (1, 2, "4"), (1, 1, "9"), (1, 2, "6"), (2, 0, "5"), (3, 0, "5")):
            flt = f_bytes(1, "lvprobe") + f_bytes(2, val) + f_varint(3, cmp_) + f_varint(4, ftype)
            r = b.relay(6607, f_varint(1, appid) + f_varint(3, 50) + f_varint(4, 0) + f_bytes(6, flt))
            ids = [struct_unpack_fixed64(v, 1) for k, v in _top(r[3]) if k == 4]
            print(f"type {ftype} cmp {cmp_} value {val}: {len(ids)} lobbies, mine {'yes' if lobby in ids else 'no'}")
        show("Leave", b.relay(6605, f_varint(1, appid) + f_fixed64(2, lobby)))
    elif name == "lobby-push":
        def kv(d):
            return b"\x00\x00" + b"".join(b"\x01" + k.encode() + b"\0" + v.encode() + b"\0" for k, v in d.items()) + b"\x08\x08"
        for e in (6612, 6614, 6618, 6619, 6620):
            b.call(8, struct.pack("<i", e))
        res = b.relay(6601, f_varint(1, appid) + f_varint(2, 4) + f_varint(3, 2) + f_varint(4, 0) + f_bytes(7, kv({"a": "1"})) + f_bytes(8, "probe"))
        show("Create", res)
        lobby = struct_unpack_fixed64(res[3], 2)
        def drain(label):
            time.sleep(1.5)
            st, out = b.call(9)
            o = 0
            while o < len(out):
                e, n = struct.unpack_from("<iI", out, o)
                print(f"-- push after {label}: emsg {e}\n{pbdump(out[o + 8:o + 8 + n])}")
                o += 8 + n
            print(f"-- end ({label})")
        drain("create")
        show("SetLobbyData", b.relay(6609, f_varint(1, appid) + f_fixed64(2, lobby) + f_fixed64(3, 0) + f_varint(4, 4) + f_varint(5, 2) + f_varint(6, 0) + f_bytes(7, kv({"a": "2", "b": "x"}))))
        drain("SetLobbyData")
        show("SetMemberData", b.relay(6609, f_varint(1, appid) + f_fixed64(2, lobby) + f_fixed64(3, 7 and 76561198354875885) + f_bytes(7, kv({"m": "1"}))))
        drain("SetMemberData(with id)")
        show("Chat", b.relay(6613, f_varint(1, appid) + f_fixed64(2, lobby) + f_bytes(4, b"hi\0"), reply=False))
        drain("chat")
        show("Leave", b.relay(6605, f_varint(1, appid) + f_fixed64(2, lobby)))
        drain("leave")
    elif name == "lobby-flags":
        def kv(d):
            return b"\x00\x00" + b"".join(b"\x01" + k.encode() + b"\0" + v.encode() + b"\0" for k, v in d.items()) + b"\x08\x08"
        res = b.relay(6601, f_varint(1, appid) + f_varint(2, 4) + f_varint(3, 2) + f_varint(4, 0) + f_bytes(7, kv({"flagprobe": "1"})) + f_bytes(8, "probe"))
        lobby = struct_unpack_fixed64(res[3], 2)
        me = 76561198354875885
        def visible():
            flt = f_bytes(1, "flagprobe") + f_bytes(2, "1") + f_varint(3, 0) + f_varint(4, 0)
            r = b.relay(6607, f_varint(1, appid) + f_varint(3, 50) + f_varint(4, 0) + f_bytes(6, flt))
            return lobby in [struct_unpack_fixed64(v, 1) for k, v in _top(r[3]) if k == 4]
        time.sleep(2)
        print("flags 0 visible:", visible())
        for fl in (1, 2, 4):
            b.relay(6609, f_varint(1, appid) + f_fixed64(2, lobby) + f_fixed64(3, 0) + f_varint(4, 4) + f_varint(5, 2) + f_varint(6, fl) + f_bytes(7, kv({"flagprobe": "1"})))
            time.sleep(2)
            print(f"flags {fl} visible:", visible())
        show("Leave", b.relay(6605, f_varint(1, appid) + f_fixed64(2, lobby)))
    elif name == "lobby-joinbit":
        def kv(d):
            return b"\x00\x00" + b"".join(b"\x01" + k.encode() + b"\0" + v.encode() + b"\0" for k, v in d.items()) + b"\x08\x08"
        res = b.relay(6601, f_varint(1, appid) + f_varint(2, 4) + f_varint(3, 2) + f_varint(4, 0) + f_bytes(8, "probe"))
        lobby = struct_unpack_fixed64(res[3], 2)
        for fl in (0, 1, 2, 4, 8):
            b.relay(6609, f_varint(1, appid) + f_fixed64(2, lobby) + f_fixed64(3, 0) + f_varint(4, 4) + f_varint(5, 2) + f_varint(6, fl) + f_bytes(7, kv({"a": "1"})))
            time.sleep(1)
            j = b.relay(6603, f_varint(1, appid) + f_fixed64(2, lobby) + f_bytes(3, "probe"))
            f = dict((k, v) for k, v in _top(j[3]) if k in (3, 6))
            print(f"flags {fl}: join response={f.get(3)} flags_back={f.get(6)}")
        show("Leave", b.relay(6605, f_varint(1, appid) + f_fixed64(2, lobby)))
    elif name == "lobby-create":
        meta = bytes.fromhex(args[0]) if args else b""
        body = f_varint(1, appid) + f_varint(2, 4) + f_varint(3, 1) + f_varint(4, 0) + (f_bytes(7, meta) if meta else b"") + f_bytes(8, "probe")
        res = b.relay(6601, body)
        show("CreateLobby", res)
        if res[3] and len(res[3]) > 8:
            lobby = dict((n, v) for n, v in [(1, 0)])
            time.sleep(1)
            for st in range(2):
                m = b.relay(6609, f_varint(1, appid) + f_fixed64(2, struct_unpack_fixed64(res[3], 2)) + f_varint(4, 4) + f_varint(5, 1) + f_bytes(7, meta))
                show("SetLobbyData", m)
            show("GetLobbyData", b.relay(6611, f_varint(1, appid) + f_fixed64(2, struct_unpack_fixed64(res[3], 2))))
            show("Leave", b.relay(6605, f_varint(1, appid) + f_fixed64(2, struct_unpack_fixed64(res[3], 2))))
    elif name == "ugc":
        payload = bytes([int(args[0])]) + struct.pack("<IQ", appid, int(args[1]) if len(args) > 1 else 0)
        status, out = b.call(10, payload, timeout=900)
        print(f"== ugc op {args[0]}: status={status} {len(out)}B {out[:80].hex()}")
    elif name == "servers":
        body = f_varint(1, appid) + f_varint(2, 0) + f_varint(3, 255) + f_bytes(4, args[0]) + f_varint(5, int(args[1]) if len(args) > 1 else 5)
        show("GMSServerQuery", b.relay(6403, body))
    elif name == "cloudfile":
        nm = args[1].encode()
        status, out = b.call(11, bytes([1]) + struct.pack("<IH", int(args[0]), len(nm)) + nm)
        print(f"== cloudfile: status={status} {len(out)}B")
        print(out.decode("utf-8", "replace"))
    elif name == "ugcquery":
        body = (f_varint(1, int(args[0])) + f_varint(2, 1) + f_varint(3, int(args[1]) if len(args) > 1 else 2) + f_varint(5, appid) + f_varint(37, 1)
                + f_varint(17, 1) + f_varint(18, 1) + f_varint(19, 1) + f_varint(20, 1) + f_varint(21, 1) + f_varint(32, 1) + f_bytes(39, "*"))
        show("QueryFiles", b.relay(151, body, job="PublishedFile.QueryFiles#1"))
    elif name == "ugcuser":
        sid = int(args[1]) if len(args) > 1 else 76561198354875885
        body = f_fixed64(1, sid) + f_varint(2, appid) + f_varint(4, 1) + f_varint(5, 3) + f_bytes(6, args[0]) + f_varint(17, 0) + f_varint(20, 1)
        show("GetUserFiles", b.relay(151, body, job="PublishedFile.GetUserFiles#1"))
    elif name == "gslogon":
        status, out = b.call(12, bytes([1]))
        print(f"== gs logon: status={status} steamid={struct.unpack('<Q', out)[0] if len(out) == 8 else out}")
    elif name == "gsregister":
        status, out = b.call(12, bytes([1]))
        sid = struct.unpack('<Q', out)[0]
        print("gs steamid", sid)
        stype = (f_varint(1, appid) + f_varint(2, 4 | 1) + f_varint(4, 27099) + f_bytes(5, "spacewar") + f_bytes(6, "1.0.0.0") + f_varint(7, 27099))
        print("908:", b.relay(908, stype, reply=False, gs=True)[0])
        data = (f_varint(3, 27099) + f_varint(4, 27099) + f_bytes(22, "AureliaPlus probe") + f_varint(6, appid) + f_bytes(7, "spacewar") + f_bytes(8, "1.0.0.0")
                + f_bytes(9, "spacewar") + f_bytes(21, "probe_map") + f_varint(12, 4) + f_varint(13, 0) + f_varint(16, 1) + f_bytes(17, "l") + f_bytes(10, "255"))
        print("906:", b.relay(906, data, reply=False, gs=True)[0])
        for i in range(6):
            time.sleep(4)
            res = b.relay(6403, f_varint(1, appid) + f_varint(2, 0) + f_varint(3, 255) + f_bytes(4, "\\appid\\%d" % appid) + f_varint(5, 500))
            ids = [struct_unpack_fixed64(v, 6) for k, v in _top(res[3]) if k == 1 and b"\x31" is not None] if res[3] else []
            n = 1 if sid in ids else 0
            print("  total", len(ids))
            print(f"search {i}: servers={n} eresult={res[2]}")
            if n > 0:
                show("list", res)
                break
        b.call(12, bytes([2]))
    elif name == "gsvariants":
        status, out = b.call(12, bytes([1]))
        sid = struct.unpack('<QI', out)[0:1][0] if False else struct.unpack('<Q', out[:8])[0]
        ip = struct.unpack('<I', out[8:12])[0]
        print("gs steamid", sid, "ip", socket.inet_ntoa(struct.pack(">I", ip)))
        stype = (f_varint(1, appid) + f_varint(2, 4 | 1) + f_varint(3, ip) + f_varint(4, 27099) + f_bytes(5, "spacewar") + f_bytes(6, "1.0.0.0") + f_varint(7, 27099))
        b.relay(908, stype, reply=False, gs=True)
        data = (f_varint(3, 27099) + f_varint(4, 27099) + f_bytes(22, "AureliaPlus probe") + f_varint(6, appid) + f_bytes(7, "spacewar") + f_bytes(8, "1.0.0.0")
                + f_bytes(9, "spacewar") + f_bytes(21, "probe_map") + f_varint(12, 4) + f_varint(13, 0) + f_varint(16, 1) + f_bytes(17, "w") + f_varint(24, 1))
        for em in (4331,):
            print("sending", em, b.relay(em, data, reply=False, gs=True)[0])
            time.sleep(6)
            res = b.relay(6403, f_varint(1, appid) + f_varint(2, 0) + f_varint(3, 255) + f_bytes(4, "\\appid\\%d\\map\\probe_map" % appid) + f_varint(5, 500))
            ids = []
            for k, v in _top(res[3]) if res[3] else []:
                if k == 1:
                    try:
                        ids.append(struct_unpack_fixed64(v, 6))
                    except KeyError:
                        pass
            print(f"  list: {len(ids)} servers; mine {'YES' if sid in ids else 'no'}")
        b.call(12, bytes([2]))
    elif name == "p2p-self":
        me = 76561198354875885
        for e in (5434, 5435, 813):
            b.call(8, struct.pack("<i", e))
        body = f_fixed64(1, me) + f_fixed64(2, me) + f_varint(3, appid) + f_bytes(4, b"cand") + f_bytes(6, b"hello-rendezvous")
        print("sent:", b.relay(5434, body, reply=False)[0])
        for i in range(5):
            time.sleep(1.5)
            st, out = b.call(9)
            o = 0
            while o < len(out):
                e, n = struct.unpack_from("<iI", out, o)
                print(f"push emsg {e}:\n{pbdump(out[o + 8:o + 8 + n])}")
                o += 8 + n
    elif name == "gstoken":
        status, out = b.call(12, bytes([1]) + args[0].encode())
        print(f"== gs logon with token: status={status} {out.hex()}")
    elif name == "players":
        show("NumberOfCurrentPlayers", b.relay(5592, f_varint(1, appid)))
    elif name == "global-stats":
        show("GetGlobalStats", b.relay(7901, f_varint(1, appid) + f_varint(2, int(args[0]) if args else 0)))
    else:
        raise SystemExit("unknown scenario: " + name)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--app", type=int, required=True)
    ap.add_argument("what", choices=["scenario", "raw", "service"])
    ap.add_argument("args", nargs="*")
    a = ap.parse_args()
    b = Bridge(a.app)
    try:
        time.sleep(1)
        if a.what == "scenario":
            scenario(b, a.app, a.args[0], a.args[1:])
        elif a.what == "raw":
            show("raw", b.relay(int(a.args[0]), bytes.fromhex(a.args[1].replace(" ", ""))))
        else:
            show("service", b.relay(151, bytes.fromhex(a.args[1].replace(" ", "")), job=a.args[0]))
    finally:
        b.close()

if __name__ == "__main__":
    main()
