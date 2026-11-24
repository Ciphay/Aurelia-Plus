#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>
#include <bcrypt.h>

#include "../shim.h"

enum { WAIT_MS = 20000 };
typedef struct { const char **strings; int32_t n; } ParamStringArray;

static uint32_t app_id(void) { const Snapshot *s = shim_snapshot(); return s && s->has_ticket ? s->ticket.appid : 0; }
static void put32(uint8_t *d, int off, uint32_t v) { memcpy(d + off, &v, 4); }
static void put64(uint8_t *d, int off, uint64_t v) { memcpy(d + off, &v, 8); }

static uint64_t failed_call(void) { uint64_t call = shim_callresult_reserve(); shim_callresult_fail_handle(call); return call; }

static uint64_t done(int callback, const void *data, int size) {
    uint64_t call = shim_callresult_reserve();
    shim_callresult_complete(call, callback, data, size);
    return call;
}

typedef struct { uint64_t call; char path[MAX_PATH]; } FileJob;

static bool sha1_of_file(const char *path, uint64_t *size, uint8_t sha[20]) {
    HANDLE f = CreateFileA(path, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, NULL);
    if (f == INVALID_HANDLE_VALUE) return false;
    BCRYPT_ALG_HANDLE alg = NULL;
    BCRYPT_HASH_HANDLE h = NULL;
    bool ok = BCryptOpenAlgorithmProvider(&alg, BCRYPT_SHA1_ALGORITHM, NULL, 0) >= 0 && BCryptCreateHash(alg, &h, NULL, 0, NULL, 0, 0) >= 0;
    static __thread uint8_t buf[1 << 16];
    uint64_t total = 0;
    DWORD n;
    while (ok && ReadFile(f, buf, sizeof buf, &n, NULL) && n) {
        ok = BCryptHashData(h, buf, n, 0) >= 0;
        total += n;
    }
    ok = ok && BCryptFinishHash(h, sha, 20, 0) >= 0;
    if (h) BCryptDestroyHash(h);
    if (alg) BCryptCloseAlgorithmProvider(alg, 0);
    CloseHandle(f);
    *size = total;
    return ok;
}

static DWORD WINAPI file_details_thread(LPVOID arg) {
    FileJob *j = arg;
    uint8_t cb[40] = {0};
    uint64_t size = 0;
    uint8_t sha[20] = {0};
    bool ok = sha1_of_file(j->path, &size, sha);
    put32(cb, 0, ok ? 1 : 9);
    if (ok) { put64(cb, 8, size); memcpy(cb + 16, sha, 20); }
    shim_callresult_complete(j->call, 1023, cb, sizeof cb);
    free(j);
    return 0;
}

static uint64_t SHIM_TC get_file_details(void *self, const char *name) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    FileJob *j = calloc(1, sizeof *j);
    const Snapshot *s = shim_snapshot();
    if (!j || !name || !*name) { free(j); uint8_t cb[40] = {0}; put32(cb, 0, 8); shim_callresult_complete(call, 1023, cb, sizeof cb); return call; }
    j->call = call;
    bool absolute = (name[0] && name[1] == ':') || name[0] == '\\' || name[0] == '/';
    char dir[MAX_PATH] = "";
    if (!absolute && s && s->has_app && s->app.install_dir) shim_win_path(s->app.install_dir, dir, sizeof dir);
    snprintf(j->path, sizeof j->path, "%s%s%s", absolute ? "" : dir, absolute || !dir[0] ? "" : "\\", name);
    HANDLE t = CreateThread(NULL, 0, file_details_thread, j, 0, NULL);
    if (!t) { file_details_thread(j); return call; }
    CloseHandle(t);
    return call;
}

enum { CB_UGC_FAVORITES = 3407, CB_UGC_SET_VOTE = 3408, CB_UGC_GET_VOTE = 3409, CB_UGC_ADD_DEP = 3412, CB_UGC_REMOVE_DEP = 3413,
       CB_UGC_ADD_APP_DEP = 3414, CB_UGC_REMOVE_APP_DEP = 3415, CB_UGC_GET_APP_DEPS = 3416, CB_UGC_EULA = 3419 };
