#include <stdlib.h>
#include <string.h>
#include <time.h>

#include <windows.h>

#include "../shim.h"

typedef struct { uint32_t id, kind, value, def; char *name; bool dirty; } Stat;
typedef struct { uint32_t stat_id, bit, time; char *name; bool dirty; char *title, *desc; bool hidden, hidden_absent; float percent; bool has_display; } Ach;

static Stat *g_stats;
static size_t g_n_stats;
static Ach *g_achs;
static size_t g_n_achs;
static bool g_loaded;

static uint32_t rd32(const uint8_t *p) { uint32_t v; memcpy(&v, p, 4); return v; }
static void wr32(uint8_t *p, uint32_t v) { memcpy(p, &v, 4); }

static void clear_state(void) {
    for (size_t i = 0; i < g_n_stats; i++) free(g_stats[i].name);
    for (size_t i = 0; i < g_n_achs; i++) { free(g_achs[i].name); free(g_achs[i].title); free(g_achs[i].desc); }
    free(g_stats);
    free(g_achs);
    g_stats = NULL;
    g_achs = NULL;
    g_n_stats = g_n_achs = 0;
}

static char *read_name(const uint8_t **p, const uint8_t *end) {
    if (end - *p < 2) return NULL;
    size_t n = (size_t)(*p)[0] | ((size_t)(*p)[1] << 8);
    *p += 2;
    if ((size_t)(end - *p) < n) return NULL;
    char *s = malloc(n + 1);
    if (!s) return NULL;
    memcpy(s, *p, n);
    s[n] = 0;
    *p += n;
    return s;
}

static bool parse_reply_raw(const uint8_t *b, size_t len) {
    const uint8_t *p = b, *end = b + len;
    if (len < 8) return false;
    p += 4;
    uint32_t n = rd32(p); p += 4;
    clear_state();
    g_stats = calloc(n ? n : 1, sizeof *g_stats);
    if (!g_stats) return false;
    for (uint32_t i = 0; i < n; i++) {
        if (end - p < 16) return false;
        Stat *s = &g_stats[g_n_stats++];
        s->id = rd32(p); s->kind = rd32(p + 4); s->value = rd32(p + 8); s->def = rd32(p + 12);
        p += 16;
        if (!(s->name = read_name(&p, end))) return false;
    }
    if (end - p < 4) return false;
    uint32_t m = rd32(p); p += 4;
    g_achs = calloc(m ? m : 1, sizeof *g_achs);
    if (!g_achs) return false;
    for (uint32_t i = 0; i < m; i++) {
        if (end - p < 12) return false;
        Ach *a = &g_achs[g_n_achs++];
        a->stat_id = rd32(p); a->bit = rd32(p + 4); a->time = rd32(p + 8);
        p += 12;
        if (!(a->name = read_name(&p, end))) return false;
    }
    if (end - p >= 4) {
        uint32_t d = rd32(p); p += 4;
        for (uint32_t i = 0; i < d; i++) {
            char *api = read_name(&p, end), *title = read_name(&p, end), *desc = read_name(&p, end);
            if (!api || !title || !desc || end - p < 5) { free(api); free(title); free(desc); return true; }
            uint8_t hidden_flags = *p++;
            bool hidden = hidden_flags & 1, hidden_absent = hidden_flags & 2;
            float percent;
            memcpy(&percent, p, 4); p += 4;
            for (size_t k = 0; k < g_n_achs; k++) {
                if (strcmp(g_achs[k].name, api)) continue;
                g_achs[k].title = title; g_achs[k].desc = desc; g_achs[k].hidden = hidden; g_achs[k].hidden_absent = hidden_absent; g_achs[k].percent = percent; g_achs[k].has_display = true;
                title = desc = NULL;
                break;
            }
            free(api); free(title); free(desc);
        }
    }
    return true;
}

static int ach_by_name(const void *a, const void *b) { return _stricmp(((const Ach *)a)->name, ((const Ach *)b)->name); }

