#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <windows.h>
#include <winhttp.h>

#include "../shim.h"

enum {
    CB_QUERY_DONE = 3401, CB_DETAILS_RESULT = 3402, MAX_QUERIES = 16, MAX_RESULTS = 100, MAX_IDS = 256, MAX_TAGS = 16,
    DETAILS_SIZE_OLD = 9776, DETAILS_SIZE_NEW = 9784, WAIT_MS = 20000,
};
#define NO_HANDLE (~0ULL)

typedef struct { const char **strings; int32_t n; } ParamStringArray;

enum {
    D_FILEID = 0, D_RESULT = 8, D_TYPE = 12, D_CREATOR_APP = 16, D_CONSUMER_APP = 20, D_TITLE = 24, D_DESC = 153, D_OWNER = 8160,
    D_CREATED = 8168, D_UPDATED = 8172, D_ADDED = 8176, D_VISIBILITY = 8180, D_BANNED = 8184, D_ACCEPTED = 8185, D_TRUNC = 8186,
    D_TAGS = 8187, D_HFILE = 9216, D_HPREVIEW = 9224, D_FILENAME = 9232, D_FSIZE = 9492, D_PSIZE = 9496, D_URL = 9500,
    D_UP = 9756, D_DOWN = 9760, D_SCORE = 9764, D_CHILDREN = 9768, D_TOTAL = 9776,
};

typedef struct { char tag[64], display[64]; } Tag;
typedef struct { char key[256], value[256]; } Kv;
typedef struct { char url[512], file[260]; uint32_t type; } Preview;
typedef struct {
    uint8_t details[DETAILS_SIZE_NEW];
    Tag tags[MAX_TAGS]; int n_tags;
    Kv kv[MAX_TAGS]; int n_kv;
    Preview prev[4]; int n_prev;
    char preview_url[512];
    char *metadata;
    uint64_t children[32]; int n_children;
    uint64_t stat[13];
} Entry;

typedef struct {
    uint64_t handle;
    bool used, done;
    int kind;
    uint32_t list_type, matching, sort, query_type, creator_app, consumer_app, page, account, trend_days;
    char cursor[256], search[256], language[32], next_cursor[256], filename_filter[128];
    uint64_t ids[MAX_IDS];
    int n_ids;
    char req_tags[MAX_TAGS][64], excl_tags[MAX_TAGS][64];
    int n_req, n_excl;
    bool only_ids, kv_tags, long_desc, metadata, children, previews, total_only, match_any;
    Entry *r;
    int n, total, result;
} Query;

static Query g_q[MAX_QUERIES];
static uint64_t g_next = 1;
static SRWLOCK g_lock = SRWLOCK_INIT;

static uint32_t app_id(void) { const Snapshot *s = shim_snapshot(); return s && s->has_ticket ? s->ticket.appid : 0; }
static Query *find(uint64_t h) { Query *q = &g_q[h % MAX_QUERIES]; return q->used && q->handle == h ? q : NULL; }

static void copy_str(char *dst, size_t cap, const PbField *f) {
    size_t n = f->len < cap - 1 ? f->len : cap - 1;
    memcpy(dst, f->data, n);
    dst[n] = 0;
}

static void put32(uint8_t *d, int off, uint64_t v) { uint32_t x = (uint32_t)v; memcpy(d + off, &x, 4); }
static void put64(uint8_t *d, int off, uint64_t v) { memcpy(d + off, &v, 8); }

static void remember_ugc_details(const Entry *e);

static void read_details(const uint8_t *data, size_t len, Entry *e) {
    PbR in;
    PbField f;
    uint8_t *d = e->details;
    char tags[1100] = "";
    memset(e, 0, sizeof *e);
    pbr_init(&in, data, len);
    while (pbr_next(&in, &f)) {
        switch (f.field) {
        case 1: put32(d, D_RESULT, f.v); break;
        case 2: put64(d, D_FILEID, f.v); break;
        case 3: put64(d, D_OWNER, f.v); break;
        case 4: put32(d, D_CREATOR_APP, f.v); break;
        case 5: put32(d, D_CONSUMER_APP, f.v); break;
        case 7: if (f.wt == 2) copy_str((char *)d + D_FILENAME, 260, &f); break;
        case 8: put32(d, D_FSIZE, f.v); put64(d, D_TOTAL, f.v); break;
        case 9: put32(d, D_PSIZE, f.v); break;
        case 10: if (f.wt == 2) copy_str((char *)d + D_URL, 256, &f); break;
        case 11: if (f.wt == 2) copy_str(e->preview_url, sizeof e->preview_url, &f); break;
        case 13: if (f.wt == 2 && f.len && !d[D_URL]) copy_str((char *)d + D_URL, 256, &f); break;
        case 14: put64(d, D_HFILE, f.v); break;
        case 15: put64(d, D_HPREVIEW, f.v); break;
        case 16: if (f.wt == 2) copy_str((char *)d + D_TITLE, 129, &f); break;
        case 17: if (f.wt == 2) copy_str((char *)d + D_DESC, 8000, &f); break;
        case 19: put32(d, D_CREATED, f.v); break;
        case 20: put32(d, D_UPDATED, f.v); break;
        case 21: put32(d, D_VISIBILITY, f.v); break;
        case 24: d[D_ACCEPTED] = f.v != 0; break;
        case 28: d[D_BANNED] = f.v != 0; break;
        case 34: put32(d, D_TYPE, f.v); break;
        case 36: e->stat[0] = f.v; break;
        case 37: e->stat[1] = f.v; break;
        case 38: e->stat[2] = f.v; break;
        case 39: e->stat[3] = f.v; break;
        case 40: e->stat[4] = f.v; break;
        case 41: e->stat[5] = f.v; break;
        case 42: e->stat[6] = f.v; break;
        case 27: e->stat[10] = f.v; break;
        case 49: put32(d, D_CHILDREN, f.v); break;
        case 50: e->stat[7] = f.v; break;
        case 56: put32(d, D_ADDED, f.v); break;
        case 58: if (f.wt == 2) { e->metadata = malloc(f.len + 1); if (e->metadata) { memcpy(e->metadata, f.data, f.len); e->metadata[f.len] = 0; } } break;
        case 62: e->stat[8] = f.v; break;
        case 63: e->stat[9] = f.v; break;
        case 51:
            if (f.wt == 2 && e->n_prev < 4) {
                PbR p;
                PbField g;
                Preview *pv = &e->prev[e->n_prev];
                pbr_init(&p, f.data, f.len);
                while (pbr_next(&p, &g)) {
                    if (g.field == 3 && g.wt == 2) copy_str(pv->url, sizeof pv->url, &g);
                    else if (g.field == 5 && g.wt == 2) copy_str(pv->file, sizeof pv->file, &g);
                    else if (g.field == 7) pv->type = (uint32_t)g.v;
                }
                e->n_prev++;
            }
            break;
        case 52:
            if (f.wt == 2 && e->n_tags < MAX_TAGS) {
                PbR p;
                PbField g;
                Tag *t = &e->tags[e->n_tags];
                pbr_init(&p, f.data, f.len);
                while (pbr_next(&p, &g)) {
                    if (g.field == 1 && g.wt == 2) copy_str(t->tag, sizeof t->tag, &g);
                    else if (g.field == 3 && g.wt == 2) copy_str(t->display, sizeof t->display, &g);
                }
                if (!t->display[0]) snprintf(t->display, sizeof t->display, "%s", t->tag);
                if (tags[0]) strncat(tags, ",", sizeof tags - strlen(tags) - 1);
                strncat(tags, t->tag, sizeof tags - strlen(tags) - 1);
                e->n_tags++;
            }
            break;
        case 53:
            if (f.wt == 2 && e->n_children < 32) {
                PbR p;
                PbField g;
                pbr_init(&p, f.data, f.len);
                while (pbr_next(&p, &g)) if (g.field == 1) e->children[e->n_children++] = g.v;
            }
            break;
        case 54:
            if (f.wt == 2 && e->n_kv < MAX_TAGS) {
                PbR p;
                PbField g;
                Kv *kv = &e->kv[e->n_kv];
                pbr_init(&p, f.data, f.len);
                while (pbr_next(&p, &g)) {
                    if (g.field == 1 && g.wt == 2) copy_str(kv->key, sizeof kv->key, &g);
                    else if (g.field == 2 && g.wt == 2) copy_str(kv->value, sizeof kv->value, &g);
                }
                e->n_kv++;
            }
            break;
        case 55:
            if (f.wt == 2) {
                PbR p;
                PbField g;
                pbr_init(&p, f.data, f.len);
                while (pbr_next(&p, &g)) {
                    if (g.field == 1) { uint32_t s = (uint32_t)g.v; memcpy(d + D_SCORE, &s, 4); }
                    else if (g.field == 2) put32(d, D_UP, g.v);
                    else if (g.field == 3) put32(d, D_DOWN, g.v);
                }
            }
            break;
        default: break;
        }
    }
    size_t tl = strlen(tags);
    if (tl > 1024) { tags[1024] = 0; d[D_TRUNC] = 1; }
    snprintf((char *)d + D_TAGS, 1025, "%s", tags);
    remember_ugc_details(e);
}