typedef struct { uint64_t id, other; uint32_t app; uint32_t flag; } Ctx;
static Ctx *ctx_of(uint64_t id, uint64_t other, uint32_t app, uint32_t flag) { Ctx *c = calloc(1, sizeof *c); if (c) { c->id = id; c->other = other; c->app = app; c->flag = flag; } return c; }

static bool parse_triple_u64(const CmReply *r, void *ctx, void *out, int *size) {
    Ctx *c = ctx;
    if (*size < 24) return false;
    memset(out, 0, 24);
    put32(out, 0, (uint32_t)r->eresult);
    put64(out, 8, c->id);
    put64(out, 16, c->other);
    *size = 24;
    return true;
}
static bool parse_triple_app(const CmReply *r, void *ctx, void *out, int *size) {
    Ctx *c = ctx;
    if (*size < 24) return false;
    memset(out, 0, 24);
    put32(out, 0, (uint32_t)r->eresult);
    put64(out, 8, c->id);
    put32(out, 16, c->app);
    *size = 24;
    return true;
}

static uint64_t child_call(const char *job, int callback, uint64_t parent, uint64_t child) {
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_varint(&w, 1, parent);
    pbw_varint(&w, 2, child);
    shim_cm_async(call, callback, 0, true, job, &w, WAIT_MS, parse_triple_u64, ctx_of(parent, child, 0, 0));
    return call;
}
static uint64_t SHIM_TC ugc_add_dependency(void *self, uint64_t parent, uint64_t child) { (void)self; return child_call("PublishedFile.AddChild#1", CB_UGC_ADD_DEP, parent, child); }
static uint64_t SHIM_TC ugc_remove_dependency(void *self, uint64_t parent, uint64_t child) { (void)self; return child_call("PublishedFile.RemoveChild#1", CB_UGC_REMOVE_DEP, parent, child); }

enum { APP_RELATION_DEPENDENCY = 1 };
static uint64_t app_relation_call(const char *job, int callback, uint64_t id, uint32_t app) {
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_varint(&w, 1, id);
    pbw_varint(&w, 2, app);
    pbw_varint(&w, 3, APP_RELATION_DEPENDENCY);
    shim_cm_async(call, callback, 0, true, job, &w, WAIT_MS, parse_triple_app, ctx_of(id, 0, app, 0));
    return call;
}
static uint64_t SHIM_TC ugc_add_app_dependency(void *self, uint64_t id, uint32_t app) { (void)self; return app_relation_call("PublishedFile.AddAppRelationship#1", CB_UGC_ADD_APP_DEP, id, app); }
static uint64_t SHIM_TC ugc_remove_app_dependency(void *self, uint64_t id, uint32_t app) { (void)self; return app_relation_call("PublishedFile.RemoveAppRelationship#1", CB_UGC_REMOVE_APP_DEP, id, app); }

static bool parse_app_deps(const CmReply *r, void *ctx, void *out, int *size) {
    Ctx *c = ctx;
    if (*size < 152) return false;
    uint8_t *o = out;
    memset(o, 0, 152);
    put32(o, 0, (uint32_t)r->eresult);
    put64(o, 8, c->id);
    uint32_t n = 0, total = 0;
    PbR in;
    PbField f;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field != 3 || f.wt != 2) continue;
        PbR p;
        PbField g;
        uint32_t app = 0, rel = 0;
        pbr_init(&p, f.data, f.len);
        while (pbr_next(&p, &g)) { if (g.field == 1) app = (uint32_t)g.v; else if (g.field == 2) rel = (uint32_t)g.v; }
        if (rel != APP_RELATION_DEPENDENCY) continue;
        total++;
        if (n < 32) put32(o, 16 + 4 * (int)n++, app);
    }
    put32(o, 144, n);
    put32(o, 148, total);
    *size = 152;
    return true;
}
static uint64_t SHIM_TC ugc_get_app_dependencies(void *self, uint64_t id) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_varint(&w, 1, id);
    shim_cm_async(call, CB_UGC_GET_APP_DEPS, 0, true, "PublishedFile.GetAppRelationships#1", &w, WAIT_MS, parse_app_deps, ctx_of(id, 0, 0, 0));
    return call;
}