static bool parse_reply(const uint8_t *b, size_t len) {
    bool ok = parse_reply_raw(b, len);
    if (ok && g_achs && g_n_achs > 1) qsort(g_achs, g_n_achs, sizeof *g_achs, ach_by_name);
    return ok;
}

static void post_received(int32_t eresult) {
    const Snapshot *s = shim_snapshot();
    uint8_t resp[24] = {0};
    uint64_t gameid = s && s->has_ticket ? s->ticket.appid : 0, sid = s ? s->steamid : 0;
    memcpy(resp, &gameid, 8);
    memcpy(resp + 8, &eresult, 4);
    memcpy(resp + 16, &sid, 8);
    shim_post_callback(1101, resp, sizeof resp);
}

typedef struct { uint8_t *reply; size_t n; int32_t status; bool called; } StatsJob;
static StatsJob *volatile g_pending_stats;
static volatile LONG g_stats_inflight;
static HANDLE g_stats_event;

static void apply_stats(StatsJob *j) {
    int32_t eresult = 1;
    if (j->called) {
        if (j->status == 1 && parse_reply(j->reply, j->n)) {
            g_loaded = true;
            shim_log("stats loaded: %lu stats, %lu achievements", (unsigned long)g_n_stats, (unsigned long)g_n_achs);
        } else {
            shim_log("stats unavailable: status=%d bytes=%lu", j->status, (unsigned long)j->n);
            eresult = j->status >= 0 && j->status != 1 ? j->status : 2;
        }
    } else {
        shim_log("stats: bridge unreachable, the game will see empty stats");
    }
    free(j->reply);
    free(j);
    post_received(eresult);
}

enum { STATS_WAIT_MS = 2500 };

static void ensure_stats(void) {
    StatsJob *j = InterlockedExchangePointer((PVOID volatile *)&g_pending_stats, NULL);
    if (!j && !g_loaded && g_stats_inflight > 0 && g_stats_event) {
        static DWORD wait_started;
        DWORD now = GetTickCount();
        if (!wait_started) wait_started = now;
        DWORD spent = now - wait_started;
        if (spent < STATS_WAIT_MS) {
            WaitForSingleObject(g_stats_event, STATS_WAIT_MS - spent);
            j = InterlockedExchangePointer((PVOID volatile *)&g_pending_stats, NULL);
        }
    }
    if (j) apply_stats(j);
}

static void stats_finish(void *a, void *b, int c) {
    (void)a; (void)b; (void)c;
    ensure_stats();
}

static DWORD WINAPI stats_thread(LPVOID arg) {
    StatsJob *j = arg;
    const Snapshot *s = shim_snapshot();
    uint8_t payload[4];
    wr32(payload, s && s->has_ticket ? s->ticket.appid : 0);
    size_t cap = 4u << 20;
    j->reply = malloc(cap);
    j->status = -99;
    j->called = j->reply && bridge_call(0xFFFF, 4, payload, sizeof payload, &j->status, j->reply, cap, &j->n, 15000);
    StatsJob *old = InterlockedExchangePointer((PVOID volatile *)&g_pending_stats, j);
    if (old) { free(old->reply); free(old); }
    InterlockedDecrement(&g_stats_inflight);
    if (g_stats_event) SetEvent(g_stats_event);
    shim_defer(stats_finish, NULL, NULL, 0);
    return 0;
}

static uint64_t SHIM_TC request_current_stats(void *self) {
    (void)self;
    StatsJob *j = calloc(1, sizeof *j);
    if (!j) return 0;
    if (!g_stats_event) g_stats_event = CreateEventA(NULL, FALSE, FALSE, NULL);
    InterlockedIncrement(&g_stats_inflight);
    HANDLE t = CreateThread(NULL, 0, stats_thread, j, 0, NULL);
    if (!t) { stats_thread(j); return 1; }
    CloseHandle(t);
    return 1;
}

