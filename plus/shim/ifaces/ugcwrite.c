#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "../shim.h"

enum {
    WAIT_MS = 15000,
    CB_CREATE_ITEM = 3403, CB_SUBMIT_UPDATE = 3404, CB_START_PLAYTIME = 3410, CB_STOP_PLAYTIME = 3411, CB_DELETE_ITEM = 3417,
    MAX_PLAYTIME_ITEMS = 100,
    SLOT_UGC = 10, OP_CREATE = 6, OP_SUBMIT = 7, OP_PROGRESS = 8, OP_DELETE = 9,
    OP_SERVER_FOLDER = 10, MAX_PREVIEWS = 32, MAX_UPDATES = 16, SUBMIT_POLL_MS = 300, SUBMIT_MAX_MS = 30 * 60 * 1000,
};

static SRWLOCK g_lock = SRWLOCK_INIT;

static uint32_t app_id(void) { const Snapshot *s = shim_snapshot(); return s && s->has_ticket ? s->ticket.appid : 0; }

static bool parse_result(const CmReply *r, void *ctx, void *out, int *size) {
    (void)ctx;
    int32_t result = r->eresult;
    memcpy(out, &result, 4);
    *size = 4;
    return true;
}

enum { TRACKED_MAX = 256 };
static uint64_t g_tracked[TRACKED_MAX];
static int g_ntracked;

static void tracking_add(const uint64_t *ids, uint32_t n) {
    AcquireSRWLockExclusive(&g_lock);
    for (uint32_t i = 0; i < n; i++) {
        bool have = false;
        for (int k = 0; k < g_ntracked; k++) have = have || g_tracked[k] == ids[i];
        if (!have && g_ntracked < TRACKED_MAX) g_tracked[g_ntracked++] = ids[i];
    }
    ReleaseSRWLockExclusive(&g_lock);
}

static void tracking_remove(const uint64_t *ids, uint32_t n) {
    AcquireSRWLockExclusive(&g_lock);
    for (uint32_t i = 0; i < n; i++)
        for (int k = 0; k < g_ntracked; k++)
            if (g_tracked[k] == ids[i]) { g_tracked[k] = g_tracked[--g_ntracked]; break; }
    ReleaseSRWLockExclusive(&g_lock);
}

static uint64_t playtime_call(const uint64_t *ids, uint32_t n, const char *job, int callback) {
    if (!ids || n == 0 || n > MAX_PLAYTIME_ITEMS) return 0;
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    for (uint32_t i = 0; i < n; i++) pbw_varint(&w, 2, ids[i]);
    shim_cm_async(call, callback, CM_EMSG_SERVICE_CALL, true, job, &w, WAIT_MS, parse_result, NULL);
    return call;
}

static uint64_t SHIM_TC start_playtime_tracking(void *self, const uint64_t *ids, uint32_t n) {
    (void)self;
    uint64_t call = playtime_call(ids, n, "PublishedFile.StartPlaytimeTracking#1", CB_START_PLAYTIME);
    if (call) tracking_add(ids, n);
    return call;
}

static uint64_t SHIM_TC stop_playtime_tracking(void *self, const uint64_t *ids, uint32_t n) {
    (void)self;
    uint64_t call = playtime_call(ids, n, "PublishedFile.StopPlaytimeTracking#1", CB_STOP_PLAYTIME);
    if (call) tracking_remove(ids, n);
    return call;
}

static uint64_t SHIM_TC stop_playtime_tracking_all(void *self) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    int tracked = g_ntracked;
    g_ntracked = 0;
    ReleaseSRWLockExclusive(&g_lock);
    if (!tracked) return 0;
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    shim_cm_async(call, CB_STOP_PLAYTIME, CM_EMSG_SERVICE_CALL, true, "PublishedFile.StopPlaytimeTrackingForAllAppItems#1", &w, WAIT_MS, parse_result, NULL);
    return call;
}

