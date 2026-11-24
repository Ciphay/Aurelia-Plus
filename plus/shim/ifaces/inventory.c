#include <ctype.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <windows.h>
#include <winhttp.h>
#include <shellapi.h>

#include "../shim.h"

enum {
    CB_PURCHASE_START = 4704, CB_PRICES = 4705, CB_RESULT_READY = 4700, CB_FULL_UPDATE = 4701, CB_DEF_UPDATE = 4702, CB_ELIGIBLE = 4703,
    MAX_RESULTS = 32, MAX_PROPS = 128, WAIT_MS = 20000,
    ER_OK = 1, ER_FAIL = 2, ER_INVALID = 8, ER_PENDING = 22,
};

typedef struct { char *key, *val; } Prop;
typedef struct { uint64_t id; int32_t def; uint16_t qty, flags; Prop p[MAX_PROPS]; int n_p; } Item;
typedef struct { int32_t handle; int32_t status; Item *items; int n; uint32_t when; bool used; } Result;
typedef struct {
    int32_t handle;
    const char *method;
    PbW body;
    int kind;
    uint64_t *filter;
    int n_filter;
} Job;

static Result g_results[MAX_RESULTS];
static int32_t g_next = 0x2000;
static SRWLOCK g_lock = SRWLOCK_INIT;
static char *g_defs_json;
static Item *g_defs;
static int g_n_defs;
static char g_currency[4] = "USD";
static bool g_prices_ready;
static uint64_t g_eligible[64];
static int g_n_eligible;
static volatile LONG g_defs_announced;

static uint64_t self_id(void) { const Snapshot *s = shim_snapshot(); return s ? s->steamid : 0; }
static uint32_t app_id(void) { const Snapshot *s = shim_snapshot(); return s && s->has_ticket ? s->ticket.appid : 0; }

static const char *skip_ws(const char *p) { while (*p == ' ' || *p == '\t' || *p == '\r' || *p == '\n' || *p == ',') p++; return p; }

static const char *parse_string(const char *p, char **out) {
    size_t cap = 64, n = 0;
    char *s = malloc(cap);
    if (!s) return NULL;
    for (p++; *p && *p != '"'; p++) {
        char c = *p;
        if (c == '\\' && p[1]) { p++; c = *p == 'n' ? '\n' : *p == 't' ? '\t' : *p; }
        if (n + 2 > cap) { cap *= 2; char *g = realloc(s, cap); if (!g) { free(s); return NULL; } s = g; }
        s[n++] = c;
    }
    s[n] = 0;
    *out = s;
    return *p == '"' ? p + 1 : p;
}

static const char *parse_scalar(const char *p, char **out) {
    if (*p == '"') return parse_string(p, out);
    const char *s = p;
    while (*p && *p != ',' && *p != '}' && *p != ']' && *p != ' ' && *p != '\n') p++;
    *out = malloc((size_t)(p - s) + 1);
    if (!*out) return NULL;
    memcpy(*out, s, (size_t)(p - s));
    (*out)[p - s] = 0;
    return p;
}

static void item_free(Item *it) {
    for (int i = 0; i < it->n_p; i++) { free(it->p[i].key); free(it->p[i].val); }
    memset(it, 0, sizeof *it);
}

static const char *item_prop(const Item *it, const char *key) {
    for (int i = 0; i < it->n_p; i++) if (!_stricmp(it->p[i].key, key)) return it->p[i].val;
    return NULL;
}

static void item_set(Item *it, const char *key, const char *val) {
    if (it->n_p >= MAX_PROPS) return;
    it->p[it->n_p].key = _strdup(key);
    it->p[it->n_p].val = _strdup(val);
    if (it->p[it->n_p].key && it->p[it->n_p].val) it->n_p++;
}

static Item *parse_objects(const char *json, int *n) {
    int cap = 16;
    Item *list = calloc((size_t)cap, sizeof *list);
    *n = 0;
    if (!list || !json) { free(list); return NULL; }
    const char *p = skip_ws(json);
    if (*p == '[') p++;
    for (;;) {
        p = skip_ws(p);
        if (*p != '{') break;
        p++;
        if (*n == cap) {
            cap *= 2;
            Item *g = realloc(list, (size_t)cap * sizeof *list);
            if (!g) break;
            list = g;
            memset(&list[*n], 0, (size_t)(cap - *n) * sizeof *list);
        }
        Item *it = &list[(*n)++];
        for (;;) {
            p = skip_ws(p);
            if (*p == '}') { p++; break; }
            if (*p != '"') { *n -= 0; break; }
            char *key = NULL, *val = NULL;
            p = parse_string(p, &key);
            p = skip_ws(p);
            if (!p || *p != ':') { free(key); break; }
            p = skip_ws(p + 1);
            if (*p == '{' || *p == '[') {
                int depth = 0;
                do { if (*p == '{' || *p == '[') depth++; else if (*p == '}' || *p == ']') depth--; p++; } while (*p && depth > 0);
                free(key);
                continue;
            }
            p = parse_scalar(p, &val);
            if (p && key && val) item_set(it, key, val);
            free(key);
            free(val);
            if (!p) break;
        }
    }
    return list;
}

static uint64_t to_u64(const char *s) { return s ? strtoull(s, NULL, 10) : 0; }

static uint16_t flags_from_state(const char *state) {
    uint16_t f = 0;
    if (!state) return 0;
    if (strstr(state, "NoTrade")) f |= 1;
    if (strstr(state, "removed")) f |= 0x100;
    if (strstr(state, "consumed")) f |= 0x200;
    return f;
}