void userstats_preload(void) {
    static volatile LONG started;
    if (InterlockedExchange(&started, 1)) return;
    request_current_stats(NULL);
}

static Stat *find_stat(const char *name) {
    ensure_stats();
    for (size_t i = 0; name && i < g_n_stats; i++) if (!strcmp(g_stats[i].name, name)) return &g_stats[i];
    return NULL;
}
static Ach *find_ach(const char *name) {
    ensure_stats();
    for (size_t i = 0; name && i < g_n_achs; i++) if (!strcmp(g_achs[i].name, name)) return &g_achs[i];
    return NULL;
}
static bool is_float(const Stat *s) { return s->kind == 2 || s->kind == 3; }

static uint64_t SHIM_TC get_stat(void *self, const char *name, void *out) {
    (void)self;
    Stat *s = find_stat(name);
    if (!out) return 0;
    if (!s) { memset(out, 0, 4); return 0; }
    memcpy(out, &s->value, 4);
    return 1;
}

static bool stat_getter_matches(void *self, const Stat *s, bool wants_float) {
    const char *iface = shim_iface_name(self);
    const char *v = iface ? strstr(iface, "VERSION") : NULL;
    return !v || atoi(v + 7) < 12 || is_float(s) == wants_float;
}
static uint64_t SHIM_TC get_stat_int(void *self, const char *name, void *out) {
    Stat *s = find_stat(name);
    if (s && out && !stat_getter_matches(self, s, false)) { memset(out, 0, 4); return 0; }
    return get_stat(self, name, out);
}
static uint64_t SHIM_TC get_stat_float(void *self, const char *name, void *out) {
    Stat *s = find_stat(name);
    if (s && out && !stat_getter_matches(self, s, true)) { memset(out, 0, 4); return 0; }
    return get_stat(self, name, out);
}

uint64_t set_stat_impl(void *self, const char *name, int32_t iv, uint32_t fbits) {
    (void)self;
    Stat *s = find_stat(name);
    if (!s) return 0;
    uint32_t v = is_float(s) ? fbits : (uint32_t)iv;
    if (v != s->value) { s->value = v; s->dirty = true; }
    return 1;
}
#ifdef _WIN64
uint64_t set_stat_any(void *self, const char *name, int32_t iv);
__asm__(".text\n.globl set_stat_any\nset_stat_any:\n\tmovd %xmm2, %r9d\n\tjmp set_stat_impl\n");
#else
static uint64_t SHIM_TC set_stat_any(void *self, const char *name, uint32_t bits) { return set_stat_impl(self, name, (int32_t)bits, bits); }
#endif
int shim_achievement_achieved(const char *name) {
    Ach *a = find_ach(name);
    return a ? a->time != 0 : -1;
}

static uint64_t SHIM_TC get_achievement(void *self, const char *name, uint8_t *achieved) {
    (void)self;
    Ach *a = find_ach(name);
    if (!a) { if (achieved) *achieved = 0; return 0; }
    if (achieved) *achieved = a->time != 0;
    return 1;
}
static uint64_t SHIM_TC get_achievement_time(void *self, const char *name, uint8_t *achieved, uint32_t *when) {
    (void)self;
    Ach *a = find_ach(name);
    if (!a) { if (achieved) *achieved = 0; if (when) *when = 0; return 0; }
    if (achieved) *achieved = a->time != 0;
    if (when) *when = a->time;
    return 1;
}
static uint64_t SHIM_TC set_achievement(void *self, const char *name) {
    (void)self;
    Ach *a = find_ach(name);
    shim_trace("SetAchievement %s (%s)", name ? name : "", a ? "known" : "unknown");
    if (!a) return 0;
    if (!a->time) { a->time = (uint32_t)time(NULL); a->dirty = true; }
    return 1;
}
static uint64_t SHIM_TC clear_achievement(void *self, const char *name) {
    (void)self;
    Ach *a = find_ach(name);
    if (!a) return 0;
    if (a->time) { a->time = 0; a->dirty = true; }
    return 1;
}
static uint32_t SHIM_TC num_achievements(void *self) { (void)self; ensure_stats(); return (uint32_t)g_n_achs; }
static const char *SHIM_TC achievement_name(void *self, uint32_t i) { (void)self; ensure_stats(); return i < g_n_achs ? g_achs[i].name : ""; }
static uint64_t SHIM_TC indicate_progress(void *self, const char *name, uint32_t cur, uint32_t max) {
    (void)self;
    shim_trace("IndicateAchievementProgress %s %u/%u", name ? name : "", cur, max);
    if (!name || !*name) return 0;
    ensure_stats();
    for (uint32_t i = 0; i < g_n_achs; i++) if (!strcmp(g_achs[i].name, name)) return 1;
    return 0;
}
static uint64_t SHIM_TC reset_all_stats(void *self, uint8_t achievements_too) {
    (void)self; (void)achievements_too;
    shim_log("ResetAllStats ignored: erasing account progress is not supported by the shim");
    return 0;
}

