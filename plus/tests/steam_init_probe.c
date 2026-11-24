#include <stdio.h>
#include <stdlib.h>
#include <windows.h>

static FILE *out;
#define LOG(...) do { fprintf(out, __VA_ARGS__); fputc('\n', out); fflush(out); } while (0)

typedef unsigned char (*Fn_b)(void);
typedef void (*Fn_v)(void);
typedef void *(*Fn_p)(void);
typedef unsigned long long (*Fn_u64_p)(void *);
typedef unsigned int (*Fn_u32_p)(void *);

static void *first_export(HMODULE m, const char *const *names) {
    for (; *names; names++) { void *p = (void *)GetProcAddress(m, *names); if (p) { LOG("using %s", *names); return p; } }
    return NULL;
}

int main(void) {
    const char *log = getenv("AURELIA_PROBE_LOG"), *api = getenv("AURELIA_PROBE_API");
    out = fopen(log ? log : "steam-init-probe.log", "wb");
    if (!out || !api) return 2;
    HMODULE m = LoadLibraryA(api);
    if (!m) { LOG("FAIL LoadLibrary %lu", GetLastError()); return 1; }
    Fn_b init = (Fn_b)GetProcAddress(m, "SteamAPI_Init");
    if (!init) { LOG("FAIL no SteamAPI_Init"); return 1; }
    unsigned char ok = init();
    LOG("Init=%d", ok);
    if (!ok) return 1;
    static const char *const users[] = { "SteamAPI_SteamUser_v023", "SteamAPI_SteamUser_v022", "SteamAPI_SteamUser_v021", "SteamAPI_SteamUser_v020", NULL };
    static const char *const utils[] = { "SteamAPI_SteamUtils_v010", "SteamAPI_SteamUtils_v009", NULL };
    Fn_p get_user = first_export(m, users), get_utils = first_export(m, utils);
    Fn_u64_p get_id = (Fn_u64_p)GetProcAddress(m, "SteamAPI_ISteamUser_GetSteamID");
    Fn_u32_p get_app = (Fn_u32_p)GetProcAddress(m, "SteamAPI_ISteamUtils_GetAppID");
    void *user = get_user ? get_user() : NULL, *ut = get_utils ? get_utils() : NULL;
    if (user && get_id) LOG("steamid=%llu", get_id(user));
    if (ut && get_app) LOG("appid=%u", get_app(ut));
    Fn_v run = (Fn_v)GetProcAddress(m, "SteamAPI_RunCallbacks");
    if (run) { run(); LOG("RunCallbacks ok"); }
    Fn_v down = (Fn_v)GetProcAddress(m, "SteamAPI_Shutdown");
    if (down) down();
    LOG("DONE");
    return 0;
}
