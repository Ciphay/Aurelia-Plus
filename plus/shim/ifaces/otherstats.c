#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <windows.h>

#include "../shim.h"

enum { SLOT_OTHER = 19, OP_GET = 1, OP_STORE = 2, MAX_USERS = 16, KIND_FLOAT = 2, KIND_AVG = 3 };

typedef struct { char name[64]; uint32_t id, kind, value; bool dirty; } OStat;
typedef struct { char name[64]; uint32_t stat_id, bit, time; bool dirty; } OAch;
typedef struct { uint64_t steam; bool used, gameserver; uint32_t crc; OStat *stats; size_t n; OAch *ach; size_t na; } OUser;

static OUser g_users[MAX_USERS];
static unsigned g_next_user;
static SRWLOCK g_lock = SRWLOCK_INIT;

static uint32_t app_id(void) { const Snapshot *s = shim_snapshot(); return s && s->has_ticket ? s->ticket.appid : 0; }
static void put32(uint8_t *d, int off, uint32_t v) { memcpy(d + off, &v, 4); }
static void put64(uint8_t *d, int off, uint64_t v) { memcpy(d + off, &v, 8); }

static void free_user(OUser *u) { free(u->stats); free(u->ach); memset(u, 0, sizeof *u); }

static OUser *find_user(uint64_t id) {
    for (int i = 0; i < MAX_USERS; i++) if (g_users[i].used && g_users[i].steam == id) return &g_users[i];
    return NULL;
}

static bool rd32(const uint8_t *p, size_t n, size_t *o, uint32_t *v) { if (*o + 4 > n) return false; memcpy(v, p + *o, 4); *o += 4; return true; }
static bool rdname(const uint8_t *p, size_t n, size_t *o, char *out, size_t cap) {
    if (*o + 2 > n) return false;
    uint16_t len;
    memcpy(&len, p + *o, 2);
    *o += 2;
    if (*o + len > n) return false;
    snprintf(out, cap, "%.*s", (int)len, (const char *)p + *o);
    *o += len;
    return true;
}

static bool parse_reply(const uint8_t *p, size_t n, OUser *u) {
    size_t o = 0;
    uint32_t count, na, tmp;
    if (!rd32(p, n, &o, &u->crc) || !rd32(p, n, &o, &count)) return false;
    u->stats = calloc(count ? count : 1, sizeof *u->stats);
    if (!u->stats) return false;
    for (uint32_t i = 0; i < count; i++) {
        OStat *s = &u->stats[u->n];
        if (!rd32(p, n, &o, &s->id) || !rd32(p, n, &o, &s->kind) || !rd32(p, n, &o, &s->value) || !rd32(p, n, &o, &tmp) || !rdname(p, n, &o, s->name, sizeof s->name)) return false;
        u->n++;
    }
    if (!rd32(p, n, &o, &na)) return false;
    u->ach = calloc(na ? na : 1, sizeof *u->ach);
    if (!u->ach) return false;
    for (uint32_t i = 0; i < na; i++) {
        OAch *a = &u->ach[u->na];
        if (!rd32(p, n, &o, &a->stat_id) || !rd32(p, n, &o, &a->bit) || !rd32(p, n, &o, &a->time) || !rdname(p, n, &o, a->name, sizeof a->name)) return false;
        u->na++;
    }
    return true;
}

static bool fetch_user(uint64_t id, bool gameserver) {
    uint8_t req[10] = { OP_GET, gameserver }, *reply = malloc(1 << 20);
    if (!reply) return false;
    memcpy(req + 2, &id, 8);
    size_t n = 0;
    int32_t status = -1;
    OUser fresh = {0};
    bool ok = bridge_call_fresh(0xFFFF, SLOT_OTHER, req, sizeof req, &status, reply, 1 << 20, &n, 30000) && status == 0 && parse_reply(reply, n, &fresh);
    free(reply);
    if (!ok) { free(fresh.stats); free(fresh.ach); return false; }
    fresh.steam = id; fresh.used = true; fresh.gameserver = gameserver;
    AcquireSRWLockExclusive(&g_lock);
    OUser *slot = find_user(id);
    if (!slot) slot = &g_users[g_next_user++ % MAX_USERS];
    free_user(slot);
    *slot = fresh;
    ReleaseSRWLockExclusive(&g_lock);
    return true;
}