typedef struct { uint64_t handle; PbW body; const char *method; } Send;

static void entries_free(Query *q) {
    for (int i = 0; i < q->n; i++) free(q->r[i].metadata);
    free(q->r);
    q->r = NULL;
    q->n = 0;
}

static void body_common(PbW *w, const Query *q, int ret_field_base) {
    (void)ret_field_base;
    (void)w;
    (void)q;
}

static void add_flags_query(PbW *w, const Query *q, bool user) {
    if (user) {
        pbw_varint(w, 19, 1);
        pbw_varint(w, 20, 1);
        if (q->kv_tags) pbw_varint(w, 21, 1);
        pbw_varint(w, 22, q->previews);
        pbw_varint(w, 23, q->children);
        if (q->metadata) pbw_varint(w, 28, 1);
        if (q->total_only) pbw_varint(w, 17, 1);
        if (q->only_ids) pbw_varint(w, 18, 1);
        return;
    }
    pbw_varint(w, 17, 1);
    pbw_varint(w, 18, 1);
    if (q->kv_tags) pbw_varint(w, 19, 1);
    pbw_varint(w, 20, q->previews);
    pbw_varint(w, 21, q->children);
    if (q->metadata) pbw_varint(w, 32, 1);
    if (q->total_only) pbw_varint(w, 16, 1);
    if (q->only_ids) pbw_varint(w, 35, 1);
}

static DWORD WINAPI query_thread(LPVOID arg) {
    uint64_t handle = (uint64_t)(uintptr_t)arg;
    Query q;
    AcquireSRWLockExclusive(&g_lock);
    Query *live = find(handle);
    if (live) q = *live;
    ReleaseSRWLockExclusive(&g_lock);
    if (!live) return 0;
    PbW w = {0};
    const char *method;
    uint64_t sid = 0;
    int result = 2, total = 0, n = 0;
    Entry *list = NULL;
    char next[256] = "";
    const Snapshot *snap = shim_snapshot();
    if (q.kind == 2) {
        method = "PublishedFile.GetDetails#1";
        for (int i = 0; i < q.n_ids; i++) pbw_fixed64(&w, 1, q.ids[i]);
        pbw_varint(&w, 2, 1);
        pbw_varint(&w, 4, q.children);
        pbw_varint(&w, 5, q.kv_tags);
        pbw_varint(&w, 6, 1);
        pbw_varint(&w, 3, q.previews);
        pbw_varint(&w, 11, q.metadata);
        pbw_varint(&w, 14, q.consumer_app ? q.consumer_app : app_id());
    } else if (q.kind == 0) {
        method = "PublishedFile.GetUserFiles#1";
        sid = q.account ? (((uint64_t)1 << 56) | ((uint64_t)1 << 52) | ((uint64_t)1 << 32) | q.account) : (snap ? snap->steamid : 0);
        static const char *const types[] = { "myfiles", "", "", "", "", "myfavorites", "mysubscriptions", "", "myfollowedfiles" };
        const char *type = q.list_type < 9 ? types[q.list_type] : "";
        if (!type[0]) { result = 1; goto finish_empty; }
        pbw_fixed64(&w, 1, sid);
        pbw_varint(&w, 2, q.consumer_app ? q.consumer_app : app_id());
        pbw_varint(&w, 4, q.page ? q.page : 1);
        pbw_varint(&w, 5, MAX_RESULTS);
        pbw_str(&w, 6, type);
        add_flags_query(&w, &q, true);
    } else {
        method = "PublishedFile.QueryFiles#1";
        static const uint8_t to_steam[20] = { 0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 9, 13, 14, 15, 16, 17, 18, 21 };
        pbw_varint(&w, 1, q.query_type < 20 ? to_steam[q.query_type] : q.query_type);
        if (q.cursor[0]) pbw_str(&w, 39, q.cursor); else pbw_varint(&w, 2, q.page ? q.page : 1);
        pbw_varint(&w, 3, MAX_RESULTS);
        pbw_varint(&w, 4, q.creator_app);
        pbw_varint(&w, 5, q.consumer_app ? q.consumer_app : app_id());
        for (int i = 0; i < q.n_req; i++) pbw_str(&w, 6, q.req_tags[i]);
        for (int i = 0; i < q.n_excl; i++) pbw_str(&w, 7, q.excl_tags[i]);
        pbw_varint(&w, 8, !q.match_any);
        if (q.search[0]) pbw_str(&w, 11, q.search);
        if (q.trend_days) pbw_varint(&w, 14, q.trend_days);
        add_flags_query(&w, &q, false);
        pbw_varint(&w, 37, 1);
    }
    (void)body_common;
    CmReply r = {0};
    bool ok = shim_cm_service(method, w.p, w.len, WAIT_MS, &r);
    pbw_free(&w);
    if (ok) {
        result = r.eresult;
        list = calloc(MAX_RESULTS, sizeof *list);
        PbR in;
        PbField f;
        pbr_init(&in, r.body, r.len);
        uint32_t details_field = q.kind == 1 ? 2 : q.kind == 0 ? 3 : 1;
        while (list && pbr_next(&in, &f)) {
            if (f.field == 1 && q.kind != 2) total = (int)f.v;
            else if (f.field == 3 && q.kind == 1 && f.wt == 2) copy_str(next, sizeof next, &f);
            else if (f.field == details_field && f.wt == 2 && n < MAX_RESULTS) read_details(f.data, f.len, &list[n++]);
        }
        if (q.kind == 2) total = n;
        shim_cm_reply_free(&r);
    }
finish_empty:
    q.result = result;
    AcquireSRWLockExclusive(&g_lock);
    live = find(handle);
    if (live) {
        entries_free(live);
        live->r = list;
        live->n = list ? n : 0;
        live->total = total;
        live->result = result;
        live->done = true;
        snprintf(live->next_cursor, sizeof live->next_cursor, "%s", next);
        list = NULL;
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (list) { for (int i = 0; i < n; i++) free(list[i].metadata); free(list); }
    return 0;
}

static DWORD WINAPI send_thread(LPVOID arg) {
    uint64_t *a = arg;
    uint64_t call = a[0], handle = a[1];
    free(a);
    query_thread((LPVOID)(uintptr_t)handle);
    uint8_t cb[280] = {0};
    AcquireSRWLockExclusive(&g_lock);
    Query *q = find(handle);
    if (q) {
        int32_t result = q->result;
        uint32_t n = (uint32_t)q->n, total = (uint32_t)q->total;
        memcpy(cb, &handle, 8);
        memcpy(cb + 8, &result, 4);
        memcpy(cb + 12, &n, 4);
        memcpy(cb + 16, &total, 4);
        snprintf((char *)cb + 21, 256, "%s", q->next_cursor);
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (q) shim_callresult_complete(call, CB_QUERY_DONE, cb, sizeof cb);
    else shim_callresult_fail_handle(call);
    return 0;
}

static uint64_t SHIM_TC send_query(void *self, uint64_t h) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    AcquireSRWLockExclusive(&g_lock);
    Query *q = find(h);
    if (q) q->done = false;
    ReleaseSRWLockExclusive(&g_lock);
    uint64_t *a = malloc(16);
    if (!q || !a) { free(a); shim_callresult_fail_handle(call); return call; }
    a[0] = call;
    a[1] = h;
    HANDLE t = CreateThread(NULL, 0, send_thread, a, 0, NULL);
    if (!t) { send_thread(a); return call; }
    CloseHandle(t);
    return call;
}

static uint64_t new_query(int kind) {
    AcquireSRWLockExclusive(&g_lock);
    uint64_t h = g_next++;
    Query *q = &g_q[h % MAX_QUERIES];
    entries_free(q);
    memset(q, 0, sizeof *q);
    q->handle = h;
    q->used = true;
    q->kind = kind;
    ReleaseSRWLockExclusive(&g_lock);
    return h;
}

static uint64_t SHIM_TC create_user(void *self, uint32_t account, uint32_t list, uint32_t matching, uint32_t sort, uint32_t creator, uint32_t consumer, uint32_t page) {
    (void)self;
    if (!creator && !consumer) return UINT64_MAX;
    uint64_t h = new_query(0);
    AcquireSRWLockExclusive(&g_lock);
    Query *q = find(h);
    q->account = account; q->list_type = list; q->matching = matching; q->sort = sort; q->creator_app = creator; q->consumer_app = consumer; q->page = page;
    ReleaseSRWLockExclusive(&g_lock);
    return h;
}

static uint64_t SHIM_TC create_all_page(void *self, uint32_t type, uint32_t matching, uint32_t creator, uint32_t consumer, uint32_t page) {
    (void)self;
    if (!creator && !consumer) return UINT64_MAX;
    uint64_t h = new_query(1);
    AcquireSRWLockExclusive(&g_lock);
    Query *q = find(h);
    q->query_type = type; q->matching = matching; q->creator_app = creator; q->consumer_app = consumer; q->page = page;
    ReleaseSRWLockExclusive(&g_lock);
    return h;
}

static uint64_t SHIM_TC create_all_cursor(void *self, uint32_t type, uint32_t matching, uint32_t creator, uint32_t consumer, const char *cursor) {
    (void)self;
    uint64_t h = create_all_page(self, type, matching, creator, consumer, 1);
    if (h == UINT64_MAX) return h;
    AcquireSRWLockExclusive(&g_lock);
    Query *q = find(h);
    if (cursor && strcmp(cursor, "*")) snprintf(q->cursor, sizeof q->cursor, "%s", cursor);
    ReleaseSRWLockExclusive(&g_lock);
    return h;
}

static uint64_t SHIM_TC create_details(void *self, const uint64_t *ids, uint32_t n) {
    (void)self;
    if (!ids || !n || n > MAX_IDS) return NO_HANDLE;
    uint64_t h = new_query(2);
    AcquireSRWLockExclusive(&g_lock);
    Query *q = find(h);
    memcpy(q->ids, ids, 8 * (size_t)n);
    q->n_ids = (int)n;
    ReleaseSRWLockExclusive(&g_lock);
    return h;
}

static uint64_t SHIM_TC release_query(void *self, uint64_t h) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Query *q = find(h);
    if (q) { entries_free(q); q->used = false; }
    ReleaseSRWLockExclusive(&g_lock);
    return q != NULL;
}

