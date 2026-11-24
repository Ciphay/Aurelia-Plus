#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "../shim.h"

enum { WAIT_MS = 20000 };

static uint32_t app_id(void) { const Snapshot *s = shim_snapshot(); return s && s->has_ticket ? s->ticket.appid : 0; }
static uint64_t my_steamid(void) { const Snapshot *s = shim_snapshot(); return s ? s->steamid : 0; }
static void put32(uint8_t *d, int off, uint32_t v) { memcpy(d + off, &v, 4); }
static void put64(uint8_t *d, int off, uint64_t v) { memcpy(d + off, &v, 8); }

static uint64_t done(int callback, const void *data, int size) {
    uint64_t call = shim_callresult_reserve();
    shim_callresult_complete(call, callback, data, size);
    return call;
}

static bool bridge(uint16_t slot, const uint8_t *req, size_t n, int32_t *status, uint8_t *reply, size_t cap, size_t *got, int timeout_ms) {
    return bridge_call_fresh(0xFFFF, slot, req, n, status, reply, cap, got, timeout_ms);
}

typedef struct { uint64_t id; } IdCtx;
static IdCtx *id_ctx(uint64_t id) { IdCtx *c = calloc(1, sizeof *c); if (c) c->id = id; return c; }

static bool parse_follower_count(const CmReply *r, void *ctx, void *out, int *size) {
    if (*size < 24) return false;
    uint8_t *o = out;
    memset(o, 0, 24);
    int32_t eresult = r->eresult, count = 0;
    PbR in; PbField f;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) { if (f.field == 1) eresult = (int32_t)f.v; else if (f.field == 2) count = (int32_t)f.v; }
    put32(o, 0, (uint32_t)eresult); put64(o, 8, ((IdCtx *)ctx)->id); put32(o, 16, (uint32_t)count);
    *size = 24;
    return true;
}
static uint64_t SHIM_TC fr_follower_count(void *self, uint64_t id) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_fixed64(&w, 1, id);
    shim_cm_async(call, 344, 7515, false, NULL, &w, WAIT_MS, parse_follower_count, id_ctx(id));
    return call;
}

static bool parse_is_following(const CmReply *r, void *ctx, void *out, int *size) {
    if (*size < 24) return false;
    uint8_t *o = out;
    memset(o, 0, 24);
    int32_t eresult = r->eresult;
    uint8_t following = 0;
    PbR in; PbField f;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) { if (f.field == 1) eresult = (int32_t)f.v; else if (f.field == 2) following = f.v != 0; }
    put32(o, 0, (uint32_t)eresult); put64(o, 8, ((IdCtx *)ctx)->id); o[16] = following;
    *size = 24;
    return true;
}
static uint64_t SHIM_TC fr_is_following(void *self, uint64_t id) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_fixed64(&w, 1, id);
    shim_cm_async(call, 345, 7517, false, NULL, &w, WAIT_MS, parse_is_following, id_ctx(id));
    return call;
}

static bool parse_following_list(const CmReply *r, void *ctx, void *out, int *size) {
    (void)ctx;
    if (*size < 416) return false;
    uint8_t *o = out;
    memset(o, 0, 416);
    int32_t eresult = r->eresult, total = 0, n = 0;
    PbR in; PbField f;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field == 1) eresult = (int32_t)f.v;
        else if (f.field == 2) total = (int32_t)f.v;
        else if (f.field == 3 && n < 50) put64(o, 8 + 8 * n++, f.v);
    }
    put32(o, 0, (uint32_t)eresult); put32(o, 408, (uint32_t)n); put32(o, 412, (uint32_t)total);
    *size = 416;
    return true;
}
static uint64_t SHIM_TC fr_enumerate_following(void *self, uint32_t start) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_varint(&w, 1, start);
    shim_cm_async(call, 346, 7519, false, NULL, &w, WAIT_MS, parse_following_list, NULL);
    return call;
}

typedef struct { char name[260]; } NameCtx;
static bool parse_file_share(const CmReply *r, void *ctx, void *out, int *size) {
    if (*size < 280) return false;
    uint8_t *o = out;
    memset(o, 0, 280);
    int32_t eresult = r->eresult;
    uint64_t handle = ~0ULL;
    PbR in; PbField f;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) { if (f.field == 1) eresult = (int32_t)f.v; else if (f.field == 2) handle = f.v; }
    put32(o, 0, (uint32_t)eresult); put64(o, 8, handle);
    snprintf((char *)o + 16, 260, "%s", ((NameCtx *)ctx)->name);
    *size = 280;
    return true;
}
static uint64_t SHIM_TC rs_file_share(void *self, const char *name) {
    (void)self;
    NameCtx *c = calloc(1, sizeof *c);
    if (c && name) snprintf(c->name, sizeof c->name, "%s", name);
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_str(&w, 2, name ? name : "");
    shim_cm_async(call, 1307, 5232, false, NULL, &w, WAIT_MS, parse_file_share, c);
    return call;
}

enum { MAX_CLANS = 32 };
static struct { uint64_t clan; int32_t officers; uint64_t owner; bool joined; uint32_t members; } g_clans[MAX_CLANS];
static SRWLOCK g_clan_lock = SRWLOCK_INIT;