typedef struct {
    bool used;
    uint64_t handle, item;
    uint32_t app;
    char *title, *description, *metadata, *language, *content, *preview;
    int visibility;
    bool has_visibility;
    char **tags;
    int ntags;
    bool has_tags;
    char **kv;
    int nkv;
    char **removed;
    int nremoved;
    struct { bool video; int type; char *value; int replaces; } previews[MAX_PREVIEWS];
    int npreviews;
    int remove_idx[MAX_PREVIEWS], nremove;
    int desc_add[MAX_PREVIEWS], nadd, desc_rm[MAX_PREVIEWS], nrm;
    char *game_min, *game_max;
    bool submitted;
    int status;
    uint64_t done, total;
} Update;

static Update g_updates[MAX_UPDATES];
static uint64_t g_next_handle = 0x7500;

typedef struct { char *p; size_t len, cap; } Buf;

static void put(Buf *b, const char *s, size_t n) {
    if (b->len + n + 1 > b->cap) {
        size_t cap = b->cap ? b->cap : 256;
        while (b->len + n + 1 > cap) cap *= 2;
        char *g = realloc(b->p, cap);
        if (!g) return;
        b->p = g;
        b->cap = cap;
    }
    memcpy(b->p + b->len, s, n);
    b->len += n;
    b->p[b->len] = 0;
}
static void put_str(Buf *b, const char *s) { put(b, s, strlen(s)); }
static void put_text(Buf *b, const char *s) {
    put_str(b, "\"");
    for (const unsigned char *c = (const unsigned char *)(s ? s : ""); *c; c++) {
        char tmp[8];
        if (*c == '"' || *c == '\\') { tmp[0] = '\\'; tmp[1] = (char)*c; put(b, tmp, 2); }
        else if (*c < 0x20) { snprintf(tmp, sizeof tmp, "\\u%04x", *c); put(b, tmp, 6); }
        else put(b, (const char *)c, 1);
    }
    put_str(b, "\"");
}

static void unix_path(const char *win, char *out, size_t cap) {
    out[0] = 0;
    if (!win || !*win) return;
    if (win[0] == '/') { snprintf(out, cap, "%s", win); return; }
    if ((win[0] == 'Z' || win[0] == 'z') && win[1] == ':') {
        snprintf(out, cap, "%s", win + 2);
        for (char *c = out; *c; c++) if (*c == '\\') *c = '/';
        return;
    }
    WCHAR wide[1024];
    if (MultiByteToWideChar(CP_UTF8, 0, win, -1, wide, 1024)) {
        typedef char *(CDECL *GetUnix)(const WCHAR *);
        HMODULE k = GetModuleHandleA("kernel32.dll");
        GetUnix fn = k ? (GetUnix)(void *)GetProcAddress(k, "wine_get_unix_file_name") : NULL;
        char *path = fn ? fn(wide) : NULL;
        if (path) { snprintf(out, cap, "%s", path); HeapFree(GetProcessHeap(), 0, path); return; }
    }
    snprintf(out, cap, "%s", win);
    for (char *c = out; *c; c++) if (*c == '\\') *c = '/';
}

static Update *find_update(uint64_t handle) {
    for (int i = 0; i < MAX_UPDATES; i++) if (g_updates[i].used && g_updates[i].handle == handle) return &g_updates[i];
    return NULL;
}

static void set_str(char **dst, const char *src) {
    free(*dst);
    *dst = src ? _strdup(src) : NULL;
}

static void strs_free(char **v, int n) {
    for (int i = 0; i < n; i++) free(v[i]);
    free(v);
}

static void update_free(Update *u) {
    free(u->title); free(u->description); free(u->metadata); free(u->language); free(u->content); free(u->preview);
    strs_free(u->tags, u->ntags);
    strs_free(u->kv, u->nkv);
    strs_free(u->removed, u->nremoved);
    for (int i = 0; i < u->npreviews; i++) free(u->previews[i].value);
    free(u->game_min); free(u->game_max);
    memset(u, 0, sizeof *u);
}

static uint64_t SHIM_TC start_item_update(void *self, uint32_t app, uint64_t item) {
    (void)self;
    uint64_t handle = ~0ULL;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < MAX_UPDATES; i++) {
        if (g_updates[i].used) continue;
        memset(&g_updates[i], 0, sizeof g_updates[i]);
        g_updates[i].used = true;
        g_updates[i].handle = handle = ++g_next_handle;
        g_updates[i].item = item;
        g_updates[i].app = app;
        break;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return handle;
}