#define SETTER(name, body) static uint64_t SHIM_TC name { (void)self; AcquireSRWLockExclusive(&g_lock); Query *q = find(h); bool ok = q != NULL; if (q) { body; } ReleaseSRWLockExclusive(&g_lock); return ok; }
SETTER(set_only_ids(void *self, uint64_t h, uint8_t v), q->only_ids = v)
SETTER(set_kv_tags(void *self, uint64_t h, uint8_t v), q->kv_tags = v)
SETTER(set_long_desc(void *self, uint64_t h, uint8_t v), q->long_desc = v)
SETTER(set_metadata(void *self, uint64_t h, uint8_t v), q->metadata = v)
SETTER(set_children(void *self, uint64_t h, uint8_t v), q->children = v)
SETTER(set_previews(void *self, uint64_t h, uint8_t v), q->previews = v)
SETTER(set_total_only(void *self, uint64_t h, uint8_t v), q->total_only = v)
SETTER(set_match_any(void *self, uint64_t h, uint8_t v), q->match_any = v)
SETTER(set_playtime(void *self, uint64_t h, uint32_t days), (void)days)
SETTER(set_allow_cached(void *self, uint64_t h, uint32_t age), (void)age)
SETTER(set_trend_days(void *self, uint64_t h, uint32_t days), q->trend_days = days)
SETTER(set_language(void *self, uint64_t h, const char *l), snprintf(q->language, sizeof q->language, "%s", l ? l : ""))
SETTER(set_search(void *self, uint64_t h, const char *t), snprintf(q->search, sizeof q->search, "%s", t ? t : ""))
SETTER(set_filename_filter(void *self, uint64_t h, const char *n), snprintf(q->filename_filter, sizeof q->filename_filter, "%s", n ? n : ""))
SETTER(set_date_range(void *self, uint64_t h, uint32_t a, uint32_t b), (void)a; (void)b)
SETTER(add_required_kv(void *self, uint64_t h, const char *k, const char *v), (void)k; (void)v)
SETTER(add_required_tag(void *self, uint64_t h, const char *t), if (t && q->n_req < MAX_TAGS) snprintf(q->req_tags[q->n_req++], 64, "%s", t); else ok = false)
SETTER(add_excluded_tag(void *self, uint64_t h, const char *t), if (t && q->n_excl < MAX_TAGS) snprintf(q->excl_tags[q->n_excl++], 64, "%s", t); else ok = false)

static uint64_t SHIM_TC add_required_group(void *self, uint64_t h, const void *arr) {
    (void)self;
    if (!arr) return 0;
    const char *const *strings = *(const char *const *const *)arr;
    int32_t n = *(const int32_t *)((const uint8_t *)arr + sizeof(void *));
    AcquireSRWLockExclusive(&g_lock);
    Query *q = find(h);
    bool ok = q != NULL;
    for (int i = 0; q && strings && i < n && q->n_req < MAX_TAGS; i++) if (strings[i]) snprintf(q->req_tags[q->n_req++], 64, "%s", strings[i]);
    if (q) q->match_any = n > 1 ? true : q->match_any;
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static Entry *entry_locked(uint64_t h, uint32_t i) {
    Query *q = find(h);
    return q && q->done && q->r && i < (uint32_t)q->n ? &q->r[i] : NULL;
}

static uint64_t result_copy(uint64_t h, uint32_t i, void *out, size_t size) {
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    Entry *e = entry_locked(h, i);
    if (e && out) { memcpy(out, e->details, size); ok = true; }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}
static uint64_t SHIM_TC query_result_old(void *self, uint64_t h, uint32_t i, void *out) { (void)self; return result_copy(h, i, out, DETAILS_SIZE_OLD); }
static uint64_t SHIM_TC query_result_new(void *self, uint64_t h, uint32_t i, void *out) { (void)self; return result_copy(h, i, out, DETAILS_SIZE_NEW); }

static uint32_t SHIM_TC num_tags(void *self, uint64_t h, uint32_t i) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Entry *e = entry_locked(h, i);
    uint32_t n = e ? (uint32_t)e->n_tags : 0;
    ReleaseSRWLockExclusive(&g_lock);
    return n;
}

static uint64_t give(const char *s, char *out, uint32_t cap) {
    if (!out || !cap) return 0;
    snprintf(out, cap, "%s", s ? s : "");
    return 1;
}