static void post_stored(int32_t eresult) {
    const Snapshot *s = shim_snapshot();
    uint8_t resp[16] = {0};
    uint64_t gameid = s && s->has_ticket ? s->ticket.appid : 0;
    memcpy(resp, &gameid, 8);
    memcpy(resp + 8, &eresult, 4);
    shim_post_callback(1102, resp, sizeof resp);
}

static void post_achievement_stored(const char *name) {
    const Snapshot *s = shim_snapshot();
    uint8_t resp[152] = {0};
    uint64_t gameid = s && s->has_ticket ? s->ticket.appid : 0;
    memcpy(resp, &gameid, 8);
    strncpy((char *)resp + 9, name, 127);
    shim_post_callback(1103, resp, sizeof resp);
}

static const char *SHIM_TC achievement_display_attribute(void *self, const char *name, const char *key) {
    (void)self;
    Ach *a = find_ach(name);
    if (!a || !key) return "";
    if (!strcmp(key, "name")) return a->has_display && a->title[0] ? a->title : a->name;
    if (!strcmp(key, "desc")) return a->has_display ? a->desc : "";
    if (!strcmp(key, "hidden")) return a->has_display && a->hidden_absent ? "" : a->hidden ? "1" : "0";
    return "";
}
static uint64_t SHIM_TC achievement_percent(void *self, const char *name, float *percent) {
    (void)self;
    Ach *a = find_ach(name);
    if (!a || !a->has_display) return 0;
    if (percent) *percent = a->percent;
    return 1;
}

bool shim_userstats_stat_info(const char *name, uint32_t *id, bool *is_float_stat) {
    Stat *s = find_stat(name);
    if (!s) return false;
    if (id) *id = s->id;
    if (is_float_stat) *is_float_stat = is_float(s);
    return true;
}

static uint64_t SHIM_TC request_global_percentages(void *self) {
    (void)self;
    const Snapshot *sn = shim_snapshot();
    uint8_t res[16] = {0};
    uint64_t gameid = sn && sn->has_ticket ? sn->ticket.appid : 0;
    bool any = false;
    for (size_t i = 0; i < g_n_achs; i++) any = any || g_achs[i].has_display;
    int32_t eresult = any ? 1 : 2;
    memcpy(res, &gameid, 8);
    memcpy(res + 8, &eresult, 4);
    return shim_callresult_post(1110, res, sizeof res);
}

static int ach_by_rank(int rank) {
    int order[512], n = 0;
    for (size_t i = 0; i < g_n_achs && n < 512; i++) if (g_achs[i].has_display) order[n++] = (int)i;
    for (int i = 1; i < n; i++) {
        int v = order[i], j = i - 1;
        while (j >= 0 && g_achs[order[j]].percent < g_achs[v].percent) { order[j + 1] = order[j]; j--; }
        order[j + 1] = v;
    }
    return rank >= 0 && rank < n ? order[rank] : -1;
}

