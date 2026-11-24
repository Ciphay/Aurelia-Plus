#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "../shim.h"

bool shim_userstats_stat_info(const char *name, uint32_t *id, bool *is_float_stat);

enum {
    EMSG_LBS_SET_SCORE = 5414, EMSG_LBS_FIND = 5416, EMSG_LBS_ENTRIES = 5418, EMSG_LBS_SET_UGC = 5529,
    EMSG_PLAYERS = 5592, EMSG_GLOBAL_STATS = 7901,
    CB_FIND = 1104, CB_SCORES = 1105, CB_UPLOADED = 1106, CB_PLAYERS = 1107, CB_UGC_SET = 1111, CB_GLOBAL_STATS = 1112,
    WAIT_MS = 15000, MAX_BOARDS = 64, MAX_DETAILS = 64, SETS = 8, MAX_DAYS = 60, MAX_GSTATS = 256,
};

typedef struct { int32_t id; char name[128]; int32_t sort, display, count; } Board;
typedef struct { uint64_t steamid; int32_t rank, score; uint64_t ugc; int32_t n_details; int32_t details[MAX_DETAILS]; } Entry;
typedef struct { uint64_t handle; Entry *e; int n; } EntrySet;
typedef struct { uint64_t board; int32_t score; } Ctx;

static Board g_boards[MAX_BOARDS];
static int g_n_boards;
static EntrySet g_sets[SETS];
static uint64_t g_next_set = 0x7000;
static SRWLOCK g_lock = SRWLOCK_INIT;

static uint32_t app_id(void) {
    const Snapshot *s = shim_snapshot();
    return s && s->has_ticket ? s->ticket.appid : 0;
}

static int32_t pb_i32(const PbField *f) { return (int32_t)(uint32_t)f->v; }

static Board *board_locked(uint64_t h) {
    for (int i = 0; i < g_n_boards; i++) if ((uint64_t)g_boards[i].id == h) return &g_boards[i];
    return NULL;
}

static bool parse_find(const CmReply *r, void *ctx, void *out, int *size) {
    (void)ctx;
    PbR in;
    PbField f;
    int32_t eresult = 2;
    Board b;
    memset(&b, 0, sizeof b);
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field == 1) eresult = pb_i32(&f);
        else if (f.field == 2) b.id = pb_i32(&f);
        else if (f.field == 3) b.count = pb_i32(&f);
        else if (f.field == 4) b.sort = pb_i32(&f);
        else if (f.field == 5) b.display = pb_i32(&f);
        else if (f.field == 6 && f.wt == 2) { size_t n = f.len < sizeof b.name - 1 ? f.len : sizeof b.name - 1; memcpy(b.name, f.data, n); }
    }
    if (in.bad || *size < 16) return false;
    uint64_t handle = 0;
    uint8_t found = 0;
    if (eresult == 1 && b.id > 0) {
        AcquireSRWLockExclusive(&g_lock);
        Board *slot = board_locked((uint64_t)b.id);
        if (!slot && g_n_boards < MAX_BOARDS) slot = &g_boards[g_n_boards++];
        if (slot) *slot = b;
        ReleaseSRWLockExclusive(&g_lock);
        handle = (uint64_t)b.id;
        found = 1;
    }
    memset(out, 0, 16);
    memcpy(out, &handle, 8);
    ((uint8_t *)out)[8] = found;
    *size = 16;
    return true;
}

static uint64_t find_board(const char *name, int sort, int display, bool create) {
    uint64_t h = shim_callresult_reserve();
    if (!name || !*name) { shim_callresult_fail_handle(h); return h; }
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_varint(&w, 2, (uint64_t)(int64_t)sort);
    pbw_varint(&w, 3, (uint64_t)(int64_t)display);
    pbw_varint(&w, 4, create);
    pbw_str(&w, 5, name);
    shim_cm_async(h, CB_FIND, EMSG_LBS_FIND, false, NULL, &w, WAIT_MS, parse_find, NULL);
    return h;
}

