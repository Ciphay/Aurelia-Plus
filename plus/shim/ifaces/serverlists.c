#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <windows.h>

#include "../shim.h"

enum { SLOT_CLOUDFILE = 11, OP_READ = 1, OP_WRITE = 2, STORE_APP = 7, MAX_ENTRIES = 512, TTL_MS = 60000 };
#define LIST_FILE "serverbrowser_hist.vdf"

static ShimServerEntry g_entries[MAX_ENTRIES];
static int g_n;
static bool g_loaded, g_parsed_ok;
static DWORD g_loaded_at;
static SRWLOCK g_lock = SRWLOCK_INIT;

static const char *tok(const char *p, char *out, size_t cap, int *kind) {
    while (*p && (*p == ' ' || *p == '\t' || *p == '\r' || *p == '\n')) p++;
    if (!*p) { *kind = 0; return p; }
    if (*p == '{') { *kind = 2; return p + 1; }
    if (*p == '}') { *kind = 3; return p + 1; }
    if (*p != '"') { *kind = 0; return p; }
    size_t n = 0;
    for (p++; *p && *p != '"'; p++) {
        char c = *p;
        if (c == '\\' && p[1]) { p++; c = *p == 'n' ? '\n' : *p == 't' ? '\t' : *p; }
        if (n + 1 < cap) out[n++] = c;
    }
    out[n] = 0;
    *kind = 1;
    return *p == '"' ? p + 1 : p;
}

static bool parse_address(const char *s, uint32_t *ip, uint16_t *port) {
    unsigned a, b, c, d, p;
    if (sscanf(s, "%u.%u.%u.%u:%u", &a, &b, &c, &d, &p) != 5 || a > 255 || b > 255 || c > 255 || d > 255 || p > 65535) return false;
    *ip = (a << 24) | (b << 16) | (c << 8) | d;
    *port = (uint16_t)p;
    return true;
}

static const char *read_entry(const char *p, uint32_t flags, ShimServerEntry *out, bool *ok) {
    char key[128], val[256];
    int kind;
    ShimServerEntry e;
    memset(&e, 0, sizeof e);
    e.flags = flags;
    bool have_addr = false;
    uint32_t query = 0;
    for (;;) {
        p = tok(p, key, sizeof key, &kind);
        if (kind != 1) break;
        const char *q = tok(p, val, sizeof val, &kind);
        if (kind == 2) {
            int depth = 1;
            while (depth && *q) { char tmp[8]; int k; q = tok(q, tmp, sizeof tmp, &k); if (k == 2) depth++; else if (k == 3) depth--; else if (k == 0) break; }
            p = q;
            continue;
        }
        p = q;
        if (kind != 1) break;
        if (!_stricmp(key, "address")) have_addr = parse_address(val, &e.ip, &e.conn_port);
        else if (!_stricmp(key, "appid")) e.appid = (uint32_t)strtoul(val, NULL, 10);
        else if (!_stricmp(key, "LastPlayed")) e.last_played = (uint32_t)strtoul(val, NULL, 10);
        else if (!_stricmp(key, "queryport")) query = (uint32_t)strtoul(val, NULL, 10);
    }
    e.query_port = (uint16_t)(query ? query : e.conn_port);
    *ok = have_addr;
    *out = e;
    return p;
}

static bool parse_list(const char *text, ShimServerEntry *out, int *n) {
    char key[128];
    int kind;
    const char *p = tok(text, key, sizeof key, &kind);
    if (kind != 1 || _stricmp(key, "Filters")) return false;
    p = tok(p, key, sizeof key, &kind);
    if (kind != 2) return false;
    *n = 0;
    for (;;) {
        p = tok(p, key, sizeof key, &kind);
        if (kind != 1) break;
        uint32_t flags = !_stricmp(key, "favorites") ? 1 : !_stricmp(key, "history") ? 2 : 0;
        p = tok(p, key, sizeof key, &kind);
        if (kind != 2) return false;
        for (;;) {
            p = tok(p, key, sizeof key, &kind);
            if (kind != 1) break;
            p = tok(p, key, sizeof key, &kind);
            if (kind != 2) return false;
            bool ok;
            ShimServerEntry e;
            p = read_entry(p, flags, &e, &ok);
            if (ok && flags && *n < MAX_ENTRIES) out[(*n)++] = e;
        }
    }
    return true;
}

