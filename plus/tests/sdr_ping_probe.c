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

int main(int argc, char **argv) {
    if (argc < 3) return 2;
    if (argc > 3 && !freopen(argv[3], "w", stdout)) return 2;
    HMODULE h = LoadLibraryA(argv[1]);
    if (!h) { printf("FAIL load %lu\n", GetLastError()); return 3; }
    CreateInterfaceFn ci = (CreateInterfaceFn)GetProcAddress(h, "CreateInterface");
    const char *cv = "SteamClient023", *uv = "SteamNetworkingUtils004";
    void *client = ci(cv, NULL);
    int pipe = ((int(TC *)(void *))VF(client, cv, "CreateSteamPipe"))(client);
    int user = ((int(TC *)(void *, int))VF(client, cv, "ConnectToGlobalUser"))(client, pipe);
    void *u = ((void *(TC *)(void *, int, int, const char *))VF(client, cv, "GetISteamGenericInterface"))(client, user, pipe, uv);
    printf("utils %p\n", u);
    fflush(stdout);
    if (!u) return 4;
    ((void(TC *)(void *))VF(u, uv, "InitRelayNetworkAccess"))(u);
    printf("init\n");
    fflush(stdout);
    int secs = atoi(argv[2]);
    for (int t = 0; t < secs; t++) {
        struct { int32_t avail, ping_in_progress, avail_config, avail_relay; char msg[256]; } st;
        memset(&st, 0, sizeof st);
        int r = ((int(TC *)(void *, void *))VF(u, uv, "GetRelayNetworkStatus"))(u, &st);
        printf("status %d\n", r);
        fflush(stdout);
        uint8_t loc[512];
        float age = ((float(TC *)(void *, void *))VF(u, uv, "GetLocalPingLocation"))(u, loc);
        char str[1024] = "";
        if (age >= 0) ((void(TC *)(void *, const void *, char *, int))VF(u, uv, "ConvertPingLocationToString"))(u, loc, str, sizeof str);
        int pending = ((int(TC *)(void *))VF(u, uv, "IsPingMeasurementInProgress"))(u);
        printf("STATE %d (%d %d %d in_progress=%d measuring=%d) %s\n", r, st.avail, st.avail_config, st.avail_relay, st.ping_in_progress, pending, st.msg);
        if (age >= 0) printf("LOCATION age=%.1f %s\n", age, str);
        fflush(stdout);
        Sleep(1000);
    }
    printf("END\n");
    return 0;
}