static int clan_slot(uint64_t clan, bool create) {
    for (int i = 0; i < MAX_CLANS; i++) if (g_clans[i].clan == clan) return i;
    if (!create) return -1;
    for (int i = 0; i < MAX_CLANS; i++) if (!g_clans[i].clan) { memset(&g_clans[i], 0, sizeof g_clans[i]); g_clans[i].clan = clan; return i; }
    return -1;
}

static bool parse_officers(const CmReply *r, void *ctx, void *out, int *size) {
    (void)ctx;
    if (*size < 16) return false;
    uint8_t *o = out;
    memset(o, 0, 16);
    int32_t eresult = r->eresult, count = 0;
    uint64_t clan = 0;
    PbR in; PbField f;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) { if (f.field == 1) eresult = (int32_t)f.v; else if (f.field == 2) clan = f.v; else if (f.field == 3) count = (int32_t)f.v; }
    if (eresult == 1 && clan) {
        AcquireSRWLockExclusive(&g_clan_lock);
        int s = clan_slot(clan, true);
        if (s >= 0) g_clans[s].officers = count;
        ReleaseSRWLockExclusive(&g_clan_lock);
    }
    put64(o, 0, clan); put32(o, 8, (uint32_t)count); o[12] = eresult == 1;
    *size = 16;
    return true;
}
static uint64_t SHIM_TC fr_clan_officers(void *self, uint64_t clan) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_fixed64(&w, 1, clan);
    shim_cm_async(call, 335, 5531, false, NULL, &w, WAIT_MS, parse_officers, NULL);
    return call;
}
static int32_t SHIM_TC fr_clan_officer_count(void *self, uint64_t clan) {
    (void)self;
    AcquireSRWLockShared(&g_clan_lock);
    int s = clan_slot(clan, false);
    int32_t n = s >= 0 ? g_clans[s].officers : 0;
    ReleaseSRWLockShared(&g_clan_lock);
    return n;
}
static uint64_t *SHIM_TC fr_clan_owner(void *self, uint64_t *ret, uint64_t clan) {
    (void)self;
    AcquireSRWLockShared(&g_clan_lock);
    int s = clan_slot(clan, false);
    *ret = s >= 0 ? g_clans[s].owner : 0;
    ReleaseSRWLockShared(&g_clan_lock);
    return ret;
}
static uint64_t *SHIM_TC fr_clan_officer_by_index(void *self, uint64_t *ret, uint64_t clan, int32_t i) {
    (void)self; (void)clan; (void)i;
    *ret = 0;
    return ret;
}

enum { SLOT_CLAN = 18, OP_JOIN = 1, OP_LEAVE = 2, OP_SEND = 3, OP_MEMBERS = 4, OP_POLL = 5, MSG_MAX = 64, MSG_TEXT = 1024 };
typedef struct { uint64_t clan, sender; uint32_t time; char text[MSG_TEXT]; } ClanMsg;
static ClanMsg g_msgs[MSG_MAX];
static int g_msg_next;
static volatile LONG g_poller;

static size_t clan_request(uint8_t *buf, uint8_t op, uint64_t clan, const void *body, size_t n) {
    buf[0] = op;
    memcpy(buf + 1, &clan, 8);
    if (n) memcpy(buf + 9, body, n);
    return 9 + n;
}

static DWORD WINAPI clan_poll_thread(LPVOID arg) {
    (void)arg;
    for (;;) {
        Sleep(1000);
        uint8_t req[9], *reply = malloc(1 << 16);
        if (!reply) continue;
        size_t n = 0;
        int32_t st = -1;
        size_t len = clan_request(req, OP_POLL, 0, NULL, 0);
        if (bridge(SLOT_CLAN, req, len, &st, reply, 1 << 16, &n, 4000) && st == 0 && n >= 4) {
            uint32_t count;
            memcpy(&count, reply, 4);
            size_t o = 4;
            for (uint32_t i = 0; i < count && o + 22 <= n; i++) {
                ClanMsg m = {0};
                uint16_t tl;
                memcpy(&m.clan, reply + o, 8); memcpy(&m.sender, reply + o + 8, 8); memcpy(&m.time, reply + o + 16, 4); memcpy(&tl, reply + o + 20, 2);
                o += 22;
                if (o + tl > n) break;
                snprintf(m.text, sizeof m.text, "%.*s", (int)tl, (const char *)reply + o);
                o += tl;
                AcquireSRWLockExclusive(&g_clan_lock);
                int id = g_msg_next++;
                g_msgs[id % MSG_MAX] = m;
                ReleaseSRWLockExclusive(&g_clan_lock);
                uint8_t cb[24] = {0};
                put64(cb, 0, m.clan); put64(cb, 8, m.sender); put32(cb, 16, (uint32_t)id);
                shim_post_callback(338, cb, sizeof cb);
            }
        }
        free(reply);
    }
    return 0;
}