static bool parse_set_vote(const CmReply *r, void *ctx, void *out, int *size) {
    Ctx *c = ctx;
    if (*size < 16) return false;
    memset(out, 0, 16);
    put64(out, 0, c->id);
    put32(out, 8, (uint32_t)r->eresult);
    ((uint8_t *)out)[12] = (uint8_t)c->flag;
    *size = 16;
    return true;
}
static uint64_t SHIM_TC ugc_set_user_item_vote(void *self, uint64_t id, uint32_t up) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_varint(&w, 1, id);
    pbw_varint(&w, 2, (up & 0xff) != 0);
    shim_cm_async(call, CB_UGC_SET_VOTE, 0, true, "PublishedFile.Vote#1", &w, WAIT_MS, parse_set_vote, ctx_of(id, 0, 0, (up & 0xff) != 0));
    return call;
}

static bool parse_get_vote(const CmReply *r, void *ctx, void *out, int *size) {
    Ctx *c = ctx;
    if (*size < 16) return false;
    uint8_t *o = out;
    memset(o, 0, 16);
    put64(o, 0, c->id);
    put32(o, 8, (uint32_t)r->eresult);
    PbR in;
    PbField f;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field != 1 || f.wt != 2) continue;
        PbR p;
        PbField g;
        pbr_init(&p, f.data, f.len);
        while (pbr_next(&p, &g)) { if (g.field == 2) o[12] = g.v != 0; else if (g.field == 3) o[13] = g.v != 0; }
    }
    *size = 16;
    return true;
}
static uint64_t SHIM_TC ugc_get_user_item_vote(void *self, uint64_t id) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_fixed64(&w, 1, id);
    shim_cm_async(call, CB_UGC_GET_VOTE, 0, true, "PublishedFile.GetUserVoteSummary#1", &w, WAIT_MS, parse_get_vote, ctx_of(id, 0, 0, 0));
    return call;
}

static uint64_t SHIM_TC ugc_eula_status(void *self) {
    (void)self;
    uint8_t cb[24] = {0};
    put32(cb, 0, 1);
    put32(cb, 4, app_id());
    cb[16] = 1;
    return done(CB_UGC_EULA, cb, sizeof cb);
}

uint64_t shim_other_request(uint64_t id);
static uint64_t SHIM_TC us_request_user_stats(void *self, uint64_t id) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    if (s && id == s->steamid) {
        uint8_t cb[24] = {0};
        put64(cb, 0, app_id());
        put32(cb, 8, 1);
        put64(cb, 16, id);
        return done(1101, cb, sizeof cb);
    }
    return shim_other_request(id);
}
static uint64_t SHIM_TC us_update_avg_rate_old(void *self, const char *name, uint32_t a, uint32_t b, uint32_t c, uint32_t d) {
    (void)self; (void)name; (void)a; (void)b; (void)c; (void)d;
    return 0;
}

static uint64_t SHIM_TC fr_download_clan_activity(void *self, const uint64_t *clans, int n) { (void)self; (void)clans; (void)n; uint8_t cb[8] = {0}; return done(341, cb, 1); }
static uint64_t SHIM_TC fr_equipped_items(void *self, uint64_t id) { (void)self; uint8_t cb[24] = {0}; put32(cb, 0, 2); put64(cb, 8, id); return done(351, cb, sizeof cb); }