static uint64_t SHIM_TC get_tag(void *self, uint64_t h, uint32_t i, uint32_t t, char *out, uint32_t cap) {
    (void)self;
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    Entry *e = entry_locked(h, i);
    if (e && t < (uint32_t)e->n_tags) ok = give(e->tags[t].tag, out, cap);
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static uint64_t SHIM_TC get_tag_display(void *self, uint64_t h, uint32_t i, uint32_t t, char *out, uint32_t cap) {
    (void)self;
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    Entry *e = entry_locked(h, i);
    if (e && t < (uint32_t)e->n_tags) ok = give(e->tags[t].display, out, cap);
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static uint64_t SHIM_TC get_preview_url(void *self, uint64_t h, uint32_t i, char *out, uint32_t cap) {
    (void)self;
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    Entry *e = entry_locked(h, i);
    if (e) ok = give(e->preview_url, out, cap);
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static uint64_t SHIM_TC get_metadata(void *self, uint64_t h, uint32_t i, char *out, uint32_t cap) {
    (void)self;
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    Entry *e = entry_locked(h, i);
    if (e) ok = give(e->metadata, out, cap);
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static uint64_t SHIM_TC get_children(void *self, uint64_t h, uint32_t i, uint64_t *out, uint32_t max) {
    (void)self;
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    Entry *e = entry_locked(h, i);
    if (e && out) {
        uint32_t n = (uint32_t)e->n_children < max ? (uint32_t)e->n_children : max;
        memcpy(out, e->children, 8 * (size_t)n);
        ok = true;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static uint64_t SHIM_TC get_statistic(void *self, uint64_t h, uint32_t i, uint32_t stat, uint64_t *out) {
    (void)self;
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    Entry *e = entry_locked(h, i);
    if (e && out && stat < 13) { *out = e->stat[stat]; ok = true; }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static uint32_t SHIM_TC num_additional_previews(void *self, uint64_t h, uint32_t i) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Entry *e = entry_locked(h, i);
    uint32_t n = e ? (uint32_t)e->n_prev : 0;
    ReleaseSRWLockExclusive(&g_lock);
    return n;
}

static uint64_t preview_get(uint64_t h, uint32_t i, uint32_t p, char *url, uint32_t cu, char *file, uint32_t cf, uint32_t *type) {
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    Entry *e = entry_locked(h, i);
    if (e && p < (uint32_t)e->n_prev) {
        give(e->prev[p].url, url, cu);
        if (file) give(e->prev[p].file, file, cf);
        if (type) *type = e->prev[p].type;
        ok = true;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}
static uint64_t SHIM_TC get_additional_preview(void *self, uint64_t h, uint32_t i, uint32_t p, char *url, uint32_t cu, char *file, uint32_t cf, uint32_t *type) { (void)self; return preview_get(h, i, p, url, cu, file, cf, type); }
static uint64_t SHIM_TC get_additional_preview_old(void *self, uint64_t h, uint32_t i, uint32_t p, char *url, uint32_t cu, uint32_t *type) { (void)self; return preview_get(h, i, p, url, cu, NULL, 0, type); }

static uint32_t SHIM_TC num_kv_tags(void *self, uint64_t h, uint32_t i) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Entry *e = entry_locked(h, i);
    uint32_t n = e ? (uint32_t)e->n_kv : 0;
    ReleaseSRWLockExclusive(&g_lock);
    return n;
}

static uint64_t SHIM_TC get_kv_by_index(void *self, uint64_t h, uint32_t i, uint32_t k, char *key, uint32_t ck, char *val, uint32_t cv) {
    (void)self;
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    Entry *e = entry_locked(h, i);
    if (e && k < (uint32_t)e->n_kv) { give(e->kv[k].key, key, ck); give(e->kv[k].value, val, cv); ok = true; }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static uint64_t SHIM_TC get_kv_by_key(void *self, uint64_t h, uint32_t i, const char *key, char *val, uint32_t cv) {
    (void)self;
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    Entry *e = entry_locked(h, i);
    for (int k = 0; e && key && k < e->n_kv; k++) if (!strcmp(e->kv[k].key, key)) { give(e->kv[k].value, val, cv); ok = true; break; }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

typedef struct { uint64_t call, id; } DetailsJob;
static DWORD WINAPI request_details_thread(LPVOID arg) {
    DetailsJob *j = arg;
    PbW w = {0};
    pbw_fixed64(&w, 1, j->id);
    pbw_varint(&w, 2, 1);
    CmReply r = {0};
    bool ok = shim_cm_service("PublishedFile.GetDetails#1", w.p, w.len, WAIT_MS, &r);
    pbw_free(&w);
    uint8_t *cb = calloc(1, DETAILS_SIZE_OLD + 8);
    Entry *e = calloc(1, sizeof *e);
    bool filled = false;
    if (ok && cb && e) {
        PbR in;
        PbField f;
        pbr_init(&in, r.body, r.len);
        while (pbr_next(&in, &f)) if (f.field == 1 && f.wt == 2) { read_details(f.data, f.len, e); filled = true; break; }
    }
    if (filled) { memcpy(cb, e->details, DETAILS_SIZE_OLD); shim_callresult_complete(j->call, CB_DETAILS_RESULT, cb, DETAILS_SIZE_OLD + 8); }
    else shim_callresult_fail_handle(j->call);
    if (e) free(e->metadata);
    free(e);
    free(cb);
    shim_cm_reply_free(&r);
    free(j);
    return 0;
}
static uint64_t SHIM_TC request_details(void *self, uint64_t id, uint32_t max_age) {
    (void)self;
    (void)max_age;
    uint64_t call = shim_callresult_reserve();
    DetailsJob *j = calloc(1, sizeof *j);
    if (!j) { shim_callresult_fail_handle(call); return call; }
    j->call = call;
    j->id = id;
    HANDLE t = CreateThread(NULL, 0, request_details_thread, j, 0, NULL);
    if (!t) { request_details_thread(j); return call; }
    CloseHandle(t);
    return call;
}

enum {
    CB_RS_ENUM_USER = 1312, CB_RS_DOWNLOAD = 1317, CB_RS_DETAILS = 1318, CB_RS_ENUM_WORKSHOP = 1319, CB_RS_VOTE_DETAILS = 1320,
    CB_RS_UPDATE_VOTE = 1324, CB_RS_USER_VOTE = 1325, CB_RS_ENUM_SHARED = 1326, CB_RS_SET_ACTION = 1327, CB_RS_ENUM_ACTION = 1328,
    CB_RS_PUBLISH = 1309, RS_PAGE = 50, RS_FILES = 128, RS_ACTIONS = 256,
    R_TITLE = 24, R_DESC = 153, R_HFILE = 8160, R_HPREVIEW = 8168, R_OWNER = 8176, R_CREATED = 8184, R_UPDATED = 8188, R_VISIBILITY = 8192,
    R_BANNED = 8196, R_TAGS = 8197, R_TRUNC = 9222, R_FILENAME = 9223, R_FSIZE = 9484, R_PSIZE = 9488, R_URL = 9492, R_TYPE = 9748,
    R_ACCEPTED = 9752, R_DETAILS_SIZE = 9760,
};

typedef struct {
    bool used;
    uint64_t handle, owner;
    uint32_t app;
    int32_t size;
    char name[260], url[512];
    uint8_t *data;
    size_t len;
    volatile LONG got;
    bool downloaded;
    uint64_t cursor;
} UgcFile;

static UgcFile g_files[RS_FILES];
static unsigned g_files_next;
static SRWLOCK g_files_lock = SRWLOCK_INIT;

static UgcFile *file_for(uint64_t handle) {
    for (int i = 0; i < RS_FILES; i++) if (g_files[i].used && g_files[i].handle == handle) return &g_files[i];
    return NULL;
}

static void remember_ugc(uint64_t handle, uint32_t app, uint64_t owner, int32_t size, const char *name, const char *url) {
    if (!handle || handle == NO_HANDLE || !url || !url[0]) return;
    AcquireSRWLockExclusive(&g_files_lock);
    UgcFile *f = file_for(handle);
    if (!f) {
        f = &g_files[g_files_next++ % RS_FILES];
        free(f->data);
        memset(f, 0, sizeof *f);
        f->used = true;
        f->handle = handle;
    }
    f->app = app;
    f->owner = owner;
    f->size = size;
    snprintf(f->name, sizeof f->name, "%s", name ? name : "");
    snprintf(f->url, sizeof f->url, "%s", url);
    ReleaseSRWLockExclusive(&g_files_lock);
}

static void remember_ugc_details(const Entry *e) {
    const uint8_t *d = e->details;
    uint64_t hfile, hpreview, owner;
    uint32_t app;
    int32_t fsize, psize;
    memcpy(&hfile, d + D_HFILE, 8);
    memcpy(&hpreview, d + D_HPREVIEW, 8);
    memcpy(&owner, d + D_OWNER, 8);
    memcpy(&app, d + D_CONSUMER_APP, 4);
    memcpy(&fsize, d + D_FSIZE, 4);
    memcpy(&psize, d + D_PSIZE, 4);
    remember_ugc(hfile, app, owner, fsize, (const char *)d + D_FILENAME, (const char *)d + D_URL);
    remember_ugc(hpreview, app, owner, psize, "preview", e->preview_url);
}

static wchar_t *to_wide(const char *s) {
    int n = MultiByteToWideChar(CP_UTF8, 0, s, -1, NULL, 0);
    wchar_t *w = n > 0 ? malloc((size_t)n * sizeof *w) : NULL;
    if (w) MultiByteToWideChar(CP_UTF8, 0, s, -1, w, n);
    return w;
}

static bool http_get(const char *url, uint8_t **out, size_t *out_len, volatile LONG *progress) {
    bool ok = false;
    wchar_t *wurl = to_wide(url);
    if (!wurl) return false;
    wchar_t host[256] = L"", path[2048] = L"";
    URL_COMPONENTSW uc = {0};
    uc.dwStructSize = sizeof uc;
    uc.lpszHostName = host; uc.dwHostNameLength = 256;
    uc.lpszUrlPath = path; uc.dwUrlPathLength = 2048;
    uc.dwExtraInfoLength = (DWORD)-1;
    HINTERNET s = NULL, c = NULL, r = NULL;
    uint8_t *buf = NULL;
    size_t len = 0, cap = 0;
    if (WinHttpCrackUrl(wurl, 0, 0, &uc)) {
        wchar_t full[2560];
        wcsncpy(full, path, 2047);
        full[2047] = 0;
        if (uc.lpszExtraInfo && uc.dwExtraInfoLength) wcsncat(full, uc.lpszExtraInfo, 500);
        s = WinHttpOpen(L"Valve/Steam HTTP Client 1.0", WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, NULL, NULL, 0);
        c = s ? WinHttpConnect(s, host, uc.nPort, 0) : NULL;
        r = c ? WinHttpOpenRequest(c, L"GET", full, NULL, NULL, NULL, uc.nScheme == INTERNET_SCHEME_HTTPS ? WINHTTP_FLAG_SECURE : 0) : NULL;
        if (r && WinHttpSendRequest(r, NULL, 0, NULL, 0, 0, 0) && WinHttpReceiveResponse(r, NULL)) {
            DWORD status = 0, sz = sizeof status;
            WinHttpQueryHeaders(r, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, NULL, &status, &sz, NULL);
            ok = status >= 200 && status < 300;
            DWORD avail;
            while (ok && WinHttpQueryDataAvailable(r, &avail) && avail) {
                if (len + avail + 1 > cap) {
                    cap = (len + avail + 1) * 2;
                    uint8_t *nb = realloc(buf, cap);
                    if (!nb) { ok = false; break; }
                    buf = nb;
                }
                DWORD got = 0;
                if (!WinHttpReadData(r, buf + len, avail, &got) || !got) { ok = false; break; }
                len += got;
                if (progress) InterlockedExchange(progress, (LONG)len);
            }
        }
    }
    if (r) WinHttpCloseHandle(r);
    if (c) WinHttpCloseHandle(c);
    if (s) WinHttpCloseHandle(s);
    free(wurl);
    if (!ok) { free(buf); return false; }
    *out = buf;
    *out_len = len;
    return true;
}

enum { L_USER, L_WORKSHOP, L_SHARED, L_VOTES, L_USERVOTE, L_UPDVOTE, L_DETAILS, L_VIDEO };
typedef struct { uint32_t kind, skip, count, start, app; uint64_t id; } Leg;

static Leg *leg(uint32_t kind) { Leg *l = calloc(1, sizeof *l); if (l) l->kind = kind; return l; }

static void rs_put32(uint8_t *d, int off, uint32_t v) { memcpy(d + off, &v, 4); }
static void rs_put64(uint8_t *d, int off, uint64_t v) { memcpy(d + off, &v, 8); }

static bool parse_list(const CmReply *r, void *ctx, void *out, int *size) {
    Leg *l = ctx;
    uint8_t *o = out;
    uint64_t ids[RS_PAGE] = {0};
    uint32_t score[RS_PAGE] = {0};
    uint32_t total = 0, seen = 0, n = 0;
    PbR in;
    PbField f;
    uint32_t details_field = l->kind == L_WORKSHOP ? 2 : 3;
    if (*size < 624) return false;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field == 1 && f.wt == 0) total = (uint32_t)f.v;
        else if (f.field == details_field && f.wt == 2) {
            PbR p;
            PbField g;
            uint64_t id = 0;
            uint32_t sc = 0;
            pbr_init(&p, f.data, f.len);
            while (pbr_next(&p, &g)) {
                if (g.field == 2 && g.wt == 0) id = g.v;
                else if (g.field == 55 && g.wt == 2) {
                    PbR v;
                    PbField h;
                    pbr_init(&v, g.data, g.len);
                    while (pbr_next(&v, &h)) if (h.field == 1) sc = (uint32_t)h.v;
                }
            }
            if (seen++ < l->skip || n >= l->count || n >= RS_PAGE) continue;
            ids[n] = id; score[n] = sc; n++;
        }
    }
    if (in.bad && !n) return false;
    memset(o, 0, 624);
    rs_put32(o, 0, (uint32_t)r->eresult);
    rs_put32(o, 4, n);
    rs_put32(o, 8, total > n ? total : n);
    for (uint32_t i = 0; i < n; i++) rs_put64(o, 16 + 8 * (int)i, ids[i]);
    if (l->kind == L_WORKSHOP) {
        for (uint32_t i = 0; i < n; i++) rs_put32(o, 416 + 4 * (int)i, score[i]);
        rs_put32(o, 616, l->app);
        rs_put32(o, 620, l->start);
        *size = 624;
    } else {
        *size = 416;
    }
    return true;
}

static uint64_t finish_user_list(uint32_t kind, uint64_t sid, uint32_t start, const ParamStringArray *req, const ParamStringArray *excl, int callback) {
    uint64_t call = shim_callresult_reserve();
    Leg *l = leg(kind);
    if (!l) { shim_callresult_fail_handle(call); return call; }
    l->count = RS_PAGE; l->start = start; l->skip = 0;
    PbW w = {0};
    pbw_fixed64(&w, 1, sid);
    pbw_varint(&w, 2, app_id());
    pbw_varint(&w, 4, 1);
    pbw_varint(&w, 5, RS_PAGE);
    pbw_str(&w, 6, "myfiles");
    for (int i = 0; req && req->strings && i < req->n; i++) if (req->strings[i]) pbw_str(&w, 10, req->strings[i]);
    for (int i = 0; excl && excl->strings && i < excl->n; i++) if (excl->strings[i]) pbw_str(&w, 11, excl->strings[i]);
    pbw_varint(&w, 25, start);
    pbw_varint(&w, 19, 1);
    shim_cm_async(call, callback, 0, true, "PublishedFile.GetUserFiles#1", &w, WAIT_MS, parse_list, l);
    return call;
}

static uint64_t SHIM_TC rs_enum_user(void *self, uint32_t start) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    return finish_user_list(L_USER, s ? s->steamid : 0, start, NULL, NULL, CB_RS_ENUM_USER);
}

static uint64_t SHIM_TC rs_enum_shared(void *self, uint64_t steamid, uint32_t start, const ParamStringArray *req, const ParamStringArray *excl) {
    (void)self;
    return finish_user_list(L_SHARED, steamid, start, req, excl, CB_RS_ENUM_SHARED);
}

static uint64_t SHIM_TC rs_enum_workshop(void *self, uint32_t type, uint32_t start, uint32_t count, uint32_t days, const ParamStringArray *tags, const ParamStringArray *user_tags) {
    (void)self;
    static const uint8_t to_query[7] = { 0, 1, 3, 4, 5, 5, 7 };
    uint64_t call = shim_callresult_reserve();
    Leg *l = leg(L_WORKSHOP);
    if (!l) { shim_callresult_fail_handle(call); return call; }
    if (!count || count > RS_PAGE) count = RS_PAGE;
    l->count = count; l->start = start; l->app = app_id();
    uint32_t page = start / count + 1, per = count;
    if (start % count) { page = 1; per = start + count > 100 ? 100 : start + count; l->skip = start; }
    PbW w = {0};
    pbw_varint(&w, 1, type < 7 ? to_query[type] : 0);
    pbw_varint(&w, 2, page);
    pbw_varint(&w, 3, per);
    pbw_varint(&w, 5, app_id());
    for (int i = 0; tags && tags->strings && i < tags->n; i++) if (tags->strings[i]) pbw_str(&w, 6, tags->strings[i]);
    for (int i = 0; user_tags && user_tags->strings && i < user_tags->n; i++) if (user_tags->strings[i]) pbw_str(&w, 6, user_tags->strings[i]);
    pbw_varint(&w, 8, 1);
    if (type == 2 && days) pbw_varint(&w, 14, days);
    pbw_varint(&w, 17, 1);
    pbw_varint(&w, 37, 1);
    shim_cm_async(call, CB_RS_ENUM_WORKSHOP, 0, true, "PublishedFile.QueryFiles#1", &w, WAIT_MS, parse_list, l);
    return call;
}

static struct { uint64_t id; uint32_t action, when; } g_acts[RS_ACTIONS];
static unsigned g_n_acts;
static SRWLOCK g_acts_lock = SRWLOCK_INIT;

static uint64_t SHIM_TC rs_set_action(void *self, uint64_t id, uint32_t action) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    AcquireSRWLockExclusive(&g_acts_lock);
    unsigned i;
    for (i = 0; i < g_n_acts; i++) if (g_acts[i].id == id && g_acts[i].action == action) break;
    if (i == g_n_acts && g_n_acts < RS_ACTIONS) g_n_acts++;
    if (i < RS_ACTIONS) { g_acts[i].id = id; g_acts[i].action = action; g_acts[i].when = (uint32_t)time(NULL); }
    ReleaseSRWLockExclusive(&g_acts_lock);
    uint8_t cb[24] = {0};
    rs_put32(cb, 0, 1);
    rs_put64(cb, 8, id);
    rs_put32(cb, 16, action);
    shim_callresult_complete(call, CB_RS_SET_ACTION, cb, sizeof cb);
    return call;
}

static uint64_t SHIM_TC rs_enum_by_action(void *self, uint32_t action, uint32_t start) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    uint8_t cb[616] = {0};
    uint32_t n = 0, total = 0;
    AcquireSRWLockShared(&g_acts_lock);
    for (unsigned i = 0; i < g_n_acts; i++) {
        if (g_acts[i].action != action) continue;
        if (total++ < start || n >= RS_PAGE) continue;
        rs_put64(cb, 16 + 8 * (int)n, g_acts[i].id);
        rs_put32(cb, 416 + 4 * (int)n, g_acts[i].when);
        n++;
    }
    ReleaseSRWLockShared(&g_acts_lock);
    rs_put32(cb, 0, 1);
    rs_put32(cb, 4, action);
    rs_put32(cb, 8, n);
    rs_put32(cb, 12, total);
    shim_callresult_complete(call, CB_RS_ENUM_ACTION, cb, sizeof cb);
    return call;
}

static bool first_details(const CmReply *r, Entry *e) {
    PbR in;
    PbField f;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) if (f.field == 1 && f.wt == 2) { read_details(f.data, f.len, e); return true; }
    return false;
}

static void copy_cstr(uint8_t *dst, size_t cap, const uint8_t *src) {
    size_t n = strnlen((const char *)src, cap - 1);
    memcpy(dst, src, n);
    dst[n] = 0;
}

static bool parse_rs_details(const CmReply *r, void *ctx, void *out, int *size) {
    (void)ctx;
    Entry *e = calloc(1, sizeof *e);
    bool ok = false;
    if (e && *size >= R_DETAILS_SIZE && first_details(r, e)) {
        const uint8_t *d = e->details;
        uint8_t *o = out;
        uint32_t result = 0;
        memcpy(&result, d + D_RESULT, 4);
        memset(o, 0, R_DETAILS_SIZE);
        rs_put32(o, 0, result ? result : (uint32_t)r->eresult);
        memcpy(o + 8, d + D_FILEID, 8);
        memcpy(o + 16, d + D_CREATOR_APP, 4);
        memcpy(o + 20, d + D_CONSUMER_APP, 4);
        copy_cstr(o + R_TITLE, 129, d + D_TITLE);
        copy_cstr(o + R_DESC, 8000, d + D_DESC);
        memcpy(o + R_HFILE, d + D_HFILE, 8);
        memcpy(o + R_HPREVIEW, d + D_HPREVIEW, 8);
        memcpy(o + R_OWNER, d + D_OWNER, 8);
        memcpy(o + R_CREATED, d + D_CREATED, 4);
        memcpy(o + R_UPDATED, d + D_UPDATED, 4);
        memcpy(o + R_VISIBILITY, d + D_VISIBILITY, 4);
        o[R_BANNED] = d[D_BANNED];
        copy_cstr(o + R_TAGS, 1025, d + D_TAGS);
        o[R_TRUNC] = d[D_TRUNC];
        copy_cstr(o + R_FILENAME, 260, d + D_FILENAME);
        memcpy(o + R_FSIZE, d + D_FSIZE, 4);
        memcpy(o + R_PSIZE, d + D_PSIZE, 4);
        copy_cstr(o + R_URL, 256, d + D_URL);
        memcpy(o + R_TYPE, d + D_TYPE, 4);
        o[R_ACCEPTED] = d[D_ACCEPTED];
        *size = R_DETAILS_SIZE;
        ok = true;
    }
    if (e) free(e->metadata);
    free(e);
    return ok;
}

static uint64_t rs_details(uint64_t id) {
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_fixed64(&w, 1, id);
    pbw_varint(&w, 2, 1);
    pbw_varint(&w, 6, 1);
    pbw_varint(&w, 14, app_id());
    shim_cm_async(call, CB_RS_DETAILS, 0, true, "PublishedFile.GetDetails#1", &w, WAIT_MS, parse_rs_details, NULL);
    return call;
}
static uint64_t SHIM_TC rs_details_old(void *self, uint64_t id) { (void)self; return rs_details(id); }
static uint64_t SHIM_TC rs_details_new(void *self, uint64_t id, uint32_t max_age) { (void)self; (void)max_age; return rs_details(id); }

static bool parse_vote_details(const CmReply *r, void *ctx, void *out, int *size) {
    Leg *l = ctx;
    Entry *e = calloc(1, sizeof *e);
    bool ok = false;
    if (e && *size >= 32 && first_details(r, e)) {
        uint8_t *o = out;
        memset(o, 0, 32);
        rs_put32(o, 0, (uint32_t)r->eresult);
        rs_put64(o, 8, l->id);
        memcpy(o + 16, e->details + D_UP, 4);
        memcpy(o + 20, e->details + D_DOWN, 4);
        rs_put32(o, 24, (uint32_t)e->stat[7]);
        memcpy(o + 28, e->details + D_SCORE, 4);
        *size = 32;
        ok = true;
    }
    if (e) free(e->metadata);
    free(e);
    return ok;
}

static uint64_t SHIM_TC rs_vote_details(void *self, uint64_t id) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    Leg *l = leg(L_VOTES);
    if (!l) { shim_callresult_fail_handle(call); return call; }
    l->id = id;
    PbW w = {0};
    pbw_fixed64(&w, 1, id);
    pbw_varint(&w, 6, 1);
    pbw_varint(&w, 14, app_id());
    shim_cm_async(call, CB_RS_VOTE_DETAILS, 0, true, "PublishedFile.GetDetails#1", &w, WAIT_MS, parse_vote_details, l);
    return call;
}

static bool parse_user_vote(const CmReply *r, void *ctx, void *out, int *size) {
    Leg *l = ctx;
    PbR in;
    PbField f;
    uint32_t vote = 0;
    if (*size < 24) return false;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field != 1 || f.wt != 2) continue;
        PbR p;
        PbField g;
        uint64_t id = 0;
        bool up = false, down = false;
        pbr_init(&p, f.data, f.len);
        while (pbr_next(&p, &g)) {
            if (g.field == 1) id = g.v;
            else if (g.field == 2) up = g.v != 0;
            else if (g.field == 3) down = g.v != 0;
        }
        if (!id || id == l->id) vote = up ? 1 : down ? 2 : 0;
    }
    uint8_t *o = out;
    memset(o, 0, 24);
    rs_put32(o, 0, (uint32_t)r->eresult);
    rs_put64(o, 8, l->id);
    rs_put32(o, 16, vote);
    *size = 24;
    return true;
}