#define WITH_UPDATE(handle, body) do { \
    bool ok_ = false; \
    AcquireSRWLockExclusive(&g_lock); \
    Update *u = find_update(handle); \
    if (u && !u->submitted) { body; ok_ = true; } \
    ReleaseSRWLockExclusive(&g_lock); \
    return ok_; } while (0)

#define WITH_UPDATE_OK(handle, body) do { \
    AcquireSRWLockExclusive(&g_lock); \
    Update *u = find_update(handle); \
    if (u && !u->submitted) { body; } \
    ReleaseSRWLockExclusive(&g_lock); } while (0)

static uint64_t SHIM_TC set_item_title(void *self, uint64_t h, const char *v) { (void)self; WITH_UPDATE(h, set_str(&u->title, v ? v : "")); }
static uint64_t SHIM_TC set_item_description(void *self, uint64_t h, const char *v) { (void)self; WITH_UPDATE(h, set_str(&u->description, v ? v : "")); }
static uint64_t SHIM_TC set_item_metadata(void *self, uint64_t h, const char *v) { (void)self; WITH_UPDATE(h, set_str(&u->metadata, v ? v : "")); }
static uint64_t SHIM_TC set_item_language(void *self, uint64_t h, const char *v) { (void)self; WITH_UPDATE(h, set_str(&u->language, v && *v ? v : "english")); }
static uint64_t SHIM_TC set_item_visibility(void *self, uint64_t h, int v) { (void)self; WITH_UPDATE(h, { u->visibility = v; u->has_visibility = true; }); }
static uint64_t SHIM_TC set_item_preview(void *self, uint64_t h, const char *v) { (void)self; WITH_UPDATE(h, set_str(&u->preview, v)); }
static uint64_t SHIM_TC set_allow_legacy_upload(void *self, uint64_t h, uint8_t v) { (void)self; (void)v; WITH_UPDATE(h, (void)u); }

static uint64_t SHIM_TC set_item_content(void *self, uint64_t h, const char *folder) {
    (void)self;
    char path[1024];
    unix_path(folder, path, sizeof path);
    if (!*path) return false;
    WITH_UPDATE(h, set_str(&u->content, path));
}

typedef struct { const char **strings; int32_t n; } ParamStringArray;

static uint64_t SHIM_TC set_item_tags(void *self, uint64_t h, const ParamStringArray *tags) {
    (void)self;
    WITH_UPDATE(h, {
        strs_free(u->tags, u->ntags);
        u->tags = NULL;
        u->ntags = 0;
        u->has_tags = true;
        int n = tags && tags->strings && tags->n > 0 ? tags->n : 0;
        u->tags = n ? calloc((size_t)n, sizeof *u->tags) : NULL;
        for (int i = 0; u->tags && i < n; i++) if (tags->strings[i]) u->tags[u->ntags++] = _strdup(tags->strings[i]);
    });
}
static uint64_t SHIM_TC set_item_tags_admin(void *self, uint64_t h, const ParamStringArray *tags, uint8_t allow_admin) { (void)allow_admin; return set_item_tags(self, h, tags); }

static char **append2(char **v, int *n, const char *a, const char *b) {
    char **g = realloc(v, (size_t)(*n + (b ? 2 : 1)) * sizeof *g);
    if (!g) return v;
    g[(*n)++] = _strdup(a ? a : "");
    if (b) g[(*n)++] = _strdup(b);
    return g;
}

static uint64_t SHIM_TC add_item_kv_tag(void *self, uint64_t h, const char *key, const char *value) {
    (void)self;
    if (!key || !*key) return false;
    WITH_UPDATE(h, u->kv = append2(u->kv, &u->nkv, key, value ? value : ""));
}
static uint64_t SHIM_TC remove_item_kv_tags(void *self, uint64_t h, const char *key) {
    (void)self;
    if (!key) return false;
    WITH_UPDATE(h, u->removed = append2(u->removed, &u->nremoved, key, NULL));
}
static uint64_t SHIM_TC remove_all_item_kv_tags(void *self, uint64_t h) { (void)self; WITH_UPDATE(h, u->removed = append2(u->removed, &u->nremoved, "", NULL)); }