typedef struct { uint64_t call, clan; } JoinJob;
static DWORD WINAPI clan_join_thread(LPVOID arg) {
    JoinJob *j = arg;
    uint8_t req[9], reply[16];
    size_t n = 0;
    int32_t st = -1;
    int32_t response = 5;
    size_t len = clan_request(req, OP_JOIN, j->clan, NULL, 0);
    if (bridge(SLOT_CLAN, req, len, &st, reply, sizeof reply, &n, 40000) && st == 0 && n >= 16) {
        uint32_t members;
        uint64_t owner;
        memcpy(&response, reply, 4); memcpy(&members, reply + 4, 4); memcpy(&owner, reply + 8, 8);
        if (response == 1) {
            AcquireSRWLockExclusive(&g_clan_lock);
            int s = clan_slot(j->clan, true);
            if (s >= 0) { g_clans[s].joined = true; g_clans[s].members = members; g_clans[s].owner = owner; }
            ReleaseSRWLockExclusive(&g_clan_lock);
            if (!InterlockedCompareExchange(&g_poller, 1, 0)) { HANDLE h = CreateThread(NULL, 0, clan_poll_thread, NULL, 0, NULL); if (h) CloseHandle(h); }
        }
    }
    uint8_t cb[16] = {0};
    put64(cb, 0, j->clan); put32(cb, 8, (uint32_t)response);
    shim_callresult_complete(j->call, 342, cb, sizeof cb);
    free(j);
    return 0;
}
static uint64_t SHIM_TC fr_join_clan_chat(void *self, uint64_t clan) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    JoinJob *j = calloc(1, sizeof *j);
    if (!j) { shim_callresult_fail_handle(call); return call; }
    j->call = call; j->clan = clan;
    HANDLE h = CreateThread(NULL, 0, clan_join_thread, j, 0, NULL);
    if (!h) clan_join_thread(j); else CloseHandle(h);
    return call;
}

static bool clan_bool(uint8_t op, uint64_t clan, const void *body, size_t body_len) {
    uint8_t req[9 + 1100], reply[4];
    size_t n = 0;
    int32_t st = -1, ok = 0;
    if (body_len > 1100) body_len = 1100;
    size_t len = clan_request(req, op, clan, body, body_len);
    if (bridge(SLOT_CLAN, req, len, &st, reply, sizeof reply, &n, 15000) && st == 0 && n >= 4) memcpy(&ok, reply, 4);
    return ok != 0;
}
static uint64_t SHIM_TC fr_leave_clan_chat(void *self, uint64_t clan) {
    (void)self;
    bool ok = clan_bool(OP_LEAVE, clan, NULL, 0);
    AcquireSRWLockExclusive(&g_clan_lock);
    int s = clan_slot(clan, false);
    if (s >= 0) g_clans[s].joined = false;
    ReleaseSRWLockExclusive(&g_clan_lock);
    return ok;
}
static uint64_t SHIM_TC fr_send_clan_chat(void *self, uint64_t clan, const char *text) {
    (void)self;
    return text && *text ? clan_bool(OP_SEND, clan, text, strlen(text)) : false;
}
static int32_t SHIM_TC fr_clan_chat_member_count(void *self, uint64_t clan) {
    (void)self;
    AcquireSRWLockShared(&g_clan_lock);
    int s = clan_slot(clan, false);
    int32_t n = s >= 0 && g_clans[s].joined ? (int32_t)g_clans[s].members : 0;
    ReleaseSRWLockShared(&g_clan_lock);
    return n;
}
static uint64_t *SHIM_TC fr_chat_member_by_index(void *self, uint64_t *ret, uint64_t clan, int32_t index) {
    (void)self;
    *ret = 0;
    uint8_t req[9], *reply = malloc(1 << 15);
    if (!reply || index < 0) { free(reply); return ret; }
    size_t n = 0;
    int32_t st = -1;
    size_t len = clan_request(req, OP_MEMBERS, clan, NULL, 0);
    if (bridge(SLOT_CLAN, req, len, &st, reply, 1 << 15, &n, 10000) && st == 0 && n >= 4) {
        uint32_t count;
        memcpy(&count, reply, 4);
        if ((uint32_t)index < count && 4 + 8 * ((size_t)index + 1) <= n) memcpy(ret, reply + 4 + 8 * (size_t)index, 8);
    }
    free(reply);
    return ret;
}
static int32_t SHIM_TC fr_get_clan_chat_message(void *self, uint64_t clan, int32_t id, char *txt, int32_t cch, int32_t *type, uint64_t *chatter) {
    (void)self;
    ClanMsg m;
    bool found = false;
    AcquireSRWLockShared(&g_clan_lock);
    if (id >= 0 && id < g_msg_next && g_msg_next - id <= MSG_MAX) { m = g_msgs[id % MSG_MAX]; found = m.clan == clan; }
    ReleaseSRWLockShared(&g_clan_lock);
    if (!found || !txt || cch <= 0) return 0;
    snprintf(txt, (size_t)cch, "%s", m.text);
    if (type) *type = 1;
    if (chatter) *chatter = m.sender;
    return (int32_t)strlen(txt) + 1;
}
static uint64_t SHIM_TC fr_is_clan_chat_admin(void *self, uint64_t clan, uint64_t user) {
    (void)self;
    AcquireSRWLockShared(&g_clan_lock);
    int s = clan_slot(clan, false);
    bool admin = s >= 0 && user != 0 && g_clans[s].owner == user;
    ReleaseSRWLockShared(&g_clan_lock);
    return admin;
}