static uint64_t SHIM_TC rs_user_vote(void *self, uint64_t id) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    Leg *l = leg(L_USERVOTE);
    if (!l) { shim_callresult_fail_handle(call); return call; }
    l->id = id;
    PbW w = {0};
    pbw_fixed64(&w, 1, id);
    shim_cm_async(call, CB_RS_USER_VOTE, 0, true, "PublishedFile.GetUserVoteSummary#1", &w, WAIT_MS, parse_user_vote, l);
    return call;
}

static bool parse_update_vote(const CmReply *r, void *ctx, void *out, int *size) {
    Leg *l = ctx;
    if (*size < 16) return false;
    uint8_t *o = out;
    memset(o, 0, 16);
    rs_put32(o, 0, (uint32_t)r->eresult);
    rs_put64(o, 8, l->id);
    *size = 16;
    return true;
}

static uint64_t SHIM_TC rs_update_vote(void *self, uint64_t id, uint32_t up) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    Leg *l = leg(L_UPDVOTE);
    if (!l) { shim_callresult_fail_handle(call); return call; }
    l->id = id;
    PbW w = {0};
    pbw_varint(&w, 1, id);
    pbw_varint(&w, 2, (up & 0xff) != 0);
    shim_cm_async(call, CB_RS_UPDATE_VOTE, 0, true, "PublishedFile.Vote#1", &w, WAIT_MS, parse_update_vote, l);
    return call;
}