static uint64_t SHIM_TC find_or_create(void *self, const char *name, int sort, int display) { (void)self; return find_board(name, sort, display, true); }
static uint64_t SHIM_TC find_leaderboard(void *self, const char *name) { (void)self; return find_board(name, 0, 0, false); }

static const char *SHIM_TC board_name(void *self, uint64_t h) {
    (void)self;
    static char names[4][128];
    static int turn;
    char *out = names[turn++ & 3];
    out[0] = 0;
    AcquireSRWLockExclusive(&g_lock);
    Board *b = board_locked(h);
    if (b) memcpy(out, b->name, sizeof b->name);
    ReleaseSRWLockExclusive(&g_lock);
    return out;
}

static int board_field(uint64_t h, int which) {
    int v = 0;
    AcquireSRWLockExclusive(&g_lock);
    Board *b = board_locked(h);
    if (b) v = which == 0 ? b->count : which == 1 ? b->sort : b->display;
    ReleaseSRWLockExclusive(&g_lock);
    return v;
}
static int SHIM_TC board_count(void *self, uint64_t h) { (void)self; return board_field(h, 0); }
static int SHIM_TC board_sort(void *self, uint64_t h) { (void)self; return board_field(h, 1); }
static int SHIM_TC board_display(void *self, uint64_t h) { (void)self; return board_field(h, 2); }

static bool parse_entries(const CmReply *r, void *ctx, void *out, int *size) {
    PbR in;
    PbField f;
    int32_t eresult = 2;
    Entry *list = calloc(256, sizeof *list);
    int n = 0;
    if (!list || *size < 24) { free(list); return false; }
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field == 1) eresult = pb_i32(&f);
        else if (f.field == 3 && f.wt == 2 && n < 256) {
            PbR ein;
            PbField g;
            Entry *e = &list[n];
            e->ugc = ~0ULL;
            pbr_init(&ein, f.data, f.len);
            while (pbr_next(&ein, &g)) {
                if (g.field == 1) e->steamid = g.v;
                else if (g.field == 2) e->rank = pb_i32(&g);
                else if (g.field == 3) e->score = pb_i32(&g);
                else if (g.field == 4 && g.wt == 2) {
                    size_t cnt = g.len / 4 < MAX_DETAILS ? g.len / 4 : MAX_DETAILS;
                    memcpy(e->details, g.data, cnt * 4);
                    e->n_details = (int32_t)cnt;
                } else if (g.field == 5) e->ugc = g.v;
            }
            n++;
        }
    }
    if (in.bad || eresult != 1) { free(list); return false; }
    uint64_t handle;
    AcquireSRWLockExclusive(&g_lock);
    handle = ++g_next_set;
    EntrySet *slot = &g_sets[handle % SETS];
    free(slot->e);
    slot->handle = handle;
    slot->e = list;
    slot->n = n;
    ReleaseSRWLockExclusive(&g_lock);
    memset(out, 0, 24);
    memcpy(out, &((Ctx *)ctx)->board, 8);
    memcpy((uint8_t *)out + 8, &handle, 8);
    int32_t count = n;
    memcpy((uint8_t *)out + 16, &count, 4);
    *size = 24;
    return true;
}

static uint64_t download(uint64_t h, int request, int start, int end, const uint64_t *users, int n_users) {
    uint64_t call = shim_callresult_reserve();
    Ctx *ctx = calloc(1, sizeof *ctx);
    if (!ctx) { shim_callresult_fail_handle(call); return call; }
    ctx->board = h;
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_varint(&w, 2, h);
    pbw_varint(&w, 3, (uint64_t)(int64_t)start);
    pbw_varint(&w, 4, (uint64_t)(int64_t)end);
    pbw_varint(&w, 5, (uint64_t)(int64_t)request);
    for (int i = 0; i < n_users; i++) pbw_fixed64(&w, 6, users[i]);
    shim_cm_async(call, CB_SCORES, EMSG_LBS_ENTRIES, false, NULL, &w, WAIT_MS, parse_entries, ctx);
    return call;
}