static bool add_preview(Update *u, bool video, int type, const char *value, int replaces) {
    if (!value || !*value || u->npreviews >= MAX_PREVIEWS) return false;
    u->previews[u->npreviews].video = video;
    u->previews[u->npreviews].type = type;
    u->previews[u->npreviews].value = _strdup(value);
    u->previews[u->npreviews].replaces = replaces;
    u->npreviews++;
    return true;
}
static uint64_t SHIM_TC add_item_preview_file(void *self, uint64_t h, const char *file, int type) {
    (void)self;
    char path[1024];
    unix_path(file, path, sizeof path);
    bool ok = false;
    WITH_UPDATE_OK(h, ok = add_preview(u, false, type, path, -1));
    return ok;
}
static uint64_t SHIM_TC add_item_preview_video(void *self, uint64_t h, const char *id) { (void)self; bool ok = false; WITH_UPDATE_OK(h, ok = add_preview(u, true, 1, id, -1)); return ok; }
static uint64_t SHIM_TC update_item_preview_file(void *self, uint64_t h, uint32_t index, const char *file) {
    (void)self;
    char path[1024];
    unix_path(file, path, sizeof path);
    bool ok = false;
    WITH_UPDATE_OK(h, ok = add_preview(u, false, 0, path, (int)index));
    return ok;
}
static uint64_t SHIM_TC update_item_preview_video(void *self, uint64_t h, uint32_t index, const char *id) { (void)self; bool ok = false; WITH_UPDATE_OK(h, ok = add_preview(u, true, 1, id, (int)index)); return ok; }
static uint64_t SHIM_TC remove_item_preview(void *self, uint64_t h, uint32_t index) {
    (void)self;
    bool ok = false;
    WITH_UPDATE_OK(h, if (u->nremove < MAX_PREVIEWS) { u->remove_idx[u->nremove++] = (int)index; ok = true; });
    return ok;
}
static uint64_t SHIM_TC add_content_descriptor(void *self, uint64_t h, int descriptor) {
    (void)self;
    bool ok = false;
    WITH_UPDATE_OK(h, if (u->nadd < MAX_PREVIEWS) { u->desc_add[u->nadd++] = descriptor; ok = true; });
    return ok;
}
static uint64_t SHIM_TC remove_content_descriptor(void *self, uint64_t h, int descriptor) {
    (void)self;
    bool ok = false;
    WITH_UPDATE_OK(h, if (u->nrm < MAX_PREVIEWS) { u->desc_rm[u->nrm++] = descriptor; ok = true; });
    return ok;
}
static uint64_t SHIM_TC set_required_game_versions(void *self, uint64_t h, const char *min, const char *max) {
    (void)self;
    bool ok = false;
    WITH_UPDATE_OK(h, { set_str(&u->game_min, min); set_str(&u->game_max, max); ok = true; });
    return ok;
}

static bool ugc_op(uint8_t op, uint32_t app, uint64_t id, const char *body, int timeout_ms, uint8_t *reply, size_t cap, size_t *n) {
    size_t bl = body ? strlen(body) : 0;
    uint8_t *req = malloc(13 + bl);
    if (!req) return false;
    req[0] = op;
    memcpy(req + 1, &app, 4);
    memcpy(req + 5, &id, 8);
    if (bl) memcpy(req + 13, body, bl);
    int32_t status = -1;
    bool ok = bridge_call_fresh(0xFFFF, SLOT_UGC, req, 13 + bl, &status, reply, cap, n, timeout_ms) && status == 0;
    free(req);
    return ok;
}

typedef struct { uint64_t call; uint32_t app; uint64_t id; } ItemJob;

static DWORD WINAPI create_thread(LPVOID arg) {
    ItemJob *j = arg;
    uint8_t reply[32], res[16] = {0};
    size_t n = 0;
    int32_t result = 2;
    uint64_t id = 0;
    uint8_t legal = 0;
    if (ugc_op(OP_CREATE, j->app, 0, NULL, 30000, reply, sizeof reply, &n) && n >= 13) {
        memcpy(&result, reply, 4);
        memcpy(&id, reply + 4, 8);
        legal = reply[12];
    }
    memcpy(res, &id, 8);
    memcpy(res + 8, &result, 4);
    res[12] = legal;
    shim_callresult_complete(j->call, CB_CREATE_ITEM, res, sizeof res);
    shim_log("CreateItem app=%u result=%d item=%llu", j->app, result, (unsigned long long)id);
    free(j);
    return 0;
}

