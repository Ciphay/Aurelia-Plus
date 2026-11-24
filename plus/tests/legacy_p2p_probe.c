#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>
#include "conformance/probe_table.h"

typedef void *(*CreateInterfaceFn)(const char *, int *);
#ifdef _WIN64
#define TC
#else
#define TC __attribute__((thiscall))
#endif
static int slot_of(const char *iface, const char *method) {
    for (int i = 0; i < PROBE_COUNT; i++)
        if (!strcmp(g_probe[i].iface, iface))
            for (int k = 0; k < g_probe[i].n; k++) if (!strcmp(g_probe[i].methods[k], method)) return k;
    fprintf(stderr, "no slot %s.%s\n", iface, method);
    exit(9);
}
#define VF(obj, iface, m) ((*(void ***)(obj))[slot_of(iface, m)])
typedef struct { uint8_t active, connecting; uint8_t err; uint8_t relay; int32_t queued_bytes, queued_packets; uint32_t remote_ip; uint16_t remote_port; } P2PState;
typedef struct { int32_t user; void *callback; int32_t id; uint8_t *param; int32_t cb; } CallbackMsg;

int main(int argc, char **argv) {
    if (argc < 4) { fprintf(stderr, "usage: legacy_p2p_probe <dll> <steamid> <seconds> [output]\n"); return 2; }
    FILE *out = argc > 4 ? fopen(argv[4], "w") : stdout;
    if (!out) return 2;
    uint64_t dest = strtoull(argv[2], NULL, 10);
    int secs = atoi(argv[3]);
    HMODULE h = LoadLibraryA(argv[1]);
    if (!h) { fprintf(out, "FAIL load %lu\n", GetLastError()); return 3; }
    CreateInterfaceFn ci = (CreateInterfaceFn)GetProcAddress(h, "CreateInterface");
    typedef int (*BGetCallback)(int, CallbackMsg *);
    typedef void (*FreeLast)(int);
    BGetCallback bget = (BGetCallback)GetProcAddress(h, "Steam_BGetCallback");
    FreeLast freelast = (FreeLast)GetProcAddress(h, "Steam_FreeLastCallback");
    const char *cv = "SteamClient023", *nv = "SteamNetworking006";
    void *client = ci(cv, NULL);
    int pipe = ((int(TC *)(void *))VF(client, cv, "CreateSteamPipe"))(client);
    int user = ((int(TC *)(void *, int))VF(client, cv, "ConnectToGlobalUser"))(client, pipe);
    void *net = ((void *(TC *)(void *, int, int, const char *))VF(client, cv, "GetISteamGenericInterface"))(client, user, pipe, nv);
    fprintf(out, "pipe %d user %d net %p destination %llu\n", pipe, user, net, (unsigned long long)dest);
    fflush(out);
    if (!net) return 4;
    for (int t = 0; t < secs * 10; t++) {
        if (t % 10 == 0) {
            char msg[64];
            int n = snprintf(msg, sizeof msg, "aurelia legacy %d", t / 10) + 1;
            int ok = ((int(TC *)(void *, uint64_t, const void *, uint32_t, int, int))VF(net, nv, "SendP2PPacket"))(net, dest, msg, (uint32_t)n, 2, 0);
            P2PState st = {0};
            int has = ((int(TC *)(void *, uint64_t, P2PState *))VF(net, nv, "GetP2PSessionState"))(net, dest, &st);
            fprintf(out, "SENT %d -> %d; STATE %d active=%d connecting=%d error=%d relay=%d queue=%d\n", t / 10, ok, has, st.active, st.connecting, st.err, st.relay, st.queued_packets);
            fflush(out);
        }
        uint32_t size = 0;
        while (((int(TC *)(void *, uint32_t *, int))VF(net, nv, "IsP2PPacketAvailable"))(net, &size, 0)) {
            char buf[2048];
            uint32_t got = 0;
            uint64_t from = 0;
            if (!((int(TC *)(void *, void *, uint32_t, uint32_t *, uint64_t *, int))VF(net, nv, "ReadP2PPacket"))(net, buf, sizeof buf, &got, &from, 0)) break;
            fprintf(out, "RECEIVED from %llu: \"%.*s\"\n", (unsigned long long)from, (int)got, buf);
            fflush(out);
        }
        if (bget) {
            CallbackMsg m;
            while (bget(pipe, &m)) {
                if (m.id == 1202 && m.cb >= 8) {
                    uint64_t from; memcpy(&from, m.param, 8);
                    int ok = ((int(TC *)(void *, uint64_t))VF(net, nv, "AcceptP2PSessionWithUser"))(net, from);
                    fprintf(out, "REQUEST for session from %llu: accepted %d\n", (unsigned long long)from, ok);
                } else if (m.id == 1203) {
                    fprintf(out, "FAILURE of session (1203): error %d\n", m.cb >= 9 ? m.param[8] : -1);
                }
                fflush(out);
                if (freelast) freelast(pipe);
            }
        }
        Sleep(100);
    }
    fprintf(out, "END\n");
    fclose(out);
    return 0;
}