static size_t write_section(char *buf, size_t cap, size_t o, const char *name, uint32_t flag) {
    o += (size_t)snprintf(buf + o, cap - o, "\t\"%s\"\n\t{\n", name);
    int idx = 0;
    for (int i = 0; i < g_n; i++) {
        if (!(g_entries[i].flags & flag)) continue;
        const ShimServerEntry *e = &g_entries[i];
        char addr[40];
        snprintf(addr, sizeof addr, "%u.%u.%u.%u:%u", e->ip >> 24, (e->ip >> 16) & 255, (e->ip >> 8) & 255, e->ip & 255, e->conn_port);
        o += (size_t)snprintf(buf + o, cap - o, "\t\t\"%d\"\n\t\t{\n\t\t\t\"name\"\t\t\"%s\"\n\t\t\t\"address\"\t\t\"%s\"\n\t\t\t\"LastPlayed\"\t\t\"%u\"\n\t\t\t\"appid\"\t\t\"%u\"\n\t\t\t\"accountid\"\t\t\"0\"\n",
                              ++idx, addr, addr, e->last_played, e->appid);
        if (e->query_port != e->conn_port) o += (size_t)snprintf(buf + o, cap - o, "\t\t\t\"queryport\"\t\t\"%u\"\n", e->query_port);
        o += (size_t)snprintf(buf + o, cap - o, "\t\t}\n");
    }
    o += (size_t)snprintf(buf + o, cap - o, "\t}\n");
    return o;
}

static bool cloud_call(uint8_t op, const char *data, size_t len, char **reply) {
    size_t nl = strlen(LIST_FILE);
    uint8_t *req = malloc(7 + nl + len);
    if (!req) return false;
    uint32_t app = STORE_APP;
    req[0] = op;
    memcpy(req + 1, &app, 4);
    req[5] = (uint8_t)nl;
    req[6] = 0;
    memcpy(req + 7, LIST_FILE, nl);
    if (len) memcpy(req + 7 + nl, data, len);
    size_t cap = 1u << 20, got = 0;
    uint8_t *buf = reply ? malloc(cap) : NULL;
    int32_t status = -1;
    bool ok = bridge_call_fresh(0xFFFF, SLOT_CLOUDFILE, req, 7 + nl + len, &status, buf, reply ? cap - 1 : 0, &got, 20000) && status == 0;
    free(req);
    if (reply) {
        if (ok && buf) { buf[got] = 0; *reply = (char *)buf; } else { free(buf); *reply = NULL; ok = false; }
    }
    return ok;
}

static void load_now(bool force);
static volatile LONG g_refreshing;
static DWORD WINAPI refresh_thread(LPVOID arg) {
    load_now(arg != NULL);
    InterlockedExchange(&g_refreshing, 0);
    return 0;
}

static void load(bool force) {
    AcquireSRWLockExclusive(&g_lock);
    bool have = g_loaded;
    bool fresh = have && !force && GetTickCount() - g_loaded_at < TTL_MS;
    ReleaseSRWLockExclusive(&g_lock);
    if (fresh) return;
    if (have && !force) {
        if (!InterlockedExchange(&g_refreshing, 1)) {
            HANDLE t = CreateThread(NULL, 0, refresh_thread, NULL, 0, NULL);
            if (t) CloseHandle(t); else InterlockedExchange(&g_refreshing, 0);
        }
        return;
    }
    if (!have && g_refreshing) {
        for (int i = 0; i < 300; i++) {
            AcquireSRWLockExclusive(&g_lock);
            bool done = g_loaded;
            ReleaseSRWLockExclusive(&g_lock);
            if (done) return;
            Sleep(10);
        }
    }
    load_now(force);
}

void serverlists_preload(void) {
    static volatile LONG once;
    if (InterlockedExchange(&once, 1)) return;
    if (!InterlockedExchange(&g_refreshing, 1)) {
        HANDLE t = CreateThread(NULL, 0, refresh_thread, (LPVOID)1, 0, NULL);
        if (t) CloseHandle(t); else InterlockedExchange(&g_refreshing, 0);
    }
}