static uint64_t SHIM_TC create_item(void *self, uint32_t app, int file_type) {
    (void)self; (void)file_type;
    ItemJob *j = calloc(1, sizeof *j);
    if (!j) return 0;
    j->call = shim_callresult_reserve();
    j->app = app ? app : app_id();
    uint64_t call = j->call;
    HANDLE t = CreateThread(NULL, 0, create_thread, j, 0, NULL);
    if (!t) { shim_callresult_fail_handle(call); free(j); return call; }
    CloseHandle(t);
    return call;
}

static DWORD WINAPI delete_thread(LPVOID arg) {
    ItemJob *j = arg;
    uint8_t reply[8], res[16] = {0};
    size_t n = 0;
    int32_t result = 2;
    if (ugc_op(OP_DELETE, j->app, j->id, NULL, 30000, reply, sizeof reply, &n) && n >= 4) memcpy(&result, reply, 4);
    memcpy(res, &result, 4);
    memcpy(res + 8, &j->id, 8);
    shim_callresult_complete(j->call, CB_DELETE_ITEM, res, sizeof res);
    free(j);
    return 0;
}

static uint64_t SHIM_TC delete_item(void *self, uint64_t item) {
    (void)self;
    ItemJob *j = calloc(1, sizeof *j);
    if (!j) return 0;
    j->call = shim_callresult_reserve();
    j->app = app_id();
    j->id = item;
    uint64_t call = j->call;
    HANDLE t = CreateThread(NULL, 0, delete_thread, j, 0, NULL);
    if (!t) { shim_callresult_fail_handle(call); free(j); return call; }
    CloseHandle(t);
    return call;
}

typedef struct { uint64_t call, handle; uint32_t app; char *json; } SubmitJob;

static char *spec_json(const Update *u, const char *note) {
    Buf b = {0};
    char tmp[64];
    put_str(&b, "{\"item\":");
    snprintf(tmp, sizeof tmp, "%llu", (unsigned long long)u->item);
    put_str(&b, tmp);
    if (u->title) { put_str(&b, ",\"title\":"); put_text(&b, u->title); }
    if (u->description) { put_str(&b, ",\"description\":"); put_text(&b, u->description); }
    if (u->metadata) { put_str(&b, ",\"metadata\":"); put_text(&b, u->metadata); }
    if (u->language) { put_str(&b, ",\"language\":"); put_text(&b, u->language); }
    if (u->content) { put_str(&b, ",\"content\":"); put_text(&b, u->content); }
    if (u->preview) { char path[1024]; unix_path(u->preview, path, sizeof path); put_str(&b, ",\"preview\":"); put_text(&b, path); }
    if (note && *note) { put_str(&b, ",\"change_note\":"); put_text(&b, note); }
    if (u->has_visibility) { snprintf(tmp, sizeof tmp, ",\"visibility\":%d", u->visibility); put_str(&b, tmp); }
    if (u->has_tags) {
        put_str(&b, ",\"tags\":[");
        for (int i = 0; i < u->ntags; i++) { if (i) put_str(&b, ","); put_text(&b, u->tags[i]); }
        put_str(&b, "]");
    }
    if (u->nkv) {
        put_str(&b, ",\"kv_tags\":[");
        for (int i = 0; i + 1 < u->nkv; i += 2) { if (i) put_str(&b, ","); put_str(&b, "["); put_text(&b, u->kv[i]); put_str(&b, ","); put_text(&b, u->kv[i + 1]); put_str(&b, "]"); }
        put_str(&b, "]");
    }
    if (u->nremoved) {
        put_str(&b, ",\"removed_keys\":[");
        for (int i = 0; i < u->nremoved; i++) { if (i) put_str(&b, ","); put_text(&b, u->removed[i]); }
        put_str(&b, "]");
    }
    if (u->npreviews) {
        put_str(&b, ",\"previews\":[");
        for (int i = 0; i < u->npreviews; i++) {
            if (i) put_str(&b, ",");
            put_str(&b, u->previews[i].video ? "{\"video\":" : "{\"file\":");
            put_text(&b, u->previews[i].value);
            snprintf(tmp, sizeof tmp, ",\"type\":%d", u->previews[i].type);
            put_str(&b, tmp);
            if (u->previews[i].replaces >= 0) { snprintf(tmp, sizeof tmp, ",\"replaces\":%d", u->previews[i].replaces); put_str(&b, tmp); }
            put_str(&b, "}");
        }
        put_str(&b, "]");
    }
    if (u->nremove) { put_str(&b, ",\"previews_to_remove\":["); for (int i = 0; i < u->nremove; i++) { snprintf(tmp, sizeof tmp, "%s%d", i ? "," : "", u->remove_idx[i]); put_str(&b, tmp); } put_str(&b, "]"); }
    if (u->nadd) { put_str(&b, ",\"descriptors_add\":["); for (int i = 0; i < u->nadd; i++) { snprintf(tmp, sizeof tmp, "%s%d", i ? "," : "", u->desc_add[i]); put_str(&b, tmp); } put_str(&b, "]"); }
    if (u->nrm) { put_str(&b, ",\"descriptors_remove\":["); for (int i = 0; i < u->nrm; i++) { snprintf(tmp, sizeof tmp, "%s%d", i ? "," : "", u->desc_rm[i]); put_str(&b, tmp); } put_str(&b, "]"); }
    if (u->game_min && *u->game_min) { put_str(&b, ",\"game_branch_min\":"); put_text(&b, u->game_min); }
    if (u->game_max && *u->game_max) { put_str(&b, ",\"game_branch_max\":"); put_text(&b, u->game_max); }
    put_str(&b, "}");
    return b.p;
}