enum { MAX_BEACONS = 16 };
typedef struct { uint32_t type; uint64_t location; } BeaconLocation;
static struct { uint64_t id, owner; uint32_t slots; BeaconLocation loc; char connect[256], meta[256]; } g_beacons[MAX_BEACONS];
static SRWLOCK g_beacon_lock = SRWLOCK_INIT;

typedef struct { uint32_t slots; BeaconLocation loc; char connect[256], meta[256]; uint64_t id; } BeaconCtx;
static uint64_t first_id_field(const CmReply *r, uint32_t field) {
    PbR in; PbField f;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) if (f.field == field) return f.v;
    return 0;
}
static bool parse_create_beacon(const CmReply *r, void *ctx, void *out, int *size) {
    BeaconCtx *c = ctx;
    if (*size < 16) return false;
    uint64_t id = r->eresult == 1 ? first_id_field(r, 1) : 0;
    memset(out, 0, 16);
    put32(out, 0, (uint32_t)r->eresult); put64(out, 8, id);
    if (id) {
        AcquireSRWLockExclusive(&g_beacon_lock);
        for (int i = 0; i < MAX_BEACONS; i++) if (!g_beacons[i].id) {
            g_beacons[i].id = id; g_beacons[i].owner = my_steamid(); g_beacons[i].slots = c->slots; g_beacons[i].loc = c->loc;
            snprintf(g_beacons[i].connect, sizeof g_beacons[i].connect, "%s", c->connect);
            snprintf(g_beacons[i].meta, sizeof g_beacons[i].meta, "%s", c->meta);
            break;
        }
        ReleaseSRWLockExclusive(&g_beacon_lock);
    }
    *size = 16;
    return true;
}
static uint64_t SHIM_TC party_create(void *self, uint32_t slots, const BeaconLocation *loc, const char *connect, const char *meta) {
    (void)self;
    BeaconCtx *c = calloc(1, sizeof *c);
    if (!c) return 0;
    c->slots = slots;
    if (loc) c->loc = *loc;
    snprintf(c->connect, sizeof c->connect, "%s", connect ? connect : "");
    snprintf(c->meta, sizeof c->meta, "%s", meta ? meta : "");
    PbW w = {0}, l = {0};
    pbw_varint(&w, 1, app_id());
    pbw_varint(&w, 2, slots);
    if (loc) { pbw_varint(&l, 1, loc->type); pbw_varint(&l, 2, loc->location); pbw_msg(&w, 3, &l); pbw_free(&l); }
    pbw_str(&w, 4, c->connect);
    pbw_str(&w, 5, c->meta);
    uint64_t call = shim_callresult_reserve();
    shim_cm_async(call, 5302, 0, true, "Parties.CreateBeacon#1", &w, WAIT_MS, parse_create_beacon, c);
    return call;
}

static bool parse_join_party(const CmReply *r, void *ctx, void *out, int *size) {
    (void)ctx;
    if (*size < 280) return false;
    uint8_t *o = out;
    memset(o, 0, 280);
    uint64_t id = 0;
    PbR in; PbField f;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field == 1) id = f.v;
        else if (f.field == 2 && f.wt == 2) snprintf((char *)o + 24, 256, "%.*s", (int)f.len, (const char *)f.data);
    }
    put32(o, 0, (uint32_t)r->eresult); put64(o, 8, id);
    *size = 280;
    return true;
}
static uint64_t SHIM_TC party_join(void *self, uint64_t beacon) {
    (void)self;
    PbW w = {0};
    pbw_fixed64(&w, 1, my_steamid());
    pbw_varint(&w, 2, beacon);
    uint64_t call = shim_callresult_reserve();
    shim_cm_async(call, 5301, 0, true, "Parties.JoinParty#1", &w, WAIT_MS, parse_join_party, NULL);
    return call;
}

static bool parse_slots(const CmReply *r, void *ctx, void *out, int *size) {
    (void)ctx;
    if (*size < 4) return false;
    put32(out, 0, (uint32_t)r->eresult);
    *size = 4;
    return true;
}
static uint64_t SHIM_TC party_change_slots(void *self, uint64_t beacon, uint32_t slots) {
    (void)self;
    PbW w = {0};
    pbw_varint(&w, 1, beacon);
    pbw_varint(&w, 2, slots);
    uint64_t call = shim_callresult_reserve();
    shim_cm_async(call, 5304, 0, true, "Parties.ChangeNumOpenSlots#1", &w, WAIT_MS, parse_slots, NULL);
    AcquireSRWLockExclusive(&g_beacon_lock);
    for (int i = 0; i < MAX_BEACONS; i++) if (g_beacons[i].id == beacon) g_beacons[i].slots = slots;
    ReleaseSRWLockExclusive(&g_beacon_lock);
    return call;
}
static bool service_ok(const char *method, PbW *w) {
    CmReply r = {0};
    bool ok = shim_cm_service(method, w->p, w->len, 10000, &r) && r.eresult == 1;
    shim_cm_reply_free(&r);
    return ok;
}
static uint64_t SHIM_TC party_destroy(void *self, uint64_t beacon) {
    (void)self;
    PbW w = {0};
    pbw_varint(&w, 1, beacon);
    bool ok = service_ok("Parties.DestroyBeacon#1", &w);
    pbw_free(&w);
    if (ok) {
        AcquireSRWLockExclusive(&g_beacon_lock);
        for (int i = 0; i < MAX_BEACONS; i++) if (g_beacons[i].id == beacon) memset(&g_beacons[i], 0, sizeof g_beacons[i]);
        ReleaseSRWLockExclusive(&g_beacon_lock);
    }
    return ok;
}
static void party_reservation(const char *method, uint64_t beacon, uint64_t user) {
    PbW w = {0};
    pbw_varint(&w, 1, beacon);
    pbw_fixed64(&w, 2, user);
    service_ok(method, &w);
    pbw_free(&w);
}
static void SHIM_TC party_on_reservation(void *self, uint64_t beacon, uint64_t user) { (void)self; party_reservation("Parties.OnReservationCompleted#1", beacon, user); }
static void SHIM_TC party_cancel_reservation(void *self, uint64_t beacon, uint64_t user) { (void)self; party_reservation("Parties.CancelReservation#1", beacon, user); }