static uint64_t SHIM_TC download_entries(void *self, uint64_t h, int request, int start, int end) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    uint64_t ids[512];
    int n = 0;
    if (request == 2 && s) {
        ids[n++] = s->steamid;
        for (size_t i = 0; i < s->n_friends && n < 512; i++) ids[n++] = s->friends[i].steamid;
    }
    return download(h, request, start, end, ids, n);
}

static uint64_t SHIM_TC download_for_users(void *self, uint64_t h, const uint64_t *users, int n) {
    (void)self;
    if (n < 0 || n > 512 || (n && !users)) { uint64_t c = shim_callresult_reserve(); shim_callresult_fail_handle(c); return c; }
    return download(h, 3, 0, 0, users, n);
}

static uint64_t SHIM_TC downloaded_entry(void *self, uint64_t set, int index, uint8_t *out, int32_t *details, int max_details) {
    (void)self;
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    EntrySet *s = &g_sets[set % SETS];
    if (s->e && s->handle == set && index >= 0 && index < s->n && out) {
        const Entry *e = &s->e[index];
        int copy = max_details < e->n_details ? max_details : e->n_details;
        if (copy < 0) copy = 0;
        memcpy(out, &e->steamid, 8);
        memcpy(out + 8, &e->rank, 4);
        memcpy(out + 12, &e->score, 4);
        memcpy(out + 16, &e->n_details, 4);
        memcpy(out + 20, &e->ugc, 8);
        if (details && copy) memcpy(details, e->details, (size_t)copy * 4);
        ok = true;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static bool parse_uploaded(const CmReply *r, void *ctx, void *out, int *size) {
    PbR in;
    PbField f;
    int32_t eresult = 2, rank_prev = 0, rank_new = 0;
    uint8_t changed = 0;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field == 1) eresult = pb_i32(&f);
        else if (f.field == 3) changed = f.v != 0;
        else if (f.field == 4) rank_prev = pb_i32(&f);
        else if (f.field == 5) rank_new = pb_i32(&f);
    }
    if (in.bad || *size < 32) return false;
    memset(out, 0, 32);
    ((uint8_t *)out)[0] = eresult == 1;
    memcpy((uint8_t *)out + 8, &((Ctx *)ctx)->board, 8);
    memcpy((uint8_t *)out + 16, &((Ctx *)ctx)->score, 4);
    ((uint8_t *)out)[20] = changed;
    memcpy((uint8_t *)out + 24, &rank_new, 4);
    memcpy((uint8_t *)out + 28, &rank_prev, 4);
    *size = 32;
    return true;
}

static uint64_t SHIM_TC upload_score(void *self, uint64_t h, int method, int32_t score, const int32_t *details, int n_details) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    Ctx *ctx = calloc(1, sizeof *ctx);
    if (!ctx || n_details < 0 || n_details > MAX_DETAILS || (n_details && !details)) { free(ctx); shim_callresult_fail_handle(call); return call; }
    ctx->board = h;
    ctx->score = score;
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_varint(&w, 2, h);
    pbw_varint(&w, 3, (uint64_t)(int64_t)score);
    if (n_details) pbw_bytes(&w, 4, details, (size_t)n_details * 4);
    pbw_varint(&w, 5, (uint64_t)(int64_t)method);
    shim_cm_async(call, CB_UPLOADED, EMSG_LBS_SET_SCORE, false, NULL, &w, WAIT_MS, parse_uploaded, ctx);
    return call;
}

static bool parse_ugc_set(const CmReply *r, void *ctx, void *out, int *size) {
    PbR in;
    PbField f;
    int32_t eresult = 2;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) if (f.field == 1) eresult = pb_i32(&f);
    if (in.bad || *size < 16) return false;
    memset(out, 0, 16);
    memcpy(out, &eresult, 4);
    memcpy((uint8_t *)out + 8, &((Ctx *)ctx)->board, 8);
    *size = 16;
    return true;
}