static DWORD WINAPI submit_thread(LPVOID arg) {
    SubmitJob *j = arg;
    uint8_t reply[64], res[16] = {0};
    size_t n = 0;
    int32_t result = 2;
    uint8_t legal = 0;
    uint64_t item = 0;
    if (ugc_op(OP_SUBMIT, j->app, j->handle, j->json, 30000, reply, sizeof reply, &n)) {
        DWORD start = GetTickCount();
        for (;;) {
            Sleep(SUBMIT_POLL_MS);
            n = 0;
            if (ugc_op(OP_PROGRESS, j->app, j->handle, NULL, 15000, reply, sizeof reply, &n) && n >= 30) {
                uint64_t done, total;
                int32_t r;
                memcpy(&done, reply + 1, 8);
                memcpy(&total, reply + 9, 8);
                memcpy(&r, reply + 17, 4);
                AcquireSRWLockExclusive(&g_lock);
                Update *u = find_update(j->handle);
                if (u) { u->status = reply[0]; u->done = done; u->total = total; }
                ReleaseSRWLockExclusive(&g_lock);
                if (r != 0) { result = r; memcpy(&item, reply + 21, 8); legal = reply[29]; break; }
            }
            if (GetTickCount() - start > SUBMIT_MAX_MS) { result = 16; break; }
        }
    }
    memcpy(res, &result, 4);
    res[4] = legal;
    memcpy(res + 8, &item, 8);
    shim_callresult_complete(j->call, CB_SUBMIT_UPDATE, res, sizeof res);
    shim_log("SubmitItemUpdate item=%llu result=%d", (unsigned long long)item, result);
    AcquireSRWLockExclusive(&g_lock);
    Update *u = find_update(j->handle);
    if (u) update_free(u);
    ReleaseSRWLockExclusive(&g_lock);
    free(j->json);
    free(j);
    return 0;
}

static uint64_t SHIM_TC submit_item_update(void *self, uint64_t handle, const char *note) {
    (void)self;
    SubmitJob *j = calloc(1, sizeof *j);
    if (!j) return 0;
    AcquireSRWLockExclusive(&g_lock);
    Update *u = find_update(handle);
    if (!u || u->submitted) { ReleaseSRWLockExclusive(&g_lock); free(j); return 0; }
    u->submitted = true;
    u->status = 1;
    j->json = spec_json(u, note);
    j->app = u->app ? u->app : app_id();
    ReleaseSRWLockExclusive(&g_lock);
    j->handle = handle;
    j->call = shim_callresult_reserve();
    uint64_t call = j->call;
    HANDLE t = CreateThread(NULL, 0, submit_thread, j, 0, NULL);
    if (!t) { shim_callresult_fail_handle(call); free(j->json); free(j); return call; }
    CloseHandle(t);
    return call;
}