static uint32_t SHIM_TC party_num_active(void *self) {
    (void)self;
    uint32_t n = 0;
    AcquireSRWLockShared(&g_beacon_lock);
    for (int i = 0; i < MAX_BEACONS; i++) n += g_beacons[i].id != 0;
    ReleaseSRWLockShared(&g_beacon_lock);
    return n;
}
static uint64_t SHIM_TC party_by_index(void *self, uint32_t index) {
    (void)self;
    uint64_t id = 0;
    AcquireSRWLockShared(&g_beacon_lock);
    for (int i = 0; i < MAX_BEACONS; i++) if (g_beacons[i].id && index-- == 0) { id = g_beacons[i].id; break; }
    ReleaseSRWLockShared(&g_beacon_lock);
    return id;
}
static uint64_t SHIM_TC party_details(void *self, uint64_t beacon, uint64_t *owner, BeaconLocation *loc, char *meta, int cch) {
    (void)self;
    bool found = false;
    AcquireSRWLockShared(&g_beacon_lock);
    for (int i = 0; i < MAX_BEACONS; i++) if (g_beacons[i].id == beacon) {
        if (owner) *owner = g_beacons[i].owner;
        if (loc) *loc = g_beacons[i].loc;
        if (meta && cch > 0) snprintf(meta, (size_t)cch, "%s", g_beacons[i].meta);
        found = true;
        break;
    }
    ReleaseSRWLockShared(&g_beacon_lock);
    return found;
}
static uint64_t SHIM_TC party_num_locations(void *self, uint32_t *n) { (void)self; if (n) *n = 0; return 1; }
static uint64_t SHIM_TC party_locations(void *self, BeaconLocation *list, uint32_t max) { (void)self; (void)list; (void)max; return 1; }
static uint64_t SHIM_TC party_location_data(void *self, BeaconLocation loc, int what, char *out, int cch) {
    (void)self; (void)loc; (void)what;
    if (out && cch > 0) out[0] = 0;
    return 0;
}

enum { SLOT_WEB = 17, OP_FAV_ADD = 1, OP_FAV_REMOVE = 2, OP_MARKET = 3 };
typedef struct { uint64_t call, id; bool add; } FavJob;
static DWORD WINAPI favorite_thread(LPVOID arg) {
    FavJob *j = arg;
    uint8_t req[13], reply[4];
    req[0] = j->add ? OP_FAV_ADD : OP_FAV_REMOVE;
    uint32_t app = app_id();
    memcpy(req + 1, &app, 4);
    memcpy(req + 5, &j->id, 8);
    size_t n = 0;
    int32_t st = -1, ok = 0;
    if (bridge(SLOT_WEB, req, sizeof req, &st, reply, sizeof reply, &n, 25000) && st == 0 && n >= 4) memcpy(&ok, reply, 4);
    uint8_t cb[16] = {0};
    put64(cb, 0, j->id); put32(cb, 8, ok == 1 ? 1 : 2); cb[12] = j->add;
    shim_callresult_complete(j->call, 3407, cb, sizeof cb);
    free(j);
    return 0;
}
static uint64_t favorite(uint64_t id, bool add) {
    uint64_t call = shim_callresult_reserve();
    FavJob *j = calloc(1, sizeof *j);
    if (!j) { shim_callresult_fail_handle(call); return call; }
    j->call = call; j->id = id; j->add = add;
    HANDLE h = CreateThread(NULL, 0, favorite_thread, j, 0, NULL);
    if (!h) favorite_thread(j); else CloseHandle(h);
    return call;
}
static uint64_t SHIM_TC ugc_add_favorite(void *self, uint32_t app, uint64_t id) { (void)self; (void)app; return favorite(id, true); }
static uint64_t SHIM_TC ugc_remove_favorite(void *self, uint32_t app, uint64_t id) { (void)self; (void)app; return favorite(id, false); }