static uint64_t SHIM_TC attach_ugc(void *self, uint64_t h, uint64_t ugc) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    Ctx *ctx = calloc(1, sizeof *ctx);
    if (!ctx) { shim_callresult_fail_handle(call); return call; }
    ctx->board = h;
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_varint(&w, 2, h);
    pbw_fixed64(&w, 3, ugc);
    shim_cm_async(call, CB_UGC_SET, 5529, false, NULL, &w, WAIT_MS, parse_ugc_set, ctx);
    return call;
}

static bool parse_players(const CmReply *r, void *ctx, void *out, int *size) {
    (void)ctx;
    PbR in;
    PbField f;
    int32_t eresult = 2, players = 0;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field == 1) eresult = pb_i32(&f);
        else if (f.field == 2) players = pb_i32(&f);
    }
    if (in.bad || *size < 8) return false;
    memset(out, 0, 8);
    ((uint8_t *)out)[0] = eresult == 1;
    memcpy((uint8_t *)out + 4, &players, 4);
    *size = 8;
    return true;
}

static uint64_t SHIM_TC number_of_current_players(void *self) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    shim_cm_async(call, CB_PLAYERS, EMSG_PLAYERS, false, NULL, &w, WAIT_MS, parse_players, NULL);
    return call;
}

typedef struct { uint32_t id; int64_t value; } GStat;
typedef struct { int32_t day; int n; GStat s[MAX_GSTATS]; } GDay;
static GDay g_total;
static GDay g_days[MAX_DAYS];
static int g_n_days;
static bool g_global_loaded;

static void read_day(const uint8_t *data, size_t len, GDay *d) {
    PbR in;
    PbField f;
    pbr_init(&in, data, len);
    d->n = 0;
    while (pbr_next(&in, &f)) {
        if (f.field == 1) d->day = pb_i32(&f);
        else if (f.field == 2 && f.wt == 2 && d->n < MAX_GSTATS) {
            PbR sin;
            PbField g;
            GStat st = {0, 0};
            pbr_init(&sin, f.data, f.len);
            while (pbr_next(&sin, &g)) {
                if (g.field == 1) st.id = (uint32_t)g.v;
                else if (g.field == 2) st.value = (int64_t)g.v;
            }
            d->s[d->n++] = st;
        }
    }
}

static bool parse_global_stats(const CmReply *r, void *ctx, void *out, int *size) {
    (void)ctx;
    PbR in;
    PbField f;
    int32_t eresult = 2;
    GDay total;
    GDay *days = calloc(MAX_DAYS, sizeof *days);
    int n_days = 0;
    bool has_total = false;
    memset(&total, 0, sizeof total);
    if (!days || *size < 16) { free(days); return false; }
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field == 1) eresult = pb_i32(&f);
        else if (f.field == 4 && f.wt == 2) {
            GDay d;
            memset(&d, 0, sizeof d);
            read_day(f.data, f.len, &d);
            if (d.day == 0 && !has_total) { total = d; has_total = true; }
            else if (n_days < MAX_DAYS) days[n_days++] = d;
        }
    }
    if (in.bad) { free(days); return false; }
    if (eresult == 1) {
        AcquireSRWLockExclusive(&g_lock);
        g_total = total;
        memcpy(g_days, days, (size_t)n_days * sizeof *days);
        g_n_days = n_days;
        g_global_loaded = true;
        ReleaseSRWLockExclusive(&g_lock);
    }
    free(days);
    uint64_t gameid = app_id();
    memset(out, 0, 16);
    memcpy(out, &gameid, 8);
    memcpy((uint8_t *)out + 8, &eresult, 4);
    *size = 16;
    return true;
}

static uint64_t SHIM_TC request_global_stats(void *self, int days) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_varint(&w, 2, (uint64_t)(days < 0 ? 0 : days > MAX_DAYS ? MAX_DAYS : days));
    shim_cm_async(call, CB_GLOBAL_STATS, EMSG_GLOBAL_STATS, false, NULL, &w, WAIT_MS, parse_global_stats, NULL);
    return call;
}