static int SHIM_TC get_item_update_progress(void *self, uint64_t handle, uint64_t *processed, uint64_t *total) {
    (void)self;
    int status = 0;
    AcquireSRWLockShared(&g_lock);
    Update *u = find_update(handle);
    if (u) {
        status = u->submitted ? u->status : 1;
        if (processed) *processed = u->done;
        if (total) *total = u->total;
    }
    ReleaseSRWLockShared(&g_lock);
    return status;
}

static uint64_t SHIM_TC init_workshop_for_game_server(void *self, uint32_t depot, const char *folder) {
    (void)self; (void)depot;
    if (!folder || !*folder) return true;
    char path[1024], reply[8];
    size_t n = 0;
    unix_path(folder, path, sizeof path);
    int32_t result = 2;
    if (ugc_op(OP_SERVER_FOLDER, app_id(), 0, path, 15000, (uint8_t *)reply, sizeof reply, &n) && n >= 4) memcpy(&result, reply, 4);
    return result == 1;
}

void register_ugcwrite(void) {
    const char *p = "STEAMUGC_INTERFACE_VERSION";
    shim_register_family(p, "StartPlaytimeTracking", (void *)start_playtime_tracking);
    shim_register_family(p, "StopPlaytimeTracking", (void *)stop_playtime_tracking);
    shim_register_family(p, "StopPlaytimeTrackingForAllItems", (void *)stop_playtime_tracking_all);
    shim_register_family(p, "CreateItem", (void *)create_item);
    shim_register_family(p, "StartItemUpdate", (void *)start_item_update);
    shim_register_family(p, "SetItemTitle", (void *)set_item_title);
    shim_register_family(p, "SetItemDescription", (void *)set_item_description);
    shim_register_family(p, "SetItemUpdateLanguage", (void *)set_item_language);
    shim_register_family(p, "SetItemMetadata", (void *)set_item_metadata);
    shim_register_family(p, "SetItemVisibility", (void *)set_item_visibility);
    shim_register_family_ab(p, "SetItemTags", 12, (void *)set_item_tags);
    shim_register_family_ab(p, "SetItemTags", 16, (void *)set_item_tags_admin);
    shim_register_family(p, "SetItemContent", (void *)set_item_content);
    shim_register_family(p, "SetItemPreview", (void *)set_item_preview);
    shim_register_family(p, "SetAllowLegacyUpload", (void *)set_allow_legacy_upload);
    shim_register_family(p, "RemoveAllItemKeyValueTags", (void *)remove_all_item_kv_tags);
    shim_register_family(p, "RemoveItemKeyValueTags", (void *)remove_item_kv_tags);
    shim_register_family(p, "AddItemKeyValueTag", (void *)add_item_kv_tag);
    shim_register_family(p, "AddItemPreviewFile", (void *)add_item_preview_file);
    shim_register_family(p, "AddItemPreviewVideo", (void *)add_item_preview_video);
    shim_register_family(p, "UpdateItemPreviewFile", (void *)update_item_preview_file);
    shim_register_family(p, "UpdateItemPreviewVideo", (void *)update_item_preview_video);
    shim_register_family(p, "RemoveItemPreview", (void *)remove_item_preview);
    shim_register_family(p, "AddContentDescriptor", (void *)add_content_descriptor);
    shim_register_family(p, "RemoveContentDescriptor", (void *)remove_content_descriptor);
    shim_register_family(p, "SetRequiredGameVersions", (void *)set_required_game_versions);
    shim_register_family(p, "SubmitItemUpdate", (void *)submit_item_update);
    shim_register_family(p, "GetItemUpdateProgress", (void *)get_item_update_progress);
    shim_register_family(p, "DeleteItem", (void *)delete_item);
    shim_register_family(p, "BInitWorkshopForGameServer", (void *)init_workshop_for_game_server);
}