typedef struct { uint64_t call, id; bool gameserver; } Job;
static DWORD WINAPI request_thread(LPVOID arg) {
    Job *j = arg;
    bool ok = fetch_user(j->id, j->gameserver);
    if (j->gameserver) {
        uint8_t cb[16] = {0};
        put32(cb, 0, ok ? 1 : 2); put64(cb, 8, j->id);
        shim_callresult_complete(j->call, 1800, cb, sizeof cb);
    } else {
        uint8_t cb[24] = {0};
        put64(cb, 0, app_id()); put32(cb, 8, ok ? 1 : 2); put64(cb, 16, j->id);
        shim_callresult_complete(j->call, 1101, cb, sizeof cb);
    }
    free(j);
    return 0;
}
static uint64_t request(uint64_t id, bool gameserver) {
    uint64_t call = shim_callresult_reserve();
    Job *j = calloc(1, sizeof *j);
    if (!j) { shim_callresult_fail_handle(call); return call; }
    j->call = call; j->id = id; j->gameserver = gameserver;
    HANDLE h = CreateThread(NULL, 0, request_thread, j, 0, NULL);
    if (!h) request_thread(j); else CloseHandle(h);
    return call;
}

static OStat *find_stat(OUser *u, const char *name) { for (size_t i = 0; u && name && i < u->n; i++) if (!strcmp(u->stats[i].name, name)) return &u->stats[i]; return NULL; }
static OAch *find_ach(OUser *u, const char *name) { for (size_t i = 0; u && name && i < u->na; i++) if (!strcmp(u->ach[i].name, name)) return &u->ach[i]; return NULL; }

bool shim_other_stat(uint64_t id, const char *name, void *out) {
    AcquireSRWLockShared(&g_lock);
    OStat *s = find_stat(find_user(id), name);
    bool ok = s && out;
    if (ok) memcpy(out, &s->value, 4);
    ReleaseSRWLockShared(&g_lock);
    return ok;
}
bool shim_other_achievement(uint64_t id, const char *name, uint8_t *achieved, uint32_t *when) {
    AcquireSRWLockShared(&g_lock);
    OAch *a = find_ach(find_user(id), name);
    bool ok = a != NULL;
    if (ok) { if (achieved) *achieved = a->time != 0; if (when) *when = a->time; }
    ReleaseSRWLockShared(&g_lock);
    return ok;
}
uint64_t shim_other_request(uint64_t id) { return request(id, false); }

static uint64_t SHIM_TC gss_request(void *self, uint64_t id) { (void)self; return request(id, true); }
static uint64_t SHIM_TC gss_get_stat(void *self, uint64_t id, const char *name, void *out) { (void)self; return shim_other_stat(id, name, out); }
static uint64_t SHIM_TC gss_get_achievement(void *self, uint64_t id, const char *name, uint8_t *achieved) { (void)self; return shim_other_achievement(id, name, achieved, NULL); }

static uint64_t set_stat(uint64_t id, const char *name, uint32_t bits) {
    AcquireSRWLockExclusive(&g_lock);
    OStat *s = find_stat(find_user(id), name);
    if (s) { s->value = bits; s->dirty = true; }
    ReleaseSRWLockExclusive(&g_lock);
    return s != NULL;
}
static uint64_t SHIM_TC gss_set_stat_int(void *self, uint64_t id, const char *name, int32_t value) {
    (void)self;
    AcquireSRWLockShared(&g_lock);
    OStat *s = find_stat(find_user(id), name);
    bool is_float = s && (s->kind == KIND_FLOAT || s->kind == KIND_AVG);
    ReleaseSRWLockShared(&g_lock);
    if (is_float) { float f = (float)value; uint32_t bits; memcpy(&bits, &f, 4); return set_stat(id, name, bits); }
    return set_stat(id, name, (uint32_t)value);
}
static uint64_t SHIM_TC gss_set_stat_float(void *self, uint64_t id, const char *name, float value) {
    (void)self;
    AcquireSRWLockShared(&g_lock);
    OStat *s = find_stat(find_user(id), name);
    bool is_float = !s || s->kind == KIND_FLOAT || s->kind == KIND_AVG;
    ReleaseSRWLockShared(&g_lock);
    uint32_t bits;
    if (is_float) memcpy(&bits, &value, 4); else bits = (uint32_t)(int32_t)value;
    return set_stat(id, name, bits);
}
static uint64_t SHIM_TC gss_update_avg(void *self, uint64_t id, const char *name, float count, double seconds) {
    (void)self;
    if (seconds <= 0) return 0;
    AcquireSRWLockShared(&g_lock);
    OStat *s = find_stat(find_user(id), name);
    float old = 0;
    if (s) memcpy(&old, &s->value, 4);
    bool ok = s && s->kind == KIND_AVG;
    ReleaseSRWLockShared(&g_lock);
    if (!ok) return 0;
    float avg = (float)(((double)old + (double)count) / (1.0 + seconds));
    uint32_t bits;
    memcpy(&bits, &avg, 4);
    return set_stat(id, name, bits);
}
static uint64_t set_achievement(uint64_t id, const char *name, bool on) {
    AcquireSRWLockExclusive(&g_lock);
    OAch *a = find_ach(find_user(id), name);
    if (a) { a->time = on ? (a->time ? a->time : (uint32_t)time(NULL)) : 0; a->dirty = true; }
    ReleaseSRWLockExclusive(&g_lock);
    return a != NULL;
}
static uint64_t SHIM_TC gss_set_achievement(void *self, uint64_t id, const char *name) { (void)self; return set_achievement(id, name, true); }
static uint64_t SHIM_TC gss_clear_achievement(void *self, uint64_t id, const char *name) { (void)self; return set_achievement(id, name, false); }