static DWORD WINAPI market_thread(LPVOID arg) {
    uint64_t call = (uint64_t)(uintptr_t)arg;
    uint8_t req[13] = { OP_MARKET }, reply[32];
    size_t n = 0;
    int32_t st = -1;
    if (bridge(SLOT_WEB, req, sizeof req, &st, reply, sizeof reply, &n, 30000) && st == 0 && n >= 20) {
        uint8_t cb[20] = {0};
        cb[0] = reply[0];
        memcpy(cb + 4, reply + 4, 16);
        shim_callresult_complete(call, 166, cb, sizeof cb);
    } else {
        shim_callresult_fail_handle(call);
    }
    return 0;
}
static uint64_t SHIM_TC user_market_eligibility(void *self) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    HANDLE h = CreateThread(NULL, 0, market_thread, (LPVOID)(uintptr_t)call, 0, NULL);
    if (!h) market_thread((LPVOID)(uintptr_t)call); else CloseHandle(h);
    return call;
}

static void jtext(char *dst, size_t cap, const char *s) {
    size_t o = 0;
    for (; s && *s && o + 7 < cap; s++) {
        unsigned char c = (unsigned char)*s;
        if (c == '"' || c == '\\') { dst[o++] = '\\'; dst[o++] = (char)c; }
        else if (c < 0x20) o += (size_t)snprintf(dst + o, cap - o, "\\u%04x", c);
        else dst[o++] = (char)c;
    }
    dst[o] = 0;
}
static void tl_send(const char *kind, const char *title, const char *desc, const char *icon, int priority, float offset, float duration, int clip, uint64_t handle, const char *text) {
    char t[400], d[600], i[200], x[400], json[1900];
    jtext(t, sizeof t, title); jtext(d, sizeof d, desc); jtext(i, sizeof i, icon); jtext(x, sizeof x, text);
    snprintf(json, sizeof json,
             "{\"kind\":\"%s\",\"title\":\"%s\",\"description\":\"%s\",\"icon\":\"%s\",\"priority\":%d,\"offset\":%.3f,\"duration\":%.3f,\"clip_priority\":%d,\"handle\":%llu,\"text\":\"%s\"}",
             kind, t, d, i, priority, offset, duration, clip, (unsigned long long)handle, x);
    shim_overlay_submit("timeline", json, 0);
}
static volatile LONG64 g_tl_handle = 0;
static uint64_t next_handle(void) { return (uint64_t)InterlockedIncrement64(&g_tl_handle); }

static void SHIM_TC tl_tooltip(void *self, const char *desc, float delta) { (void)self; tl_send("tooltip", "", desc, "", 0, delta, 0, 0, 0, desc); }
static void SHIM_TC tl_tooltip_clear(void *self, float delta) { (void)self; tl_send("tooltip_clear", "", "", "", 0, delta, 0, 0, 0, ""); }
static void SHIM_TC tl_game_mode(void *self, int mode) { (void)self; tl_send("game_mode", "", "", "", mode, 0, 0, 0, 0, ""); }
static uint64_t SHIM_TC tl_instant(void *self, const char *title, const char *desc, const char *icon, uint32_t prio, float offset, int clip) {
    (void)self;
    uint64_t h = next_handle();
    tl_send("instant", title, desc, icon, (int)prio, offset, 0, clip, h, "");
    return h;
}
static uint64_t SHIM_TC tl_range(void *self, const char *title, const char *desc, const char *icon, uint32_t prio, float start, float duration, int clip) {
    (void)self;
    uint64_t h = next_handle();
    tl_send("range", title, desc, icon, (int)prio, start, duration, clip, h, "");
    return h;
}
static uint64_t SHIM_TC tl_range_start(void *self, const char *title, const char *desc, const char *icon, uint32_t prio, float start, int clip) {
    (void)self;
    uint64_t h = next_handle();
    tl_send("range_start", title, desc, icon, (int)prio, start, 0, clip, h, "");
    return h;
}
static void SHIM_TC tl_range_update(void *self, uint64_t h, const char *title, const char *desc, const char *icon, uint32_t prio, int clip) { (void)self; tl_send("range_update", title, desc, icon, (int)prio, 0, 0, clip, h, ""); }
static void SHIM_TC tl_range_end(void *self, uint64_t h, float end) { (void)self; tl_send("range_end", "", "", "", 0, end, 0, 0, h, ""); }
static void SHIM_TC tl_remove(void *self, uint64_t h) { (void)self; tl_send("remove", "", "", "", 0, 0, 0, 0, h, ""); }
static void SHIM_TC tl_phase_start(void *self) { (void)self; tl_send("phase_start", "", "", "", 0, 0, 0, 0, 0, ""); }
static void SHIM_TC tl_phase_end(void *self) { (void)self; tl_send("phase_end", "", "", "", 0, 0, 0, 0, 0, ""); }
static void SHIM_TC tl_phase_id(void *self, const char *id) { (void)self; tl_send("phase_id", "", "", "", 0, 0, 0, 0, 0, id); }
static void SHIM_TC tl_phase_tag(void *self, const char *name, const char *icon, const char *group, uint32_t prio) { (void)self; tl_send("phase_tag", name, group, icon, (int)prio, 0, 0, 0, 0, ""); }
static void SHIM_TC tl_phase_attribute(void *self, const char *group, const char *value, uint32_t prio) { (void)self; tl_send("phase_attribute", group, value, "", (int)prio, 0, 0, 0, 0, ""); }
static void SHIM_TC tl_open_phase(void *self, const char *id) { (void)self; tl_send("open_phase", "", "", "", 0, 0, 0, 0, 0, id); }
static void SHIM_TC tl_open_event(void *self, uint64_t h) { (void)self; tl_send("open_event", "", "", "", 0, 0, 0, 0, h, ""); }
static uint64_t SHIM_TC tl_event_exists(void *self, uint64_t handle) { (void)self; uint8_t cb[16] = {0}; put64(cb, 0, handle); return done(6002, cb, sizeof cb); }
static uint64_t SHIM_TC tl_phase_exists(void *self, const char *phase) { (void)self; uint8_t cb[72] = {0}; if (phase) snprintf((char *)cb, 64, "%s", phase); return done(6001, cb, sizeof cb); }

