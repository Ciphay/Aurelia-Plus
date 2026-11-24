#include <stdio.h>
#include <stdlib.h>
#include <windows.h>

typedef unsigned char (*Fn_b)(void);
typedef void (*Fn_v)(void);
typedef void *(*Fn_p)(void);
typedef unsigned long long (*Fn_id)(void *);
typedef unsigned (*Fn_auth)(void *, void *, int, unsigned *, void *);

int main(void) {
    const char *log = getenv("AURELIA_PROBE_LOG"), *api = getenv("AURELIA_PROBE_API");
    FILE *out = fopen(log ? log : "ticket-probe.log", "wb");
    if (!out || !api) return 2;
    HMODULE m = LoadLibraryA(api);
    if (!m) { fprintf(out, "FAIL load\n"); return 1; }
    Fn_b init = (Fn_b)GetProcAddress(m, "SteamAPI_Init");
    unsigned char ok = init ? init() : 0;
    fprintf(out, "Init=%d\n", ok);
    fflush(out);
    if (!ok) return 1;
    Fn_v run = (Fn_v)GetProcAddress(m, "SteamAPI_RunCallbacks");
    for (int i = 0; i < 6; i++) { if (run) run(); Sleep(500); }
    static const char *const users[] = { "SteamAPI_SteamUser_v023", "SteamAPI_SteamUser_v022", "SteamAPI_SteamUser_v021", NULL };
    Fn_p get_user = NULL;
    for (int i = 0; users[i] && !get_user; i++) { get_user = (Fn_p)GetProcAddress(m, users[i]); if (get_user) fprintf(out, "using %s\n", users[i]); }
    void *user = get_user ? get_user() : NULL;
    Fn_id id = (Fn_id)GetProcAddress(m, "SteamAPI_ISteamUser_GetSteamID");
    Fn_auth auth = (Fn_auth)GetProcAddress(m, "SteamAPI_ISteamUser_GetAuthSessionTicket");
    if (!user || !auth) { fprintf(out, "FAIL no user/auth\n"); return 1; }
    if (id) fprintf(out, "steamid=%llu\n", id(user));
    unsigned char buf[2048];
    unsigned n = 0;
    unsigned h = auth(user, buf, sizeof buf, &n, NULL);
    for (int i = 0; i < 6; i++) { if (run) run(); Sleep(300); }
    fprintf(out, "handle=%u len=%u\nhex=", h, n);
    for (unsigned i = 0; i < n; i++) fprintf(out, "%02x", buf[i]);
    fprintf(out, "\nDONE\n");
    fflush(out);
    const char *hold = getenv("AURELIA_PROBE_HOLD");
    if (hold) Sleep((DWORD)atoi(hold) * 1000);
    return 0;
}