static bool parse_video(const CmReply *r, void *ctx, void *out, int *size) {
    (void)ctx;
    PbR in;
    PbField f;
    uint64_t id = 0;
    if (*size < 24) return false;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) if (f.field == 1 && f.wt == 0) id = f.v;
    uint8_t *o = out;
    memset(o, 0, 24);
    rs_put32(o, 0, (uint32_t)r->eresult);
    rs_put64(o, 8, id);
    *size = 24;
    return true;
}

static uint64_t SHIM_TC rs_publish_video(void *self, uint32_t provider, const char *account, const char *video, const char *preview, uint32_t consumer,
                                         const char *title, const char *description, uint32_t visibility, const ParamStringArray *tags);
static uint64_t SHIM_TC rs_publish_video_v6(void *self, uint32_t provider, const char *account, const char *video, const char *preview, uint32_t consumer,
                                            const char *title, const char *description) {
    return rs_publish_video(self, provider, account, video, preview, consumer, title, description, 0, NULL);
}

static uint64_t SHIM_TC rs_publish_video(void *self, uint32_t provider, const char *account, const char *video, const char *preview, uint32_t consumer,
                                         const char *title, const char *description, uint32_t visibility, const ParamStringArray *tags) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    if (provider != 1 || !video || !*video) {
        uint8_t cb[24] = {0};
        rs_put32(cb, 0, 8);
        shim_callresult_complete(call, CB_RS_PUBLISH, cb, sizeof cb);
        return call;
    }
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_varint(&w, 2, consumer ? consumer : app_id());
    if (preview && *preview) pbw_str(&w, 4, preview);
    pbw_str(&w, 5, title ? title : "");
    pbw_str(&w, 6, description ? description : "");
    pbw_varint(&w, 7, 4);
    if (account && *account) pbw_str(&w, 9, account);
    pbw_str(&w, 10, video);
    pbw_varint(&w, 11, visibility);
    for (int i = 0; tags && tags->strings && i < tags->n; i++) if (tags->strings[i]) pbw_str(&w, 13, tags->strings[i]);
    shim_cm_async(call, CB_RS_PUBLISH, 0, true, "PublishedFile.Publish#1", &w, WAIT_MS, parse_video, NULL);
    return call;
}