static void load_now(bool force) {
    (void)force;
    char *text = NULL;
    if (!cloud_call(OP_READ, NULL, 0, &text)) return;
    ShimServerEntry *tmp = calloc(MAX_ENTRIES, sizeof *tmp);
    int n = 0;
    bool parsed = tmp && (!*text || parse_list(text, tmp, &n));
    AcquireSRWLockExclusive(&g_lock);
    if (parsed) { memcpy(g_entries, tmp, (size_t)n * sizeof *tmp); g_n = n; }
    g_parsed_ok = parsed;
    g_loaded = true;
    g_loaded_at = GetTickCount();
    ReleaseSRWLockExclusive(&g_lock);
    free(tmp);
    free(text);
}

static DWORD WINAPI write_thread(LPVOID arg) {
    char *text = arg;
    cloud_call(OP_WRITE, text, strlen(text), NULL);
    free(text);
    return 0;
}

static void save(void) {
    AcquireSRWLockExclusive(&g_lock);
    size_t cap = 256 + (size_t)g_n * 400;
    char *text = g_parsed_ok ? malloc(cap) : NULL;
    if (text) {
        size_t o = (size_t)snprintf(text, cap, "\"Filters\"\n{\n");
        o = write_section(text, cap, o, "favorites", 1);
        o = write_section(text, cap, o, "history", 2);
        snprintf(text + o, cap - o, "}\n");
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (!text) return;
    HANDLE t = CreateThread(NULL, 0, write_thread, text, 0, NULL);
    if (t) CloseHandle(t); else free(text);
}

int shim_serverlist_get(ShimServerEntry *out, int max, uint32_t flag, uint32_t appid) {
    load(false);
    int n = 0;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < g_n && n < max; i++)
        if ((g_entries[i].flags & flag) && (!appid || !g_entries[i].appid || g_entries[i].appid == appid)) out[n++] = g_entries[i];
    ReleaseSRWLockExclusive(&g_lock);
    return n;
}

static int SHIM_TC favorite_count(void *self) {
    (void)self;
    load(false);
    AcquireSRWLockExclusive(&g_lock);
    int n = g_n;
    ReleaseSRWLockExclusive(&g_lock);
    return n;
}

static uint64_t SHIM_TC favorite_get(void *self, int i, uint32_t *app, uint32_t *ip, uint16_t *conn, uint16_t *query, uint32_t *flags, uint32_t *when) {
    (void)self;
    load(false);
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    if (i >= 0 && i < g_n) {
        const ShimServerEntry *e = &g_entries[i];
        if (app) *app = e->appid;
        if (ip) *ip = e->ip;
        if (conn) *conn = e->conn_port;
        if (query) *query = e->query_port;
        if (flags) *flags = e->flags;
        if (when) *when = e->last_played;
        ok = true;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static int SHIM_TC favorite_add(void *self, uint32_t app, uint32_t ip, uint16_t conn, uint16_t query, uint32_t flags, uint32_t when) {
    (void)self;
    if (!ip) return 0;
    load(false);
    int idx = -1;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < g_n; i++) {
        ShimServerEntry *e = &g_entries[i];
        if (e->ip == ip && e->conn_port == conn && e->appid == app) { e->flags |= flags; e->last_played = when; e->query_port = query; idx = i; break; }
    }
    if (idx < 0 && g_n < MAX_ENTRIES) {
        g_entries[g_n] = (ShimServerEntry){ app, ip, conn, query, flags, when };
        idx = g_n++;
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (idx >= 0) save();
    return idx;
}

static uint64_t SHIM_TC favorite_remove(void *self, uint32_t app, uint32_t ip, uint16_t conn, uint16_t query, uint32_t flags) {
    (void)self;
    (void)query;
    load(false);
    bool found = false;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < g_n; i++) {
        ShimServerEntry *e = &g_entries[i];
        if (e->ip != ip || e->conn_port != conn || e->appid != app || !(e->flags & flags)) continue;
        e->flags &= ~flags;
        if (!e->flags) g_entries[i] = g_entries[--g_n];
        found = true;
        break;
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (found) save();
    return found;
}

void register_serverlists(void) {
    const char *p = "SteamMatchMaking0";
    shim_register_family(p, "GetFavoriteGameCount", (void *)favorite_count);
    shim_register_family(p, "GetFavoriteGame", (void *)favorite_get);
    shim_register_family_ab(p, "GetFavoriteGame2", 28, (void *)favorite_get);
    shim_register_family(p, "AddFavoriteGame", (void *)favorite_add);
    shim_register_family(p, "RemoveFavoriteGame", (void *)favorite_remove);
}