static uint64_t SHIM_TC user_duration_control(void *self) {
    (void)self;
    uint8_t cb[32] = {0};
    put32(cb, 0, 1);
    put32(cb, 4, app_id());
    return done(167, cb, sizeof cb);
}

static uint64_t SHIM_TC gs_associate_clan(void *self, uint64_t clan) { (void)self; (void)clan; uint8_t cb[4] = {0}; put32(cb, 0, 2); return done(210, cb, sizeof cb); }
static uint64_t SHIM_TC gs_compute_compat(void *self, uint64_t candidate) { (void)self; uint8_t cb[24] = {0}; put32(cb, 0, 2); put64(cb, 16, candidate); return done(211, cb, sizeof cb); }
static uint64_t SHIM_TC gs_reputation(void *self) { (void)self; uint8_t cb[40] = {0}; put32(cb, 0, 2); return done(209, cb, sizeof cb); }
static uint64_t SHIM_TC game_stats_session(void *self, int8_t type, uint64_t account, int32_t app, uint32_t started) {
    (void)self; (void)type; (void)account; (void)app; (void)started;
    uint8_t cb[24] = {0};
    put32(cb, 0, 2);
    return done(1, cb, sizeof cb);
}

static uint64_t SHIM_TC unified_send(void *self, const char *method, const void *request, uint32_t cb, uint64_t context) {
    (void)self; (void)method; (void)request; (void)cb; (void)context;
    return 0;
}
static uint64_t SHIM_TC billing_receipt(void *self, uint32_t ignored) { (void)self; (void)ignored; return failed_call(); }
static uint64_t SHIM_TC serialized_cert(void *self) { (void)self; return failed_call(); }

void register_extras(void) {
    shim_register_family("STEAMAPPS_INTERFACE_VERSION", "GetFileDetails", (void *)get_file_details);
    const char *u = "STEAMUGC_INTERFACE_VERSION";
    shim_register_family(u, "AddDependency", (void *)ugc_add_dependency);
    shim_register_family(u, "RemoveDependency", (void *)ugc_remove_dependency);
    shim_register_family(u, "AddAppDependency", (void *)ugc_add_app_dependency);
    shim_register_family(u, "RemoveAppDependency", (void *)ugc_remove_app_dependency);
    shim_register_family(u, "GetAppDependencies", (void *)ugc_get_app_dependencies);
    shim_register_family(u, "SetUserItemVote", (void *)ugc_set_user_item_vote);
    shim_register_family(u, "GetUserItemVote", (void *)ugc_get_user_item_vote);
    shim_register_family(u, "GetWorkshopEULAStatus", (void *)ugc_eula_status);
    shim_register_family("STEAMUSERSTATS_INTERFACE_VERSION", "RequestUserStats", (void *)us_request_user_stats);
    shim_register_family_ab("STEAMUSERSTATS_INTERFACE_VERSION", "UpdateAvgRateStat", 24, (void *)us_update_avg_rate_old);
    shim_register_family("SteamFriends0", "DownloadClanActivityCounts", (void *)fr_download_clan_activity);
    shim_register_family("SteamFriends0", "RequestEquippedProfileItems", (void *)fr_equipped_items);
    shim_register_family("SteamUser0", "GetDurationControl", (void *)user_duration_control);
    shim_register_family("SteamGameServer0", "AssociateWithClan", (void *)gs_associate_clan);
    shim_register_family("SteamGameServer0", "ComputeNewPlayerCompatibility", (void *)gs_compute_compat);
    shim_register_family("SteamGameServer0", "GetServerReputation", (void *)gs_reputation);
    shim_register_family("SteamGameStats", "GetNewSession", (void *)game_stats_session);
    shim_register_family("STEAMUNIFIEDMESSAGES", "SendMethod", (void *)unified_send);
    shim_register_family("SteamBilling", "GetReceiptTransID", (void *)billing_receipt);
    shim_register_family("SteamNetworkingSocketsSerialized", "GetCertAsync", (void *)serialized_cert);
}