enum { CB_HTML_READY = 4501, CB_HTML_START = 4503, CB_HTML_URL = 4505, CB_HTML_FINISHED = 4506, CB_HTML_TITLE = 4508, MAX_BROWSERS = 8 };
static struct { bool used; uint32_t handle; char url[1024], post[256], title[256]; } g_browsers[MAX_BROWSERS];
static SRWLOCK g_html_lock = SRWLOCK_INIT;
static uint32_t g_next_browser = 1;

static uint64_t SHIM_TC html_init(void *self) { (void)self; return 1; }
static uint64_t SHIM_TC html_shutdown(void *self) { (void)self; return 1; }
static uint64_t SHIM_TC html_create(void *self, const char *agent, const char *css) {
    (void)self; (void)agent; (void)css;
    shim_trace("HTMLSurface.CreateBrowser");
    uint32_t handle = 0;
    AcquireSRWLockExclusive(&g_html_lock);
    for (int i = 0; i < MAX_BROWSERS; i++) if (!g_browsers[i].used) { memset(&g_browsers[i], 0, sizeof g_browsers[i]); g_browsers[i].used = true; handle = g_browsers[i].handle = g_next_browser++; break; }
    ReleaseSRWLockExclusive(&g_html_lock);
    if (!handle) { uint64_t call = shim_callresult_reserve(); shim_callresult_fail_handle(call); return call; }
    uint8_t cb[4];
    put32(cb, 0, handle);
    return done(CB_HTML_READY, cb, sizeof cb);
}
static void SHIM_TC html_remove(void *self, uint32_t handle) {
    (void)self;
    AcquireSRWLockExclusive(&g_html_lock);
    for (int i = 0; i < MAX_BROWSERS; i++) if (g_browsers[i].used && g_browsers[i].handle == handle) memset(&g_browsers[i], 0, sizeof g_browsers[i]);
    ReleaseSRWLockExclusive(&g_html_lock);
}
static void SHIM_TC html_load(void *self, uint32_t handle, const char *url, const char *post) {
    (void)self;
    shim_trace("HTMLSurface.LoadURL handle=%u url=%s", handle, url ? url : "");
    AcquireSRWLockExclusive(&g_html_lock);
    for (int i = 0; i < MAX_BROWSERS; i++) if (g_browsers[i].used && g_browsers[i].handle == handle) {
        snprintf(g_browsers[i].url, sizeof g_browsers[i].url, "%s", url ? url : "");
        snprintf(g_browsers[i].post, sizeof g_browsers[i].post, "%s", post ? post : "");
        struct { uint32_t handle; const char *url, *target, *post; uint8_t redirect; } cb = { handle, g_browsers[i].url, "", g_browsers[i].post, 0 };
        shim_post_callback(CB_HTML_START, &cb, sizeof cb);
        break;
    }
    ReleaseSRWLockExclusive(&g_html_lock);
}
static void SHIM_TC html_allow(void *self, uint32_t handle, uint8_t allowed) {
    (void)self;
    shim_trace("HTMLSurface.AllowStartRequest handle=%u allowed=%u", handle, allowed);
    char url[1024] = "";
    AcquireSRWLockExclusive(&g_html_lock);
    for (int i = 0; i < MAX_BROWSERS; i++) if (g_browsers[i].used && g_browsers[i].handle == handle) {
        if (allowed) {
            snprintf(url, sizeof url, "%s", g_browsers[i].url);
            snprintf(g_browsers[i].title, sizeof g_browsers[i].title, "%s", url);
            struct { uint32_t handle; const char *url, *post; uint8_t redirect; const char *title; uint8_t fresh; } changed = { handle, g_browsers[i].url, g_browsers[i].post, 0, g_browsers[i].title, 1 };
            shim_post_callback(CB_HTML_URL, &changed, sizeof changed);
            struct { uint32_t handle; const char *title; } titled = { handle, g_browsers[i].title };
            shim_post_callback(CB_HTML_TITLE, &titled, sizeof titled);
            struct { uint32_t handle; const char *url, *title; } finished = { handle, g_browsers[i].url, g_browsers[i].title };
            shim_post_callback(CB_HTML_FINISHED, &finished, sizeof finished);
        }
        break;
    }
    ReleaseSRWLockExclusive(&g_html_lock);
    if (url[0]) shim_overlay_web_page(url);
}