static int fill_most(int rank, char *name, uint32_t len, float *percent, uint8_t *achieved) {
    int k = ach_by_rank(rank);
    if (k < 0) return -1;
    if (name && len) { strncpy(name, g_achs[k].name, len - 1); name[len - 1] = 0; }
    if (percent) *percent = g_achs[k].percent;
    if (achieved) *achieved = g_achs[k].time != 0;
    return rank;
}

static int SHIM_TC most_achieved(void *self, char *name, uint32_t len, float *percent, uint8_t *achieved) {
    (void)self;
    return fill_most(0, name, len, percent, achieved);
}

static int SHIM_TC next_most_achieved(void *self, int prev, char *name, uint32_t len, float *percent, uint8_t *achieved) {
    (void)self;
    return prev < 0 ? -1 : fill_most(prev + 1, name, len, percent, achieved);
}

static bool is_self_user(uint64_t id) { const Snapshot *s = shim_snapshot(); return s && s->steamid == id; }
bool shim_other_stat(uint64_t id, const char *name, void *out);
bool shim_other_achievement(uint64_t id, const char *name, uint8_t *achieved, uint32_t *when);
static uint64_t SHIM_TC user_stat(void *self, uint64_t id, const char *name, void *out) { return is_self_user(id) ? get_stat(self, name, out) : shim_other_stat(id, name, out); }
static uint64_t SHIM_TC user_achievement(void *self, uint64_t id, const char *name, uint8_t *achieved) { return is_self_user(id) ? get_achievement(self, name, achieved) : shim_other_achievement(id, name, achieved, NULL); }
static uint64_t SHIM_TC user_achievement_time(void *self, uint64_t id, const char *name, uint8_t *achieved, uint32_t *when) {
    return is_self_user(id) ? get_achievement_time(self, name, achieved, when) : shim_other_achievement(id, name, achieved, when);
}

static double g_avg_count[512], g_avg_seconds[512];
static uint64_t SHIM_TC update_avg_rate(void *self, const char *name, float count, double seconds) {
    (void)self;
    Stat *st = find_stat(name);
    if (!st || st->kind != 3 || seconds <= 0) return 0;
    size_t i = (size_t)(st - g_stats) % 512;
    g_avg_count[i] += count;
    g_avg_seconds[i] += seconds;
    float avg = (float)(g_avg_count[i] / g_avg_seconds[i]);
    uint32_t bits;
    memcpy(&bits, &avg, 4);
    if (bits != st->value) { st->value = bits; st->dirty = true; }
    return 1;
}

static uint32_t SHIM_TC num_stats(void *self, uint64_t gameid) { (void)self; (void)gameid; ensure_stats(); return (uint32_t)g_n_stats; }
static const char *SHIM_TC stat_name(void *self, uint64_t gameid, uint32_t i) { (void)self; (void)gameid; ensure_stats(); return i < g_n_stats ? g_stats[i].name : ""; }
static int SHIM_TC stat_type(void *self, uint64_t gameid, const char *name) {
    (void)self; (void)gameid;
    Stat *st = find_stat(name);
    return st ? (int)st->kind : 0;
}