static bool day_value(const GDay *d, uint32_t id, int64_t *v) {
    for (int i = 0; i < d->n; i++) if (d->s[i].id == id) { *v = d->s[i].value; return true; }
    return false;
}

static double as_double(int64_t raw, bool is_float_stat) { double d; if (!is_float_stat) return (double)raw; memcpy(&d, &raw, 8); return d; }
static int64_t as_int(int64_t raw, bool is_float_stat) { return is_float_stat ? (int64_t)as_double(raw, true) : raw; }

static bool global_lookup(const char *name, int day_index, int64_t *raw, bool *is_float_stat) {
    uint32_t id;
    if (!shim_userstats_stat_info(name, &id, is_float_stat)) return false;
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    if (g_global_loaded) {
        if (day_index < 0) ok = day_value(&g_total, id, raw);
        else if (day_index < g_n_days) ok = day_value(&g_days[day_index], id, raw);
    }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static uint64_t SHIM_TC global_stat_int(void *self, const char *name, int64_t *out) {
    (void)self;
    int64_t raw;
    bool fl;
    if (!out || !global_lookup(name, -1, &raw, &fl)) return 0;
    *out = as_int(raw, fl);
    return 1;
}

static uint64_t SHIM_TC global_stat_double(void *self, const char *name, double *out) {
    (void)self;
    int64_t raw;
    bool fl;
    if (!out || !global_lookup(name, -1, &raw, &fl)) return 0;
    *out = as_double(raw, fl);
    return 1;
}

static int SHIM_TC global_history_int(void *self, const char *name, int64_t *out, uint32_t bytes) {
    (void)self;
    int n = 0, cap = (int)(bytes / 8);
    for (int i = 0; out && i < cap; i++, n++) {
        int64_t raw;
        bool fl;
        if (!global_lookup(name, i, &raw, &fl)) break;
        out[i] = as_int(raw, fl);
    }
    return n;
}

static int SHIM_TC global_history_double(void *self, const char *name, double *out, uint32_t bytes) {
    (void)self;
    int n = 0, cap = (int)(bytes / 8);
    for (int i = 0; out && i < cap; i++, n++) {
        int64_t raw;
        bool fl;
        if (!global_lookup(name, i, &raw, &fl)) break;
        out[i] = as_double(raw, fl);
    }
    return n;
}

void register_leaderboards(void) {
    const char *p = "STEAMUSERSTATS_INTERFACE_VERSION";
    shim_register_family(p, "FindOrCreateLeaderboard", (void *)find_or_create);
    shim_register_family(p, "FindLeaderboard", (void *)find_leaderboard);
    shim_register_family(p, "GetLeaderboardName", (void *)board_name);
    shim_register_family(p, "GetLeaderboardEntryCount", (void *)board_count);
    shim_register_family(p, "GetLeaderboardSortMethod", (void *)board_sort);
    shim_register_family(p, "GetLeaderboardDisplayType", (void *)board_display);
    shim_register_family(p, "DownloadLeaderboardEntries", (void *)download_entries);
    shim_register_family(p, "DownloadLeaderboardEntriesForUsers", (void *)download_for_users);
    shim_register_family(p, "GetDownloadedLeaderboardEntry", (void *)downloaded_entry);
    shim_register_family(p, "UploadLeaderboardScore", (void *)upload_score);
    shim_register_family(p, "AttachLeaderboardUGC", (void *)attach_ugc);
    shim_register_family(p, "GetNumberOfCurrentPlayers", (void *)number_of_current_players);
    shim_register_family(p, "RequestGlobalStats", (void *)request_global_stats);
    shim_register_family(p, "GetGlobalStat", (void *)global_stat_int);
    shim_register_family(p, "GetGlobalStat_2", (void *)global_stat_double);
    shim_register_family(p, "GetGlobalStatHistory", (void *)global_history_int);
    shim_register_family(p, "GetGlobalStatHistory_2", (void *)global_history_double);
}