void register_community(void) {
    const char *f = "SteamFriends0";
    shim_register_family(f, "GetFollowerCount", (void *)fr_follower_count);
    shim_register_family(f, "IsFollowing", (void *)fr_is_following);
    shim_register_family(f, "EnumerateFollowingList", (void *)fr_enumerate_following);
    shim_register_family(f, "RequestClanOfficerList", (void *)fr_clan_officers);
    shim_register_family(f, "GetClanOfficerCount", (void *)fr_clan_officer_count);
    shim_register_family(f, "GetClanOwner", (void *)fr_clan_owner);
    shim_register_family(f, "GetClanOfficerByIndex", (void *)fr_clan_officer_by_index);
    shim_register_family(f, "JoinClanChatRoom", (void *)fr_join_clan_chat);
    shim_register_family(f, "LeaveClanChatRoom", (void *)fr_leave_clan_chat);
    shim_register_family(f, "SendClanChatMessage", (void *)fr_send_clan_chat);
    shim_register_family(f, "GetClanChatMemberCount", (void *)fr_clan_chat_member_count);
    shim_register_family(f, "GetChatMemberByIndex", (void *)fr_chat_member_by_index);
    shim_register_family(f, "GetClanChatMessage", (void *)fr_get_clan_chat_message);
    shim_register_family(f, "IsClanChatAdmin", (void *)fr_is_clan_chat_admin);
    shim_register_family("STEAMREMOTESTORAGE_INTERFACE_VERSION", "FileShare", (void *)rs_file_share);
    shim_register_family("STEAMUGC_INTERFACE_VERSION", "AddItemToFavorites", (void *)ugc_add_favorite);
    shim_register_family("STEAMUGC_INTERFACE_VERSION", "RemoveItemFromFavorites", (void *)ugc_remove_favorite);
    shim_register_family("SteamUser0", "GetMarketEligibility", (void *)user_market_eligibility);
    const char *p = "SteamParties";
    shim_register_family(p, "CreateBeacon", (void *)party_create);
    shim_register_family(p, "JoinParty", (void *)party_join);
    shim_register_family(p, "ChangeNumOpenSlots", (void *)party_change_slots);
    shim_register_family(p, "DestroyBeacon", (void *)party_destroy);
    shim_register_family(p, "OnReservationCompleted", (void *)party_on_reservation);
    shim_register_family(p, "CancelReservation", (void *)party_cancel_reservation);
    shim_register_family(p, "GetNumActiveBeacons", (void *)party_num_active);
    shim_register_family(p, "GetBeaconByIndex", (void *)party_by_index);
    shim_register_family(p, "GetBeaconDetails", (void *)party_details);
    shim_register_family(p, "GetNumAvailableBeaconLocations", (void *)party_num_locations);
    shim_register_family(p, "GetAvailableBeaconLocations", (void *)party_locations);
    shim_register_family(p, "GetBeaconLocationData", (void *)party_location_data);
    const char *t = "STEAMTIMELINE_INTERFACE_V";
    shim_register_family_ab(t, "SetTimelineTooltip", 8, (void *)tl_tooltip);
    shim_register_family_ab(t, "ClearTimelineTooltip", 4, (void *)tl_tooltip_clear);
    shim_register_family_ab(t, "SetTimelineGameMode", 4, (void *)tl_game_mode);
    shim_register_family_ab(t, "AddInstantaneousTimelineEvent", 24, (void *)tl_instant);
    shim_register_family_ab(t, "AddRangeTimelineEvent", 28, (void *)tl_range);
    shim_register_family_ab(t, "StartRangeTimelineEvent", 24, (void *)tl_range_start);
    shim_register_family_ab(t, "UpdateRangeTimelineEvent", 28, (void *)tl_range_update);
    shim_register_family_ab(t, "EndRangeTimelineEvent", 12, (void *)tl_range_end);
    shim_register_family_ab(t, "RemoveTimelineEvent", 8, (void *)tl_remove);
    shim_register_family(t, "DoesEventRecordingExist", (void *)tl_event_exists);
    shim_register_family_ab(t, "StartGamePhase", 0, (void *)tl_phase_start);
    shim_register_family_ab(t, "EndGamePhase", 0, (void *)tl_phase_end);
    shim_register_family_ab(t, "SetGamePhaseID", 4, (void *)tl_phase_id);
    shim_register_family(t, "DoesGamePhaseRecordingExist", (void *)tl_phase_exists);
    shim_register_family_ab(t, "AddGamePhaseTag", 16, (void *)tl_phase_tag);
    shim_register_family_ab(t, "SetGamePhaseAttribute", 12, (void *)tl_phase_attribute);
    shim_register_family_ab(t, "OpenOverlayToGamePhase", 4, (void *)tl_open_phase);
    shim_register_family_ab(t, "OpenOverlayToTimelineEvent", 8, (void *)tl_open_event);
    const char *h = "STEAMHTMLSURFACE_INTERFACE_VERSION_";
    shim_register_family(h, "Init", (void *)html_init);
    shim_register_family(h, "Shutdown", (void *)html_shutdown);
    shim_register_family(h, "CreateBrowser", (void *)html_create);
    shim_register_family(h, "RemoveBrowser", (void *)html_remove);
    shim_register_family(h, "LoadURL", (void *)html_load);
    shim_register_family(h, "AllowStartRequest", (void *)html_allow);
}