static void merge_defs(Item *items, int n, const char *defs_json) {
    int nd = 0;
    Item *defs = parse_objects(defs_json, &nd);
    for (int i = 0; i < n; i++) {
        for (int d = 0; d < nd; d++) {
            const char *a = item_prop(&defs[d], "itemdefid"), *b = item_prop(&items[i], "itemdefid");
            if (!a || !b || strcmp(a, b)) continue;
            for (int k = 0; k < defs[d].n_p; k++) if (!item_prop(&items[i], defs[d].p[k].key) && items[i].n_p < MAX_PROPS) item_set(&items[i], defs[d].p[k].key, defs[d].p[k].val);
        }
    }
    for (int d = 0; d < nd; d++) item_free(&defs[d]);
    free(defs);
}

static Result *result_locked(int32_t h) {
    Result *r = &g_results[(uint32_t)h % MAX_RESULTS];
    return r->used && r->handle == h ? r : NULL;
}

static void result_clear(Result *r) {
    for (int i = 0; i < r->n; i++) item_free(&r->items[i]);
    free(r->items);
    memset(r, 0, sizeof *r);
}

static int32_t result_new(void) {
    AcquireSRWLockExclusive(&g_lock);
    int32_t h = ++g_next;
    Result *r = &g_results[(uint32_t)h % MAX_RESULTS];
    result_clear(r);
    r->used = true;
    r->handle = h;
    r->status = ER_PENDING;
    ReleaseSRWLockExclusive(&g_lock);
    return h;
}

static void post_ready(int32_t h, int32_t status) {
    uint8_t cb[8];
    memcpy(cb, &h, 4);
    memcpy(cb + 4, &status, 4);
    shim_post_callback(CB_RESULT_READY, cb, sizeof cb);
}

static void finish(int32_t h, int32_t status, Item *items, int n, bool full) {
    AcquireSRWLockExclusive(&g_lock);
    Result *r = result_locked(h);
    if (r) { r->items = items; r->n = n; r->status = status; r->when = (uint32_t)time(NULL); items = NULL; n = 0; }
    ReleaseSRWLockExclusive(&g_lock);
    for (int i = 0; i < n; i++) item_free(&items[i]);
    free(items);
    post_ready(h, status);
    if (full && status == ER_OK) shim_post_callback(CB_FULL_UPDATE, &h, 4);
}

static Item *items_from_response(const CmReply *r, int *n) {
    PbR in;
    PbField f;
    char *items_json = NULL, *defs_json = NULL;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field == 3 && f.wt == 2) { free(items_json); items_json = malloc(f.len + 1); if (items_json) { memcpy(items_json, f.data, f.len); items_json[f.len] = 0; } }
        else if (f.field == 4 && f.wt == 2) { free(defs_json); defs_json = malloc(f.len + 1); if (defs_json) { memcpy(defs_json, f.data, f.len); defs_json[f.len] = 0; } }
    }
    Item *items = parse_objects(items_json, n);
    for (int i = 0; i < *n; i++) {
        items[i].id = to_u64(item_prop(&items[i], "itemid"));
        items[i].def = (int32_t)to_u64(item_prop(&items[i], "itemdefid"));
        const char *q = item_prop(&items[i], "quantity");
        items[i].qty = (uint16_t)(q ? to_u64(q) : 1);
        items[i].flags = flags_from_state(item_prop(&items[i], "state"));
    }
    if (defs_json) {
        merge_defs(items, *n, defs_json);
        AcquireSRWLockExclusive(&g_lock);
        free(g_defs_json);
        g_defs_json = defs_json;
        ReleaseSRWLockExclusive(&g_lock);
    }
    free(items_json);
    return items;
}

static DWORD WINAPI job_thread(LPVOID arg) {
    Job *j = arg;
    CmReply r = {0};
    int32_t status = ER_FAIL;
    Item *items = NULL;
    int n = 0;
    if (j->kind == 1) {
        PbW q = {0};
        pbw_varint(&q, 1, app_id());
        pbw_varint(&q, 2, self_id());
        if (shim_cm_service("Inventory.GetEligiblePromoItemDefIDs#1", q.p, q.len, WAIT_MS, &r) && r.eresult == 1) {
            PbR in;
            PbField f;
            PbW add = {0};
            int count = 0;
            pbw_varint(&add, 1, app_id());
            pbr_init(&in, r.body, r.len);
            while (pbr_next(&in, &f)) {
                if (f.field != 1) continue;
                if (f.wt == 0) { pbw_varint(&add, 2, f.v); count++; }
                else if (f.wt == 2) { const uint8_t *p = f.data; size_t left = f.len; while (left) { uint64_t v = 0; int sh = 0; while (left) { uint8_t b = *p++; left--; v |= (uint64_t)(b & 0x7f) << sh; sh += 7; if (!(b & 0x80)) break; } pbw_varint(&add, 2, v); count++; } }
            }
            pbw_varint(&add, 4, self_id());
            pbw_varint(&add, 6, ((uint64_t)GetTickCount() << 20) ^ (uint64_t)time(NULL));
            shim_cm_reply_free(&r);
            if (count && shim_cm_service("Inventory.AddPromoItem#1", add.p, add.len, WAIT_MS, &r) && r.eresult == 1) { items = items_from_response(&r, &n); status = ER_OK; }
            else if (!count) status = ER_OK;
            pbw_free(&add);
        }
        pbw_free(&q);
    } else if (shim_cm_service(j->method, j->body.p, j->body.len, WAIT_MS, &r)) {
        if (r.eresult == 1 || r.len) { items = items_from_response(&r, &n); status = ER_OK; }
        else status = r.eresult;
        if (j->filter) {
            int keep = 0;
            for (int i = 0; i < n; i++) {
                bool wanted = false;
                for (int k = 0; k < j->n_filter; k++) wanted = wanted || j->filter[k] == items[i].id;
                if (wanted) items[keep++] = items[i]; else item_free(&items[i]);
            }
            n = keep;
        }
    }
    shim_cm_reply_free(&r);
    finish(j->handle, status, items, n, j->kind == 0 && !strcmp(j->method, "Inventory.GetInventory#1") && !j->filter);
    pbw_free(&j->body);
    free(j->filter);
    free(j);
    return 0;
}