static uint64_t SHIM_TC store_stats(void *self) {
    (void)self;
    size_t cap = 8 + (g_n_stats + g_n_achs) * 8, n = 0;
    uint8_t *payload = calloc(1, cap ? cap : 8);
    if (!payload) return 0;
    uint8_t *o = payload + 8;
    for (size_t i = 0; i < g_n_stats; i++)
        if (g_stats[i].dirty) { wr32(o, g_stats[i].id); wr32(o + 4, g_stats[i].value); o += 8; n++; }
    for (size_t i = 0; i < g_n_achs; i++) {
        if (!g_achs[i].dirty) continue;
        uint32_t id = g_achs[i].stat_id, mask = 0;
        bool seen = false;
        for (uint8_t *q = payload + 8; q < o; q += 8) if (rd32(q) == id) seen = true;
        if (seen) continue;
        for (size_t j = 0; j < g_n_achs; j++) if (g_achs[j].stat_id == id && g_achs[j].time) mask |= 1u << g_achs[j].bit;
        wr32(o, id); wr32(o + 4, mask); o += 8; n++;
    }
    wr32(payload, 0);
    wr32(payload + 4, (uint32_t)n);
    if (n == 0) { free(payload); post_stored(1); return 1; }
    int32_t status = -99;
    bool reached = bridge_call(0xFFFF, 5, payload, 8 + n * 8, &status, NULL, 0, NULL, 15000);
    free(payload);
    if (!reached || status != 1) {
        shim_log("StoreStats failed: bridge %s, status=%d", reached ? "answered" : "unreachable", status);
        post_stored(status >= 0 && reached ? status : 2);
        return 1;
    }
    shim_log("StoreStats ok: %lu changes sent to Steam", (unsigned long)n);
    for (size_t i = 0; i < g_n_stats; i++) g_stats[i].dirty = false;
    for (size_t i = 0; i < g_n_achs; i++) {
        if (g_achs[i].dirty && g_achs[i].time) { post_achievement_stored(g_achs[i].name); shim_overlay_achievement(g_achs[i].name); }
        g_achs[i].dirty = false;
    }
    post_stored(1);
    return 1;
}

void register_userstats(void) {
    const char *p = "STEAMUSERSTATS_INTERFACE_VERSION";
    shim_register_family(p, "RequestCurrentStats", (void *)request_current_stats);
    shim_register_family(p, "GetStat", (void *)get_stat_int);
    shim_register_family(p, "GetStat_2", (void *)get_stat_float);
    shim_register_family(p, "SetStat", (void *)set_stat_any);
    shim_register_family(p, "SetStat_2", (void *)set_stat_any);
    shim_register_family(p, "GetAchievement", (void *)get_achievement);
    shim_register_family(p, "GetAchievementAndUnlockTime", (void *)get_achievement_time);
    shim_register_family(p, "SetAchievement", (void *)set_achievement);
    shim_register_family(p, "ClearAchievement", (void *)clear_achievement);
    shim_register_family(p, "GetNumAchievements", (void *)num_achievements);
    shim_register_family(p, "GetAchievementName", (void *)achievement_name);
    shim_register_family(p, "IndicateAchievementProgress", (void *)indicate_progress);
    shim_register_family(p, "ResetAllStats", (void *)reset_all_stats);
    shim_register_family(p, "StoreStats", (void *)store_stats);
    shim_register_family_ab(p, "GetAchievementDisplayAttribute", 8, (void *)achievement_display_attribute);
    shim_register_family(p, "GetAchievementAchievedPercent", (void *)achievement_percent);
    shim_register_family(p, "GetUserStat", (void *)user_stat);
    shim_register_family(p, "GetUserStat_2", (void *)user_stat);
    shim_register_family(p, "GetUserAchievement", (void *)user_achievement);
    shim_register_family(p, "GetUserAchievementAndUnlockTime", (void *)user_achievement_time);
    shim_register_family_ab(p, "UpdateAvgRateStat", 16, (void *)update_avg_rate);
    shim_register_family_ab(p, "GetNumStats", 8, (void *)num_stats);
    shim_register_family_ab(p, "GetStatName", 12, (void *)stat_name);
    shim_register_family_ab(p, "GetStatType", 12, (void *)stat_type);
    shim_register_family(p, "RequestGlobalAchievementPercentages", (void *)request_global_percentages);
    shim_register_family(p, "GetMostAchievedAchievementInfo", (void *)most_achieved);
    shim_register_family(p, "GetNextMostAchievedAchievementInfo", (void *)next_most_achieved);
}