typedef struct { uint64_t call, handle; char location[MAX_PATH]; } DlJob;

static DWORD WINAPI download_job(LPVOID arg) {
    DlJob *j = arg;
    int32_t result = 9;
    char url[512] = "", name[260] = "";
    uint32_t app = 0;
    uint64_t owner = 0;
    int32_t size = 0;
    AcquireSRWLockExclusive(&g_files_lock);
    UgcFile *f = file_for(j->handle);
    if (f) {
        snprintf(url, sizeof url, "%s", f->url);
        snprintf(name, sizeof name, "%s", f->name);
        app = f->app; owner = f->owner; size = f->size;
        f->got = 0;
        if (f->downloaded) result = 1;
    }
    ReleaseSRWLockExclusive(&g_files_lock);
    if (f && result != 1) {
        uint8_t *data = NULL;
        size_t len = 0;
        result = 2;
        if (http_get(url, &data, &len, &f->got)) {
            AcquireSRWLockExclusive(&g_files_lock);
            f = file_for(j->handle);
            if (f) { free(f->data); f->data = data; f->len = len; f->size = (int32_t)len; f->got = (LONG)len; f->downloaded = true; f->cursor = 0; size = (int32_t)len; result = 1; }
            else free(data);
            ReleaseSRWLockExclusive(&g_files_lock);
        }
    }
    if (result == 1 && j->location[0]) {
        FILE *out = fopen(j->location, "wb");
        AcquireSRWLockShared(&g_files_lock);
        f = file_for(j->handle);
        if (out && f && f->data) fwrite(f->data, 1, f->len, out);
        else result = 2;
        ReleaseSRWLockShared(&g_files_lock);
        if (out) fclose(out);
        snprintf(name, sizeof name, "%s", j->location);
    }
    uint8_t cb[296] = {0};
    rs_put32(cb, 0, (uint32_t)result);
    rs_put64(cb, 8, j->handle);
    rs_put32(cb, 16, app);
    rs_put32(cb, 20, (uint32_t)size);
    snprintf((char *)cb + 24, 260, "%s", name);
    rs_put64(cb, 288, owner);
    shim_callresult_complete(j->call, CB_RS_DOWNLOAD, cb, sizeof cb);
    free(j);
    return 0;
}

static uint64_t start_download(uint64_t handle, const char *location) {
    uint64_t call = shim_callresult_reserve();
    DlJob *j = calloc(1, sizeof *j);
    if (!j) { shim_callresult_fail_handle(call); return call; }
    j->call = call;
    j->handle = handle;
    if (location) snprintf(j->location, sizeof j->location, "%s", location);
    HANDLE t = CreateThread(NULL, 0, download_job, j, 0, NULL);
    if (!t) { download_job(j); return call; }
    CloseHandle(t);
    return call;
}

static uint64_t SHIM_TC rs_download_old(void *self, uint64_t handle) { (void)self; return start_download(handle, NULL); }
static uint64_t SHIM_TC rs_download(void *self, uint64_t handle, uint32_t priority) { (void)self; (void)priority; return start_download(handle, NULL); }
static uint64_t SHIM_TC rs_download_to(void *self, uint64_t handle, const char *location, uint32_t priority) {
    (void)self; (void)priority;
    return start_download(handle, location && *location ? location : NULL);
}

static uint64_t SHIM_TC rs_download_progress(void *self, uint64_t handle, int32_t *got, int32_t *expected) {
    (void)self;
    bool ok = false;
    AcquireSRWLockShared(&g_files_lock);
    UgcFile *f = file_for(handle);
    if (f) { if (got) *got = f->got; if (expected) *expected = f->size; ok = true; }
    ReleaseSRWLockShared(&g_files_lock);
    if (!ok) { if (got) *got = 0; if (expected) *expected = 0; }
    return ok;
}