static bool fail_job(uint32_t *out, int32_t status) {
    if (!out) return true;
    int32_t h = result_new();
    *out = (uint32_t)h;
    finish(h, status, NULL, 0, false);
    return true;
}

static bool start_job(uint32_t *out, const char *method, PbW *body, int kind, const uint64_t *filter, int n_filter) {
    if (!out) { pbw_free(body); return true; }
    Job *j = calloc(1, sizeof *j);
    if (!j) { pbw_free(body); return false; }
    j->handle = result_new();
    j->method = method;
    j->body = *body;
    memset(body, 0, sizeof *body);
    j->kind = kind;
    if (filter && n_filter > 0) {
        j->filter = malloc((size_t)n_filter * 8);
        if (!j->filter) { pbw_free(&j->body); free(j); return false; }
        memcpy(j->filter, filter, (size_t)n_filter * 8);
        j->n_filter = n_filter;
    }
    *out = (uint32_t)j->handle;
    HANDLE t = CreateThread(NULL, 0, job_thread, j, 0, NULL);
    if (!t) { job_thread(j); return true; }
    CloseHandle(t);
    return true;
}

static int unknown_handle_status(void *self) {
    const char *name = shim_iface_name(self);
    return name && strstr(name, "V001") ? 8 : 2;
}

static int SHIM_TC result_status(void *self, int32_t h) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Result *r = result_locked(h);
    int s = r ? r->status : unknown_handle_status(self);
    ReleaseSRWLockExclusive(&g_lock);
    return s;
}