typedef struct { uint64_t call, id; } StoreJob;
static DWORD WINAPI store_thread(LPVOID arg) {
    StoreJob *j = arg;
    uint32_t changes[2 * 4096];
    uint32_t n = 0, crc = 0;
    AcquireSRWLockExclusive(&g_lock);
    OUser *u = find_user(j->id);
    if (u) {
        crc = u->crc;
        for (size_t i = 0; i < u->n && n < 4096; i++) if (u->stats[i].dirty) { changes[2 * n] = u->stats[i].id; changes[2 * n + 1] = u->stats[i].value; n++; }
        for (size_t i = 0; i < u->na && n < 4096; i++) {
            if (!u->ach[i].dirty) continue;
            uint32_t field = u->ach[i].stat_id, bits = 0;
            for (size_t k = 0; k < u->na; k++) if (u->ach[k].stat_id == field && u->ach[k].time && u->ach[k].bit < 32) bits |= 1u << u->ach[k].bit;
            bool seen = false;
            for (uint32_t c = 0; c < n; c++) if (changes[2 * c] == field) { changes[2 * c + 1] = bits; seen = true; }
            if (!seen) { changes[2 * n] = field; changes[2 * n + 1] = bits; n++; }
        }
    }
    ReleaseSRWLockExclusive(&g_lock);
    int32_t eresult = 2;
    if (u) {
        size_t len = 18 + (size_t)n * 8;
        uint8_t *req = malloc(len);
        if (req) {
            req[0] = OP_STORE; req[1] = 1;
            memcpy(req + 2, &j->id, 8); memcpy(req + 10, &crc, 4); memcpy(req + 14, &n, 4);
            memcpy(req + 18, changes, (size_t)n * 8);
            uint8_t reply[4];
            size_t got = 0;
            int32_t status = -1;
            if (bridge_call_fresh(0xFFFF, SLOT_OTHER, req, len, &status, reply, sizeof reply, &got, 30000) && status == 0 && got >= 4) memcpy(&eresult, reply, 4);
            free(req);
        }
        if (eresult == 1) {
            AcquireSRWLockExclusive(&g_lock);
            u = find_user(j->id);
            for (size_t i = 0; u && i < u->n; i++) u->stats[i].dirty = false;
            for (size_t i = 0; u && i < u->na; i++) u->ach[i].dirty = false;
            ReleaseSRWLockExclusive(&g_lock);
        }
    }
    uint8_t cb[16] = {0};
    put32(cb, 0, (uint32_t)eresult); put64(cb, 8, j->id);
    shim_callresult_complete(j->call, 1801, cb, sizeof cb);
    free(j);
    return 0;
}
static uint64_t SHIM_TC gss_store(void *self, uint64_t id) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    StoreJob *j = calloc(1, sizeof *j);
    if (!j) { shim_callresult_fail_handle(call); return call; }
    j->call = call; j->id = id;
    HANDLE h = CreateThread(NULL, 0, store_thread, j, 0, NULL);
    if (!h) store_thread(j); else CloseHandle(h);
    return call;
}

void register_otherstats(void) {
    const char *g = "SteamGameServerStats";
    shim_register_family(g, "RequestUserStats", (void *)gss_request);
    shim_register_family(g, "GetUserStat", (void *)gss_get_stat);
    shim_register_family(g, "GetUserStat_2", (void *)gss_get_stat);
    shim_register_family(g, "GetUserAchievement", (void *)gss_get_achievement);
    shim_register_family(g, "SetUserStat", (void *)gss_set_stat_int);
    shim_register_family(g, "SetUserStat_2", (void *)gss_set_stat_float);
    shim_register_family(g, "UpdateUserAvgRateStat", (void *)gss_update_avg);
    shim_register_family(g, "SetUserAchievement", (void *)gss_set_achievement);
    shim_register_family(g, "ClearUserAchievement", (void *)gss_clear_achievement);
    shim_register_family(g, "StoreUserStats", (void *)gss_store);
}