static uint64_t SHIM_TC rs_ugc_details(void *self, uint64_t handle, uint32_t *app, char **name, int32_t *size, uint64_t *owner) {
    (void)self;
    bool ok = false;
    AcquireSRWLockShared(&g_files_lock);
    UgcFile *f = file_for(handle);
    if (f) {
        if (app) *app = f->app;
        if (name) *name = f->name;
        if (size) *size = f->size;
        if (owner) *owner = f->owner;
        ok = true;
    }
    ReleaseSRWLockShared(&g_files_lock);
    return ok;
}

static int32_t read_from(uint64_t handle, void *dest, int32_t cb, bool use_offset, uint32_t offset, uint32_t action) {
    int32_t n = 0;
    if (!dest || cb <= 0) return 0;
    AcquireSRWLockExclusive(&g_files_lock);
    UgcFile *f = file_for(handle);
    if (f && f->data) {
        uint64_t pos = use_offset ? offset : f->cursor;
        if (pos < f->len) {
            size_t left = f->len - (size_t)pos;
            n = (int32_t)(left < (size_t)cb ? left : (size_t)cb);
            memcpy(dest, f->data + pos, (size_t)n);
        }
        f->cursor = pos + (uint64_t)n;
        if (action == 2) { free(f->data); f->data = NULL; f->len = 0; f->downloaded = false; f->cursor = 0; }
    }
    ReleaseSRWLockExclusive(&g_files_lock);
    return n;
}

static uint64_t SHIM_TC rs_read_seq(void *self, uint64_t handle, void *dest, int32_t cb) { (void)self; return (uint32_t)read_from(handle, dest, cb, false, 0, 0); }
static uint64_t SHIM_TC rs_read_off(void *self, uint64_t handle, void *dest, int32_t cb, uint32_t offset) { (void)self; return (uint32_t)read_from(handle, dest, cb, true, offset, 0); }
static uint64_t SHIM_TC rs_read_act(void *self, uint64_t handle, void *dest, int32_t cb, uint32_t offset, uint32_t action) { (void)self; return (uint32_t)read_from(handle, dest, cb, true, offset, action); }

static void register_remoteworkshop(void) {
    const char *p = "STEAMREMOTESTORAGE_INTERFACE_VERSION";
    shim_register_family(p, "EnumerateUserPublishedFiles", (void *)rs_enum_user);
    shim_register_family(p, "EnumerateUserSharedWorkshopFiles", (void *)rs_enum_shared);
    shim_register_family(p, "EnumeratePublishedWorkshopFiles", (void *)rs_enum_workshop);
    shim_register_family(p, "EnumeratePublishedFilesByUserAction", (void *)rs_enum_by_action);
    shim_register_family(p, "SetUserPublishedFileAction", (void *)rs_set_action);
    shim_register_family_ab(p, "GetPublishedFileDetails", 8, (void *)rs_details_old);
    shim_register_family_ab(p, "GetPublishedFileDetails", 12, (void *)rs_details_new);
    shim_register_family(p, "GetPublishedItemVoteDetails", (void *)rs_vote_details);
    shim_register_family(p, "GetUserPublishedItemVoteDetails", (void *)rs_user_vote);
    shim_register_family(p, "UpdateUserPublishedItemVote", (void *)rs_update_vote);
    shim_register_family_ab(p, "PublishVideo", 36, (void *)rs_publish_video);
    shim_register_family_ab(p, "PublishVideo", 28, (void *)rs_publish_video_v6);
    shim_register_family_ab(p, "UGCDownload", 8, (void *)rs_download_old);
    shim_register_family_ab(p, "UGCDownload", 12, (void *)rs_download);
    shim_register_family(p, "UGCDownloadToLocation", (void *)rs_download_to);
    shim_register_family(p, "GetUGCDownloadProgress", (void *)rs_download_progress);
    shim_register_family(p, "GetUGCDetails", (void *)rs_ugc_details);
    shim_register_family_ab(p, "UGCRead", 16, (void *)rs_read_seq);
    shim_register_family_ab(p, "UGCRead", 20, (void *)rs_read_off);
    shim_register_family_ab(p, "UGCRead", 24, (void *)rs_read_act);
}

void register_ugcquery(void) {
    register_remoteworkshop();
    const char *p = "STEAMUGC_INTERFACE_VERSION";
    shim_register_family(p, "CreateQueryUserUGCRequest", (void *)create_user);
    shim_register_family_ab(p, "CreateQueryAllUGCRequest", 20, (void *)create_all_page);
    shim_register_family(p, "CreateQueryAllUGCRequest_2", (void *)create_all_cursor);
    shim_register_family(p, "CreateQueryUGCDetailsRequest", (void *)create_details);
    shim_register_family(p, "SendQueryUGCRequest", (void *)send_query);
    shim_register_family(p, "ReleaseQueryUGCRequest", (void *)release_query);
    shim_register_family(p, "SetReturnOnlyIDs", (void *)set_only_ids);
    shim_register_family(p, "SetReturnKeyValueTags", (void *)set_kv_tags);
    shim_register_family(p, "SetReturnLongDescription", (void *)set_long_desc);
    shim_register_family(p, "SetReturnMetadata", (void *)set_metadata);
    shim_register_family(p, "SetReturnChildren", (void *)set_children);
    shim_register_family(p, "SetReturnAdditionalPreviews", (void *)set_previews);
    shim_register_family(p, "SetReturnTotalOnly", (void *)set_total_only);
    shim_register_family(p, "SetReturnPlaytimeStats", (void *)set_playtime);
    shim_register_family(p, "SetMatchAnyTag", (void *)set_match_any);
    shim_register_family(p, "SetLanguage", (void *)set_language);
    shim_register_family(p, "SetAllowCachedResponse", (void *)set_allow_cached);
    shim_register_family(p, "SetCloudFileNameFilter", (void *)set_filename_filter);
    shim_register_family(p, "SetSearchText", (void *)set_search);
    shim_register_family(p, "SetRankedByTrendDays", (void *)set_trend_days);
    shim_register_family(p, "SetTimeCreatedDateRange", (void *)set_date_range);
    shim_register_family(p, "SetTimeUpdatedDateRange", (void *)set_date_range);
    shim_register_family(p, "AddRequiredKeyValueTag", (void *)add_required_kv);
    shim_register_family(p, "AddRequiredTag", (void *)add_required_tag);
    shim_register_family(p, "AddRequiredTagGroup", (void *)add_required_group);
    shim_register_family(p, "AddExcludedTag", (void *)add_excluded_tag);
    shim_register_family(p, "GetQueryUGCNumTags", (void *)num_tags);
    shim_register_family(p, "GetQueryUGCTag", (void *)get_tag);
    shim_register_family(p, "GetQueryUGCTagDisplayName", (void *)get_tag_display);
    shim_register_family(p, "GetQueryUGCPreviewURL", (void *)get_preview_url);
    shim_register_family(p, "GetQueryUGCMetadata", (void *)get_metadata);
    shim_register_family(p, "GetQueryUGCChildren", (void *)get_children);
    shim_register_family(p, "GetQueryUGCStatistic", (void *)get_statistic);
    shim_register_family(p, "GetQueryUGCNumAdditionalPreviews", (void *)num_additional_previews);
    shim_register_family_ab(p, "GetQueryUGCAdditionalPreview", 36, (void *)get_additional_preview);
    shim_register_family_ab(p, "GetQueryUGCAdditionalPreview", 28, (void *)get_additional_preview_old);
    shim_register_family(p, "GetQueryUGCNumKeyValueTags", (void *)num_kv_tags);
    shim_register_family_ab(p, "GetQueryUGCKeyValueTag", 32, (void *)get_kv_by_index);
    shim_register_family(p, "GetQueryUGCKeyValueTag_2", (void *)get_kv_by_key);
    shim_register_family(p, "RequestUGCDetails", (void *)request_details);
    for (int v = 1; v <= 21; v++) {
        char name[48];
        snprintf(name, sizeof name, "STEAMUGC_INTERFACE_VERSION%03d", v);
        shim_register(name, "GetQueryUGCResult", v >= 20 ? (void *)query_result_new : (void *)query_result_old);
    }
}