static uint64_t SHIM_TC result_items(void *self, int32_t h, uint8_t *out, uint32_t *count) {
    (void)self;
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    Result *r = result_locked(h);
    if (r && r->status == ER_OK && count) {
        if (out) {
            uint32_t n = *count < (uint32_t)r->n ? *count : (uint32_t)r->n;
            for (uint32_t i = 0; i < n; i++) {
                memcpy(out + i * 16, &r->items[i].id, 8);
                memcpy(out + i * 16 + 8, &r->items[i].def, 4);
                memcpy(out + i * 16 + 12, &r->items[i].qty, 2);
                memcpy(out + i * 16 + 14, &r->items[i].flags, 2);
            }
        }
        *count = (uint32_t)r->n;
        ok = true;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static bool give_text(const char *text, char *buf, uint32_t *size) {
    size_t need = strlen(text) + 1;
    if (!size) return false;
    if (buf && *size >= need) { memcpy(buf, text, need); *size = (uint32_t)need; return true; }
    *size = (uint32_t)need;
    return buf == NULL;
}

static uint64_t SHIM_TC result_item_property(void *self, int32_t h, uint32_t index, const char *name, char *buf, uint32_t *size) {
    (void)self;
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    Result *r = result_locked(h);
    if (r && r->status == ER_OK && index < (uint32_t)r->n) {
        const Item *it = &r->items[index];
        if (!name || !*name) {
            char list[1024] = "";
            for (int i = 0; i < it->n_p; i++) { if (i) strncat(list, ",", sizeof list - strlen(list) - 1); strncat(list, it->p[i].key, sizeof list - strlen(list) - 1); }
            ok = give_text(list, buf, size);
        } else {
            const char *v = item_prop(it, name);
            if (v) ok = give_text(v, buf, size);
        }
    }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static uint32_t SHIM_TC result_timestamp(void *self, int32_t h) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Result *r = result_locked(h);
    uint32_t t = r ? r->when : 0;
    ReleaseSRWLockExclusive(&g_lock);
    return t;
}

static uint64_t SHIM_TC check_result_steamid(void *self, int32_t h, uint64_t id) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    bool ok = result_locked(h) && id == self_id();
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static void SHIM_TC destroy_result(void *self, int32_t h) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Result *r = result_locked(h);
    if (r && r->status != ER_PENDING) result_clear(r);
    ReleaseSRWLockExclusive(&g_lock);
}

static void body_head(PbW *w) {
    pbw_varint(w, 1, app_id());
    pbw_varint(w, 2, self_id());
}

static uint64_t SHIM_TC get_all_items(void *self, uint32_t *out) {
    (void)self;
    PbW w = {0};
    body_head(&w);
    return start_job(out, "Inventory.GetInventory#1", &w, 0, NULL, 0);
}

static uint64_t SHIM_TC get_items_by_id(void *self, uint32_t *out, const uint64_t *ids, uint32_t n) {
    (void)self;
    if (!ids || !n || n > 4096) return fail_job(out, ER_INVALID);
    PbW w = {0};
    body_head(&w);
    return start_job(out, "Inventory.GetInventory#1", &w, 0, ids, (int)n);
}

static uint64_t SHIM_TC consume_item(void *self, uint32_t *out, uint64_t item, uint32_t qty) {
    (void)self;
    if (!item || !qty) return fail_job(out, ER_INVALID);
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_varint(&w, 2, item);
    pbw_varint(&w, 3, qty);
    pbw_varint(&w, 5, self_id());
    pbw_varint(&w, 6, ((uint64_t)GetTickCount() << 20) ^ (uint64_t)time(NULL));
    return start_job(out, "Inventory.ConsumeItem#1", &w, 0, NULL, 0);
}

static uint64_t SHIM_TC add_promo_items(void *self, uint32_t *out, const int32_t *defs, uint32_t n) {
    (void)self;
    if (!defs || !n || n > 256) return 0;
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    for (uint32_t i = 0; i < n; i++) pbw_varint(&w, 2, (uint64_t)(uint32_t)defs[i]);
    pbw_varint(&w, 4, self_id());
    pbw_varint(&w, 6, ((uint64_t)GetTickCount() << 20) ^ (uint64_t)time(NULL));
    return start_job(out, "Inventory.AddPromoItem#1", &w, 0, NULL, 0);
}

static uint64_t SHIM_TC add_promo_item(void *self, uint32_t *out, int32_t def) { return add_promo_items(self, out, &def, 1); }

static uint64_t SHIM_TC grant_promo_items(void *self, uint32_t *out) {
    (void)self;
    PbW w = {0};
    return start_job(out, "", &w, 1, NULL, 0);
}

static uint64_t SHIM_TC exchange_items(void *self, uint32_t *out, const int32_t *gen_defs, const uint32_t *gen_qty, uint32_t n_gen,
                                       const uint64_t *destroy, const uint32_t *destroy_qty, uint32_t n_destroy) {
    (void)self;
    (void)gen_qty;
    if (!gen_defs || n_gen != 1 || !destroy || !destroy_qty || !n_destroy || n_destroy > 256) return 0;
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_varint(&w, 2, self_id());
    for (uint32_t i = 0; i < n_destroy; i++) pbw_varint(&w, 3, destroy[i]);
    for (uint32_t i = 0; i < n_destroy; i++) pbw_varint(&w, 4, destroy_qty[i]);
    pbw_varint(&w, 5, (uint64_t)(uint32_t)gen_defs[0]);
    return start_job(out, "Inventory.ExchangeItem#1", &w, 0, NULL, 0);
}

static char *http_get(const char *url) {
    wchar_t wurl[512];
    if (!MultiByteToWideChar(CP_UTF8, 0, url, -1, wurl, 512)) return NULL;
    wchar_t host[256] = L"", path[512] = L"";
    URL_COMPONENTS uc;
    memset(&uc, 0, sizeof uc);
    uc.dwStructSize = sizeof uc;
    uc.lpszHostName = host; uc.dwHostNameLength = 256; uc.lpszUrlPath = path; uc.dwUrlPathLength = 512;
    HINTERNET session = NULL, conn = NULL, req = NULL;
    char *out = NULL;
    size_t n = 0;
    if (WinHttpCrackUrl(wurl, 0, 0, &uc)) {
        session = WinHttpOpen(L"Valve/Steam HTTP Client 1.0", WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, NULL, NULL, 0);
        conn = session ? WinHttpConnect(session, host, uc.nPort, 0) : NULL;
        req = conn ? WinHttpOpenRequest(conn, L"GET", path, NULL, NULL, NULL, uc.nScheme == INTERNET_SCHEME_HTTPS ? WINHTTP_FLAG_SECURE : 0) : NULL;
        DWORD status = 0, sl = sizeof status;
        if (req && WinHttpSendRequest(req, NULL, 0, NULL, 0, 0, 0) && WinHttpReceiveResponse(req, NULL)
            && WinHttpQueryHeaders(req, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, NULL, &status, &sl, NULL) && status == 200) {
            DWORD got;
            char buf[8192];
            out = malloc(1);
            while (out && WinHttpReadData(req, buf, sizeof buf, &got) && got) {
                char *g = realloc(out, n + got + 1);
                if (!g) { free(out); out = NULL; break; }
                out = g;
                memcpy(out + n, buf, got);
                n += got;
            }
            if (out) out[n] = 0;
        }
    }
    if (req) WinHttpCloseHandle(req);
    if (conn) WinHttpCloseHandle(conn);
    if (session) WinHttpCloseHandle(session);
    return out;
}

static DWORD WINAPI definitions_thread(LPVOID unused) {
    (void)unused;
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    CmReply r = {0};
    char digest[96] = "";
    if (shim_cm_service("Inventory.GetItemDefMeta#1", w.p, w.len, WAIT_MS, &r) && r.eresult == 1) {
        PbR in;
        PbField f;
        pbr_init(&in, r.body, r.len);
        while (pbr_next(&in, &f)) if (f.field == 2 && f.wt == 2 && f.len < sizeof digest) { memcpy(digest, f.data, f.len); digest[f.len] = 0; }
    }
    shim_cm_reply_free(&r);
    pbw_free(&w);
    if (digest[0]) {
        char url[256];
        snprintf(url, sizeof url, "https://api.steampowered.com/IGameInventory/GetItemDefArchive/v0001/?appid=%u&digest=%s", app_id(), digest);
        char *json = http_get(url);
        if (json && strchr(json, '{')) {
            int nd = 0;
            Item *parsed = parse_objects(json, &nd);
            AcquireSRWLockExclusive(&g_lock);
            for (int i = 0; i < g_n_defs; i++) item_free(&g_defs[i]);
            free(g_defs);
            g_defs = parsed;
            g_n_defs = nd;
            ReleaseSRWLockExclusive(&g_lock);
        }
        free(json);
    }
    shim_post_callback(CB_DEF_UPDATE, NULL, 0);
    return 0;
}

static void announce_definitions(void) {
    if (InterlockedCompareExchange(&g_defs_announced, 1, 0)) return;
    HANDLE t = CreateThread(NULL, 0, definitions_thread, NULL, 0, NULL);
    if (t) CloseHandle(t);
}

static uint64_t SHIM_TC load_definitions(void *self) { (void)self; announce_definitions(); return 1; }

static void ensure_defs_locked(void) {
    if (g_defs || !g_defs_json) return;
    g_defs = parse_objects(g_defs_json, &g_n_defs);
}

static uint64_t SHIM_TC definition_ids(void *self, int32_t *out, uint32_t *count) {
    (void)self;
    announce_definitions();
    bool ok = count != NULL;
    AcquireSRWLockExclusive(&g_lock);
    ensure_defs_locked();
    if (ok) {
        uint32_t cap = *count, n = 0;
        for (int i = 0; i < g_n_defs; i++) {
            const char *id = item_prop(&g_defs[i], "itemdefid");
            if (!id) continue;
            if (out && n < cap) out[n] = (int32_t)to_u64(id);
            n++;
        }
        *count = n;
        ok = !out || cap >= n;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static Item *find_def_locked(int32_t def) {
    char want[16];
    snprintf(want, sizeof want, "%d", def);
    ensure_defs_locked();
    for (int i = 0; i < g_n_defs; i++) {
        const char *id = item_prop(&g_defs[i], "itemdefid");
        if (id && !strcmp(id, want)) return &g_defs[i];
    }
    return NULL;
}

static uint64_t SHIM_TC definition_property(void *self, int32_t def, const char *name, char *buf, uint32_t *size) {
    (void)self;
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    Item *d = find_def_locked(def);
    if (d) {
        if (!name || !*name) {
            char list[2048] = "";
            for (int k = 0; k < d->n_p; k++) { if (k) strncat(list, ",", sizeof list - strlen(list) - 1); strncat(list, d->p[k].key, sizeof list - strlen(list) - 1); }
            ok = give_text(list, buf, size);
        } else {
            const char *v = item_prop(d, name);
            if (v) ok = give_text(v, buf, size);
        }
    }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static bool parse_eligible(const CmReply *r, void *ctx, void *out, int *size) {
    (void)ctx;
    if (*size < 24) return false;
    uint64_t ids[64];
    int n = 0;
    PbR in;
    PbField f;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field != 1) continue;
        if (f.wt == 0 && n < 64) ids[n++] = f.v;
        else if (f.wt == 2) {
            const uint8_t *p = f.data;
            size_t left = f.len;
            while (left && n < 64) {
                uint64_t v = 0;
                int sh = 0;
                while (left) { uint8_t b = *p++; left--; v |= (uint64_t)(b & 0x7f) << sh; sh += 7; if (!(b & 0x80)) break; }
                ids[n++] = v;
            }
        }
    }
    if (in.bad) return false;
    AcquireSRWLockExclusive(&g_lock);
    memcpy(g_eligible, ids, (size_t)n * 8);
    g_n_eligible = n;
    ReleaseSRWLockExclusive(&g_lock);
    int32_t ok = r->eresult == 1 ? 1 : r->eresult;
    uint64_t sid = self_id();
    int32_t cnt = n;
    memset(out, 0, 24);
    memcpy(out, &ok, 4);
    memcpy((uint8_t *)out + 8, &sid, 8);
    memcpy((uint8_t *)out + 16, &cnt, 4);
    *size = 24;
    return true;
}

static uint64_t SHIM_TC request_eligible(void *self, uint64_t steamid) {
    (void)self;
    if (!steamid) return 0;
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    body_head(&w);
    shim_cm_async(call, CB_ELIGIBLE, 0, true, "Inventory.GetEligiblePromoItemDefIDs#1", &w, WAIT_MS, parse_eligible, NULL);
    return call;
}

static uint64_t SHIM_TC eligible_ids(void *self, uint64_t steamid, int32_t *out, uint32_t *count) {
    (void)self;
    (void)steamid;
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    if (count) {
        uint32_t cap = *count;
        for (int i = 0; out && i < g_n_eligible && (uint32_t)i < cap; i++) out[i] = (int32_t)g_eligible[i];
        *count = (uint32_t)g_n_eligible;
        ok = !out || cap >= (uint32_t)g_n_eligible;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static const char *const CURRENCY_CODES[] = { "", "USD", "GBP", "EUR", "CHF", "RUB", "PLN", "BRL", "JPY", "NOK", "IDR", "MYR", "PHP", "SGD", "THB", "VND", "KRW", "TRY", "UAH", "MXN", "CAD", "AUD", "NZD", "CNY", "INR", "CLP", "PEN", "COP", "ZAR", "HKD", "TWD", "SAR", "AED", "SEK", "ARS", "ILS", "BYN", "KZT", "KWD", "QAR", "CRC", "UYU" };

static bool price_in(const char *price, const char *code, uint64_t *out) {
    const char *p = strchr(price, ';');
    p = p ? p + 1 : price;
    uint64_t usd = 0;
    bool have_usd = false;
    while (*p) {
        const char *end = strchr(p, ',');
        size_t len = end ? (size_t)(end - p) : strlen(p);
        if (len > 3 && isalpha((unsigned char)p[0])) {
            uint64_t v = strtoull(p + 3, NULL, 10);
            if (!strncmp(p, code, 3)) { *out = v; return true; }
            if (!strncmp(p, "USD", 3) || !strncmp(p, "VLV", 3)) { usd = v; have_usd = true; }
        }
        if (!end) break;
        p = end + 1;
    }
    if (have_usd && !strcmp(code, "USD")) { *out = usd; return true; }
    return false;
}

static bool parse_prices(const CmReply *r, void *ctx, void *out, int *size) {
    (void)ctx;
    PbR in;
    PbField f;
    int32_t code = 1;
    if (*size < 8) return false;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) if (f.field == 1) code = (int32_t)(uint32_t)f.v;
    if (in.bad || r->eresult != 1) return false;
    announce_definitions();
    AcquireSRWLockExclusive(&g_lock);
    snprintf(g_currency, sizeof g_currency, "%s", code > 0 && (size_t)code < sizeof CURRENCY_CODES / sizeof *CURRENCY_CODES ? CURRENCY_CODES[code] : "USD");
    g_prices_ready = true;
    ReleaseSRWLockExclusive(&g_lock);
    int32_t ok = 1;
    memset(out, 0, 8);
    memcpy(out, &ok, 4);
    memcpy((uint8_t *)out + 4, g_currency, 4);
    *size = 8;
    return true;
}

static uint64_t SHIM_TC request_prices(void *self) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    shim_cm_async(call, CB_PRICES, 0, true, "Inventory.GetUserPurchaseInfo#1", &w, WAIT_MS, parse_prices, NULL);
    return call;
}

static bool def_price(int32_t def, uint64_t *price) {
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    Item *d = find_def_locked(def);
    const char *p = d ? item_prop(d, "price") : NULL;
    if (p && *p) ok = price_in(p, g_currency, price);
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static uint32_t SHIM_TC num_items_with_prices(void *self) {
    (void)self;
    uint32_t n = 0;
    AcquireSRWLockExclusive(&g_lock);
    ensure_defs_locked();
    for (int i = 0; g_prices_ready && i < g_n_defs; i++) {
        const char *p = item_prop(&g_defs[i], "price");
        uint64_t v;
        if (p && *p && price_in(p, g_currency, &v)) n++;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return n;
}

static uint64_t SHIM_TC items_with_prices(void *self, int32_t *defs, uint64_t *prices, uint64_t *base, uint32_t n) {
    (void)self;
    if (!defs || !prices || n == 0) return 0;
    uint32_t k = 0;
    AcquireSRWLockExclusive(&g_lock);
    ensure_defs_locked();
    for (int i = 0; g_prices_ready && i < g_n_defs && k < n; i++) {
        const char *p = item_prop(&g_defs[i], "price"), *id = item_prop(&g_defs[i], "itemdefid");
        uint64_t v;
        if (!id || !p || !*p || !price_in(p, g_currency, &v)) continue;
        defs[k] = (int32_t)to_u64(id);
        prices[k] = v;
        if (base) base[k] = v;
        k++;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return k > 0;
}

static uint64_t SHIM_TC item_price(void *self, int32_t def, uint64_t *price, uint64_t *base) {
    (void)self;
    uint64_t v;
    if (!g_prices_ready || !def_price(def, &v)) return 0;
    if (price) *price = v;
    if (base) *base = v;
    return 1;
}

typedef struct { uint64_t call; uint32_t n; int32_t defs[32]; uint32_t qty[32]; DWORD t0; } PurchaseJob;

static void open_checkout(uint64_t trans) {
    char url[160];
    snprintf(url, sizeof url, "https://checkout.steampowered.com/checkout/approvetxn/%llu/?returnurl=steam", (unsigned long long)trans);
    if (getenv("AURELIA_SHIM_NO_BROWSER")) { shim_log("purchase: open %s", url); return; }
    if (shim_overlay_capable() && !getenv("AURELIA_SHIM_OPEN_CHECKOUT")) { shim_log("purchase: the launcher handles the approval (microtxn_request); page %s is not opened", url); return; }
    shim_log("purchase: opening %s", url);
    ShellExecuteA(NULL, "open", url, NULL, NULL, SW_SHOWNORMAL);
}

static uint32_t language_id(const char *name) {
    static const struct { const char *n; uint32_t id; } t[] = { { "english", 1 }, { "german", 2 }, { "french", 3 }, { "italian", 4 }, { "koreana", 5 }, { "spanish", 6 },
        { "schinese", 7 }, { "tchinese", 8 }, { "russian", 9 }, { "thai", 10 }, { "japanese", 11 }, { "portuguese", 12 }, { "polish", 13 }, { "brazilian", 23 }, { "latam", 28 } };
    for (size_t k = 0; k < sizeof t / sizeof *t; k++) if (!_stricmp(name, t[k].n)) return t[k].id;
    return 1;
}

static DWORD WINAPI purchase_thread(LPVOID arg) {
    PurchaseJob *j = arg;
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_varint(&w, 2, language_id(shim_game_language()));
    for (uint32_t i = 0; i < j->n; i++) {
        PbW line = {0};
        pbw_varint(&line, 1, (uint64_t)(uint32_t)j->defs[i]);
        pbw_varint(&line, 2, j->qty[i]);
        pbw_msg(&w, 3, &line);
        pbw_free(&line);
    }
    CmReply r = {0};
    uint64_t order = 0, trans = 0;
    int32_t result = 2;
    bool answered = shim_cm_service("Inventory.PurchaseInit#1", w.p, w.len, WAIT_MS, &r);
    shim_log("purchase: PurchaseInit language=%u answered=%d eresult=%d", (unsigned)language_id(shim_game_language()), answered, answered ? (int)r.eresult : -1);
    if (answered) {
        result = r.eresult;
        PbR in;
        PbField f;
        pbr_init(&in, r.body, r.len);
        while (pbr_next(&in, &f)) { if (f.field == 1) order = f.v; else if (f.field == 2) trans = f.v; }
    }
    shim_cm_reply_free(&r);
    pbw_free(&w);
    uint8_t cb[24] = {0};
    memcpy(cb, &result, 4);
    memcpy(cb + 8, &order, 8);
    memcpy(cb + 16, &trans, 8);
    bool launcher = result == 1 && order && shim_overlay_capable();
    if (launcher) shim_overlay_activated_now(1);
    shim_callresult_complete(j->call, CB_PURCHASE_START, cb, sizeof cb);
    shim_log("purchase: StartPurchaseResult result=%d order=%llu transaction=%llu (%lu ms after StartPurchase; on the real Steam: ~470 ms)", result, (unsigned long long)order, (unsigned long long)trans, (unsigned long)(GetTickCount() - j->t0));
    if (result != 1 || !order) { free(j); return 0; }
    open_checkout(trans);
    if (launcher) {
        int decision = shim_microtxn_wait(order, 600000);
        if (decision != MTX_APPROVED) { shim_log("purchase %llu not approved (decision=%d): not finalizing", (unsigned long long)order, decision); free(j); return 0; }
    }
    const char *poll = getenv("AURELIA_SHIM_PURCHASE_POLL_MS");
    int poll_ms = poll && atoi(poll) > 0 ? atoi(poll) : 3000;
    for (int attempt = 0; attempt < 600000 / poll_ms; attempt++) {
        Sleep((DWORD)poll_ms);
        PbW fin = {0};
        pbw_varint(&fin, 1, app_id());
        pbw_varint(&fin, 2, 0);
        pbw_varint(&fin, 3, order);
        CmReply fr = {0};
        bool got = shim_cm_service("Inventory.PurchaseFinalize#1", fin.p, fin.len, WAIT_MS, &fr);
        pbw_free(&fin);
        if (got && fr.eresult == 1) {
            int32_t h = result_new();
            int n = 0;
            Item *items = items_from_response(&fr, &n);
            shim_cm_reply_free(&fr);
            finish(h, ER_OK, items, n, false);
            shim_log("purchase %llu completed: %d items", (unsigned long long)order, n);
            break;
        }
        shim_cm_reply_free(&fr);
    }
    free(j);
    return 0;
}

static uint64_t SHIM_TC start_purchase(void *self, const int32_t *defs, const uint32_t *qty, uint32_t n) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    PurchaseJob *j = calloc(1, sizeof *j);
    if (!j || !defs || !qty || n == 0 || n > 32) { free(j); shim_callresult_fail_handle(call); return call; }
    j->call = call;
    j->t0 = GetTickCount();
    j->n = n;
    memcpy(j->defs, defs, n * 4);
    memcpy(j->qty, qty, n * 4);
    HANDLE t = CreateThread(NULL, 0, purchase_thread, j, 0, NULL);
    if (!t) { free(j); shim_callresult_fail_handle(call); return call; }
    CloseHandle(t);
    return call;
}

static void SHIM_TC drop_heartbeat(void *self) { (void)self; }

typedef struct { uint64_t handle; PbW updates; bool used; } Update;
static Update g_updates[8];
static uint64_t g_next_update = 0x500;

static uint64_t SHIM_TC start_update(void *self) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    uint64_t h = ++g_next_update;
    Update *u = &g_updates[h % 8];
    pbw_free(&u->updates);
    memset(u, 0, sizeof *u);
    u->handle = h;
    u->used = true;
    ReleaseSRWLockExclusive(&g_lock);
    return h;
}

static bool add_update(uint64_t h, uint64_t item, const char *name, int kind, const void *value) {
    if (!name || !*name) return false;
    PbW m = {0};
    pbw_varint(&m, 1, item);
    if (kind == 0) pbw_varint(&m, 2, 1);
    pbw_str(&m, 3, name);
    if (kind == 1) pbw_varint(&m, 4, *(const uint8_t *)value != 0);
    else if (kind == 2) pbw_varint(&m, 5, (uint64_t)*(const int64_t *)value);
    else if (kind == 3) pbw_str(&m, 6, (const char *)value);
    else if (kind == 4) pbw_fixed32(&m, 7, *(const uint32_t *)value);
    AcquireSRWLockExclusive(&g_lock);
    Update *u = &g_updates[h % 8];
    bool ok = u->used && u->handle == h;
    if (ok) pbw_msg(&u->updates, 3, &m);
    ReleaseSRWLockExclusive(&g_lock);
    pbw_free(&m);
    return ok;
}

static uint64_t SHIM_TC remove_property(void *self, uint64_t h, uint64_t item, const char *name) { (void)self; return add_update(h, item, name, 0, NULL); }
static uint64_t SHIM_TC set_property_string(void *self, uint64_t h, uint64_t item, const char *name, const char *v) { (void)self; return v && add_update(h, item, name, 3, v); }
static uint64_t SHIM_TC set_property_bool(void *self, uint64_t h, uint64_t item, const char *name, uint8_t v) { (void)self; return add_update(h, item, name, 1, &v); }
static uint64_t SHIM_TC set_property_int(void *self, uint64_t h, uint64_t item, const char *name, int64_t v) { (void)self; return add_update(h, item, name, 2, &v); }
static uint64_t SHIM_TC set_property_float(void *self, uint64_t h, uint64_t item, const char *name, float v) { (void)self; uint32_t bits; memcpy(&bits, &v, 4); return add_update(h, item, name, 4, &bits); }

static uint64_t SHIM_TC submit_update(void *self, uint64_t h, uint32_t *out) {
    (void)self;
    PbW body = {0};
    bool ok = false;
    pbw_varint(&body, 1, app_id());
    pbw_varint(&body, 2, self_id());
    AcquireSRWLockExclusive(&g_lock);
    Update *u = &g_updates[h % 8];
    if (u->used && u->handle == h) {
        pbw_append(&body, u->updates.p, u->updates.len);
        pbw_free(&u->updates);
        u->used = false;
        ok = true;
    }
    ReleaseSRWLockExclusive(&g_lock);
    pbw_varint(&body, 4, (uint64_t)time(NULL));
    if (!ok) { pbw_free(&body); return 0; }
    return start_job(out, "Inventory.SafeModifyItems#1", &body, 0, NULL, 0);
}

static uint64_t SHIM_TC trigger_drop(void *self, uint32_t *out, int32_t list) {
    (void)self;
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_varint(&w, 2, (uint64_t)(uint32_t)list);
    return start_job(out, "Inventory.ConsumePlaytime#1", &w, 0, NULL, 0);
}

static uint64_t SHIM_TC transfer_quantity(void *self, uint32_t *out, uint64_t from, uint32_t qty, uint64_t dest) {
    (void)self;
    if (!from || !qty) return 0;
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    if (dest == ~0ULL || !dest) {
        pbw_varint(&w, 2, from);
        pbw_varint(&w, 3, qty);
        pbw_varint(&w, 5, self_id());
        return start_job(out, "Inventory.SplitItemStack#1", &w, 0, NULL, 0);
    }
    pbw_varint(&w, 2, from);
    pbw_varint(&w, 3, dest);
    pbw_varint(&w, 4, qty);
    pbw_fixed64(&w, 7, self_id());
    return start_job(out, "Inventory.CombineItemStacks#1", &w, 0, NULL, 0);
}

static uint64_t SHIM_TC inspect_item(void *self, uint32_t *out, const char *token) {
    (void)self;
    if (!token || !*token) return 0;
    PbW w = {0};
    pbw_str(&w, 3, token);
    return start_job(out, "Inventory.InspectItem#1", &w, 0, NULL, 0);
}

static uint64_t SHIM_TC deserialize_result(void *self, uint32_t *out, const void *buf, uint32_t cb) { (void)self; (void)buf; (void)cb; return fail_job(out, 2); }
static uint64_t SHIM_TC deserialize_result16(void *self, uint32_t *out, const void *buf, uint32_t cb, uint8_t reserved) { (void)reserved; return deserialize_result(self, out, buf, cb); }
static uint64_t SHIM_TC generate_items(void *self, uint32_t *out, const int32_t *defs, const uint32_t *qty, uint32_t n) { (void)self; (void)defs; (void)qty; (void)n; return fail_job(out, 15); }

void register_inventory(void) {
    const char *p = "STEAMINVENTORY_INTERFACE";
    shim_register_family(p, "GetResultStatus", (void *)result_status);
    shim_register_family(p, "GetResultItems", (void *)result_items);
    shim_register_family(p, "GetResultItemProperty", (void *)result_item_property);
    shim_register_family(p, "GetResultTimestamp", (void *)result_timestamp);
    shim_register_family(p, "CheckResultSteamID", (void *)check_result_steamid);
    shim_register_family(p, "DestroyResult", (void *)destroy_result);
    shim_register_family(p, "GetAllItems", (void *)get_all_items);
    shim_register_family(p, "GetItemsByID", (void *)get_items_by_id);
    shim_register_family(p, "ConsumeItem", (void *)consume_item);
    shim_register_family(p, "AddPromoItem", (void *)add_promo_item);
    shim_register_family(p, "AddPromoItems", (void *)add_promo_items);
    shim_register_family(p, "GrantPromoItems", (void *)grant_promo_items);
    shim_register_family(p, "ExchangeItems", (void *)exchange_items);
    shim_register_family(p, "SendItemDropHeartbeat", (void *)drop_heartbeat);
    shim_register_family(p, "LoadItemDefinitions", (void *)load_definitions);
    shim_register_family(p, "GetItemDefinitionIDs", (void *)definition_ids);
    shim_register_family(p, "GetItemDefinitionProperty", (void *)definition_property);
    shim_register_family(p, "RequestEligiblePromoItemDefinitionsIDs", (void *)request_eligible);
    shim_register_family_ab(p, "DeserializeResult", 12, (void *)deserialize_result);
    shim_register_family_ab(p, "DeserializeResult", 16, (void *)deserialize_result16);
    shim_register_family(p, "GenerateItems", (void *)generate_items);
    shim_register_family(p, "GetEligiblePromoItemDefinitionIDs", (void *)eligible_ids);
    shim_register_family(p, "StartUpdateProperties", (void *)start_update);
    shim_register_family(p, "RemoveProperty", (void *)remove_property);
    shim_register_family(p, "SetProperty", (void *)set_property_string);
    shim_register_family(p, "SetProperty_2", (void *)set_property_bool);
    shim_register_family(p, "SetProperty_3", (void *)set_property_int);
    shim_register_family(p, "SetProperty_4", (void *)set_property_float);
    shim_register_family(p, "SubmitUpdateProperties", (void *)submit_update);
    shim_register_family(p, "TriggerItemDrop", (void *)trigger_drop);
    shim_register_family(p, "TransferItemQuantity", (void *)transfer_quantity);
    shim_register_family(p, "InspectItem", (void *)inspect_item);
    shim_register_family(p, "RequestPrices", (void *)request_prices);
    shim_register_family(p, "GetNumItemsWithPrices", (void *)num_items_with_prices);
    shim_register_family(p, "GetItemsWithPrices", (void *)items_with_prices);
    shim_register_family(p, "GetItemPrice", (void *)item_price);
    shim_register_family(p, "StartPurchase", (void *)start_purchase);
}
