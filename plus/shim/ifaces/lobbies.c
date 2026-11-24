#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "../shim.h"

enum {
    EMSG_CREATE = 6601, EMSG_JOIN = 6603, EMSG_LEAVE = 6605, EMSG_LIST = 6607, EMSG_SET_DATA = 6609, EMSG_GET_DATA = 6611,
    EMSG_SEND_CHAT = 6613, EMSG_PUSH_CHAT = 6614, EMSG_SET_OWNER = 6615, EMSG_SET_SERVER = 6617, EMSG_PUSH_SERVER = 6618,
    EMSG_PUSH_JOINED = 6619, EMSG_PUSH_LEFT = 6620, EMSG_INVITE = 6621, EMSG_LINK = 6624, EMSG_PUSH_DATA = 6612, EMSG_PUSH_INVITE = 800,
    CB_INVITE = 503, CB_KICKED = 512, FLAG_LOCKED = 8, CB_ENTER = 504, CB_UPDATE = 505, CB_CHAT_UPDATE = 506, CB_CHAT_MSG = 507, CB_GAME_CREATED = 509, CB_LIST = 510, CB_CREATED = 513,
    WAIT_MS = 20000, MAX_LOBBIES = 96, MAX_LIST = 256, MAX_FILTERS = 32, CHAT_RING = 16, VALUE_RING = 8, VALUE_MAX = 8192,
};

typedef struct { char *key, *val; } KV;
typedef struct { KV *e; int n; } KVList;
typedef struct { uint64_t id; char *persona; KVList data; } Member;
typedef struct { uint64_t user; int type; uint32_t len; uint8_t *data; } Chat;
typedef struct {
    uint64_t id, owner, server_id;
    bool used, joined, has_server;
    int type, flags, limit, num_members;
    uint32_t server_ip, server_port, next_chat, seq;
    KVList data;
    Member *m;
    int n_m;
    Chat chat[CHAT_RING];
} Lobby;
typedef struct { int type, cmp; char key[256]; char val[256]; } Filter;

static Lobby g_lobbies[MAX_LOBBIES];
static uint32_t g_seq;
static uint64_t g_list[MAX_LIST];
static int g_n_list;
static Filter g_filters[MAX_FILTERS];
static int g_n_filters, g_want_count, g_want_slots;
static SRWLOCK g_lock = SRWLOCK_INIT;
static volatile LONG g_net_started;

static uint64_t self_id(void) { const Snapshot *s = shim_snapshot(); return s ? s->steamid : 0; }
static const char *self_name(void) { const Snapshot *s = shim_snapshot(); return s ? s->persona : ""; }
static uint32_t app_id(void) { const Snapshot *s = shim_snapshot(); return s && s->has_ticket ? s->ticket.appid : 0; }
static int32_t pb_i32(const PbField *f) { return (int32_t)(uint32_t)f->v; }

static void kv_clear(KVList *l) {
    for (int i = 0; i < l->n; i++) { free(l->e[i].key); free(l->e[i].val); }
    free(l->e);
    l->e = NULL;
    l->n = 0;
}

static KV *kv_find(KVList *l, const char *key) {
    for (int i = 0; key && i < l->n; i++) if (!_stricmp(l->e[i].key, key)) return &l->e[i];
    return NULL;
}

static bool kv_set(KVList *l, const char *key, const char *val) {
    if (!key || !key[0] || strlen(key) > 255 || (val && strlen(val) >= VALUE_MAX)) return false;
    KV *e = kv_find(l, key);
    char *v = _strdup(val ? val : "");
    if (!v) return false;
    if (e) { free(e->val); e->val = v; return true; }
    KV *g = realloc(l->e, ((size_t)l->n + 1) * sizeof *g);
    if (!g) { free(v); return false; }
    l->e = g;
    l->e[l->n].key = _strdup(key);
    l->e[l->n].val = v;
    if (!l->e[l->n].key) { free(v); return false; }
    l->n++;
    return true;
}

static void kv_del(KVList *l, const char *key) {
    KV *e = kv_find(l, key);
    if (!e) return;
    free(e->key);
    free(e->val);
    *e = l->e[--l->n];
}

static bool kv_equal(const KVList *a, const KVList *b) {
    if (a->n != b->n) return false;
    for (int i = 0; i < a->n; i++) {
        KV *o = kv_find((KVList *)b, a->e[i].key);
        if (!o || strcmp(o->val, a->e[i].val)) return false;
    }
    return true;
}

static void kv_decode(const uint8_t *p, size_t len, KVList *out) {
    const uint8_t *end = p + len;
    int depth = 0;
    kv_clear(out);
    while (p < end) {
        uint8_t t = *p++;
        if (t == 8) { if (--depth <= 0) break; continue; }
        const uint8_t *k = p;
        while (p < end && *p) p++;
        if (p >= end) break;
        const char *key = (const char *)k;
        p++;
        if (t == 0) { depth++; continue; }
        if (t == 1) {
            const uint8_t *v = p;
            while (p < end && *p) p++;
            if (p >= end) break;
            kv_set(out, key, (const char *)v);
            p++;
        } else if (t == 2 && end - p >= 4) {
            int32_t n;
            char buf[16];
            memcpy(&n, p, 4);
            snprintf(buf, sizeof buf, "%d", n);
            kv_set(out, key, buf);
            p += 4;
        } else {
            break;
        }
    }
}

static void kv_encode(const KVList *l, PbW *w, uint32_t field) {
    static const uint8_t head[2] = { 0, 0 }, tail[2] = { 8, 8 };
    uint8_t one = 1;
    size_t cap = 4;
    for (int i = 0; i < l->n; i++) cap += 3 + strlen(l->e[i].key) + strlen(l->e[i].val);
    uint8_t *b = malloc(cap), *o = b;
    if (!b) return;
    memcpy(o, head, 2); o += 2;
    for (int i = 0; i < l->n; i++) {
        *o++ = one;
        size_t kl = strlen(l->e[i].key), vl = strlen(l->e[i].val);
        memcpy(o, l->e[i].key, kl + 1); o += kl + 1;
        memcpy(o, l->e[i].val, vl + 1); o += vl + 1;
    }
    memcpy(o, tail, 2); o += 2;
    pbw_bytes(w, field, b, (size_t)(o - b));
    free(b);
}

static void free_members(Lobby *l) {
    for (int i = 0; i < l->n_m; i++) { free(l->m[i].persona); kv_clear(&l->m[i].data); }
    free(l->m);
    l->m = NULL;
    l->n_m = 0;
}

static void free_lobby(Lobby *l) {
    kv_clear(&l->data);
    free_members(l);
    for (int i = 0; i < CHAT_RING; i++) free(l->chat[i].data);
    memset(l, 0, sizeof *l);
}

static Lobby *find_locked(uint64_t id) {
    for (int i = 0; i < MAX_LOBBIES; i++) if (g_lobbies[i].used && g_lobbies[i].id == id) return &g_lobbies[i];
    return NULL;
}

static Lobby *get_locked(uint64_t id) {
    Lobby *l = find_locked(id), *victim = NULL;
    if (l) return l;
    for (int i = 0; i < MAX_LOBBIES; i++) {
        Lobby *c = &g_lobbies[i];
        if (!c->used) { victim = c; break; }
        if (!c->joined && (!victim || c->seq < victim->seq)) victim = c;
    }
    if (!victim) return NULL;
    free_lobby(victim);
    victim->used = true;
    victim->id = id;
    victim->seq = ++g_seq;
    return victim;
}

static Member *find_member(Lobby *l, uint64_t id) {
    for (int i = 0; i < l->n_m; i++) if (l->m[i].id == id) return &l->m[i];
    return NULL;
}

static Member *add_member(Lobby *l, uint64_t id, const char *persona) {
    Member *m = find_member(l, id);
    if (m) return m;
    Member *g = realloc(l->m, ((size_t)l->n_m + 1) * sizeof *g);
    if (!g) return NULL;
    l->m = g;
    m = &l->m[l->n_m++];
    memset(m, 0, sizeof *m);
    m->id = id;
    m->persona = _strdup(persona ? persona : "");
    return m;
}

static void drop_member(Lobby *l, uint64_t id) {
    for (int i = 0; i < l->n_m; i++) {
        if (l->m[i].id != id) continue;
        free(l->m[i].persona);
        kv_clear(&l->m[i].data);
        l->m[i] = l->m[--l->n_m];
        return;
    }
}

typedef struct { int32_t max, type, flags, num; uint64_t owner; const uint8_t *meta; size_t meta_len; bool has_meta, has_members; } LobbyInfo;

static void apply_member_msg(Lobby *l, const uint8_t *p, size_t len, bool *changed, uint64_t *who) {
    PbR r;
    PbField f;
    uint64_t id = 0;
    const char *name = "";
    char persona[128] = "";
    const uint8_t *meta = NULL;
    size_t meta_len = 0;
    pbr_init(&r, p, len);
    while (pbr_next(&r, &f)) {
        if (f.field == 1) id = f.v;
        else if (f.field == 2 && f.wt == 2) { size_t n = f.len < sizeof persona - 1 ? f.len : sizeof persona - 1; memcpy(persona, f.data, n); persona[n] = 0; name = persona; }
        else if (f.field == 3 && f.wt == 2) { meta = f.data; meta_len = f.len; }
    }
    if (!id) return;
    Member *m = add_member(l, id, name);
    if (!m) return;
    if (name[0] && (!m->persona || strcmp(m->persona, name))) { free(m->persona); m->persona = _strdup(name); }
    KVList fresh = {0};
    if (meta) kv_decode(meta, meta_len, &fresh);
    *changed = !kv_equal(&fresh, &m->data);
    kv_clear(&m->data);
    m->data = fresh;
    *who = id;
}

static void post_update(uint64_t lobby, uint64_t member, bool ok) {
    uint8_t cb[24] = {0};
    memcpy(cb, &lobby, 8);
    memcpy(cb + 8, &member, 8);
    cb[16] = ok;
    shim_post_callback(CB_UPDATE, cb, sizeof cb);
}

static uint64_t apply_lobby_data(const uint8_t *body, size_t len, bool notify) {
    PbR r;
    PbField f;
    uint64_t id = 0;
    LobbyInfo in = { .max = -1, .type = -1, .flags = -1, .num = -1 };
    pbr_init(&r, body, len);
    while (pbr_next(&r, &f)) if (f.field == 2) id = f.v;
    if (!id) return 0;
    uint64_t changed_members[64];
    int n_changed = 0;
    bool lobby_changed = false;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = get_locked(id);
    if (!l) { ReleaseSRWLockExclusive(&g_lock); return id; }
    pbr_init(&r, body, len);
    while (pbr_next(&r, &f)) {
        switch (f.field) {
        case 3: in.num = pb_i32(&f); break;
        case 4: in.max = pb_i32(&f); break;
        case 5: in.type = pb_i32(&f); break;
        case 6: in.flags = pb_i32(&f); break;
        case 7: in.owner = f.v; break;
        case 8: if (f.wt == 2) { in.meta = f.data; in.meta_len = f.len; in.has_meta = true; } break;
        case 9:
            if (f.wt == 2) {
                bool ch = false;
                uint64_t who = 0;
                if (!in.has_members) in.has_members = true;
                apply_member_msg(l, f.data, f.len, &ch, &who);
                if (ch && who && n_changed < 64) changed_members[n_changed++] = who;
            }
            break;
        default: break;
        }
    }
    if (in.max >= 0) l->limit = in.max;
    if (in.type >= 0) l->type = in.type;
    if (in.flags >= 0) l->flags = in.flags;
    if (in.owner && l->owner != in.owner) { l->owner = in.owner; lobby_changed = true; }
    if (in.num >= 0) l->num_members = in.num;
    if (in.has_meta) {
        KVList fresh = {0};
        kv_decode(in.meta, in.meta_len, &fresh);
        if (!kv_equal(&fresh, &l->data)) lobby_changed = true;
        kv_clear(&l->data);
        l->data = fresh;
    }
    if (in.has_members) l->num_members = l->n_m > l->num_members ? l->n_m : l->num_members;
    ReleaseSRWLockExclusive(&g_lock);
    if (notify) {
        post_update(id, id, true);
        for (int i = 0; i < n_changed; i++) post_update(id, changed_members[i], true);
    }
    (void)lobby_changed;
    return id;
}

static void post_chat_update(uint64_t lobby, uint64_t user, uint32_t state) {
    uint8_t cb[32] = {0};
    memcpy(cb, &lobby, 8);
    memcpy(cb + 8, &user, 8);
    memcpy(cb + 16, &user, 8);
    memcpy(cb + 24, &state, 4);
    shim_post_callback(CB_CHAT_UPDATE, cb, sizeof cb);
}

static void on_membership(const uint8_t *body, size_t len, bool joined) {
    PbR r;
    PbField f;
    uint64_t lobby = 0, user = 0;
    char persona[128] = "";
    pbr_init(&r, body, len);
    while (pbr_next(&r, &f)) {
        if (f.field == 2) lobby = f.v;
        else if (f.field == 3) user = f.v;
        else if (f.field == 4 && f.wt == 2) { size_t n = f.len < sizeof persona - 1 ? f.len : sizeof persona - 1; memcpy(persona, f.data, n); persona[n] = 0; }
    }
    if (!lobby || !user) return;
    bool kicked = false;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(lobby);
    if (l) {
        if (joined) { add_member(l, user, persona); l->num_members = l->n_m; }
        else if (user == self_id() && l->joined) { free_lobby(l); kicked = true; }
        else { drop_member(l, user); l->num_members = l->n_m; }
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (kicked) {
        uint8_t cb[24] = {0};
        memcpy(cb, &lobby, 8);
        shim_post_callback(CB_KICKED, cb, sizeof cb);
        return;
    }
    post_chat_update(lobby, user, joined ? 1 : 2);
}

static void on_invite(const uint8_t *body, size_t len) {
    PbR r;
    PbField f;
    uint64_t invited = 0, chat = 0, patron = 0, game = 0;
    pbr_init(&r, body, len);
    while (pbr_next(&r, &f)) {
        if (f.field == 1) invited = f.v;
        else if (f.field == 2) chat = f.v;
        else if (f.field == 3) patron = f.v;
        else if (f.field == 7) game = f.v;
    }
    bool lobby = ((chat >> 52) & 0xF) == 8 && ((chat >> 32) & 0x40000);
    if (!lobby || (invited && invited != self_id())) return;
    uint8_t cb[24];
    memcpy(cb, &patron, 8);
    memcpy(cb + 8, &chat, 8);
    memcpy(cb + 16, &game, 8);
    shim_post_callback(CB_INVITE, cb, sizeof cb);
    char json[96];
    snprintf(json, sizeof json, "{\"inviter\":%llu,\"lobby\":%llu,\"game\":%llu}", (unsigned long long)patron, (unsigned long long)chat, (unsigned long long)game);
    shim_overlay_submit("lobby_invite", json, 0);
}

static void on_chat(const uint8_t *body, size_t len) {
    PbR r;
    PbField f;
    uint64_t lobby = 0, user = 0;
    const uint8_t *msg = NULL;
    size_t msg_len = 0;
    pbr_init(&r, body, len);
    while (pbr_next(&r, &f)) {
        if (f.field == 2) lobby = f.v;
        else if (f.field == 3) user = f.v;
        else if (f.field == 4 && f.wt == 2) { msg = f.data; msg_len = f.len; }
    }
    if (!lobby || !msg) return;
    uint32_t index = 0;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(lobby);
    if (l) {
        Chat *c = &l->chat[l->next_chat % CHAT_RING];
        free(c->data);
        c->data = malloc(msg_len + 1);
        if (c->data) { memcpy(c->data, msg, msg_len); c->len = (uint32_t)msg_len; c->user = user; c->type = 1; index = l->next_chat++; }
        else l = NULL;
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (!l) return;
    uint8_t cb[24] = {0};
    memcpy(cb, &lobby, 8);
    memcpy(cb + 8, &user, 8);
    cb[16] = 1;
    memcpy(cb + 20, &index, 4);
    shim_post_callback(CB_CHAT_MSG, cb, sizeof cb);
}

static void on_game_server(const uint8_t *body, size_t len) {
    PbR r;
    PbField f;
    uint64_t lobby = 0, server = 0;
    uint32_t ip = 0, port = 0;
    pbr_init(&r, body, len);
    while (pbr_next(&r, &f)) {
        if (f.field == 2) lobby = f.v;
        else if (f.field == 3) ip = (uint32_t)f.v;
        else if (f.field == 4) port = (uint32_t)f.v;
        else if (f.field == 5) server = f.v;
    }
    if (!lobby) return;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(lobby);
    if (l) { l->server_ip = ip; l->server_port = port; l->server_id = server; l->has_server = true; }
    ReleaseSRWLockExclusive(&g_lock);
    uint8_t cb[24] = {0};
    memcpy(cb, &lobby, 8);
    memcpy(cb + 8, &server, 8);
    memcpy(cb + 16, &ip, 4);
    uint16_t p16 = (uint16_t)port;
    memcpy(cb + 20, &p16, 2);
    shim_post_callback(CB_GAME_CREATED, cb, sizeof cb);
}

static void push_data(const uint8_t *b, size_t n) { apply_lobby_data(b, n, true); }
static void push_chat(const uint8_t *b, size_t n) { on_chat(b, n); }
static void push_server(const uint8_t *b, size_t n) { on_game_server(b, n); }
static void push_joined(const uint8_t *b, size_t n) { on_membership(b, n, true); }
static void push_left(const uint8_t *b, size_t n) { on_membership(b, n, false); }
static void push_invite(const uint8_t *b, size_t n) { on_invite(b, n); }

static void ensure_net(void) {
    if (InterlockedCompareExchange(&g_net_started, 1, 0)) return;
    shim_cm_on_push(EMSG_PUSH_DATA, push_data);
    shim_cm_on_push(EMSG_PUSH_CHAT, push_chat);
    shim_cm_on_push(EMSG_PUSH_SERVER, push_server);
    shim_cm_on_push(EMSG_PUSH_JOINED, push_joined);
    shim_cm_on_push(EMSG_PUSH_LEFT, push_left);
    shim_cm_on_push(EMSG_PUSH_INVITE, push_invite);
}

typedef struct { uint64_t lobby; int type, limit; } EnterCtx;

static void post_enter(uint64_t id, int response, bool locked) {
    uint8_t cb[24] = {0};
    memcpy(cb, &id, 8);
    cb[12] = locked;
    memcpy(cb + 16, &response, 4);
    shim_post_callback(CB_ENTER, cb, sizeof cb);
}

static bool parse_created(const CmReply *r, void *ctx, void *out, int *size) {
    EnterCtx *c = ctx;
    PbR in;
    PbField f;
    uint64_t id = 0;
    int32_t eresult = 2;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field == 2) id = f.v;
        else if (f.field == 3) eresult = pb_i32(&f);
    }
    if (in.bad || *size < 16) return false;
    if (eresult == 1 && id) {
        AcquireSRWLockExclusive(&g_lock);
        Lobby *l = get_locked(id);
        if (l) {
            l->joined = true; l->owner = self_id(); l->type = c->type; l->limit = c->limit; l->num_members = 1;
            add_member(l, self_id(), self_name());
        }
        ReleaseSRWLockExclusive(&g_lock);
        post_enter(id, 1, false);
    }
    memset(out, 0, 16);
    memcpy(out, &eresult, 4);
    memcpy((uint8_t *)out + 8, &id, 8);
    *size = 16;
    return true;
}

static uint64_t SHIM_TC create_lobby(void *self, int type, int max_members) {
    (void)self;
    ensure_net();
    uint64_t h = shim_callresult_reserve();
    EnterCtx *c = calloc(1, sizeof *c);
    if (!c) { shim_callresult_fail_handle(h); return h; }
    c->type = type;
    c->limit = max_members;
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_varint(&w, 2, (uint64_t)(int64_t)max_members);
    pbw_varint(&w, 3, (uint64_t)(int64_t)type);
    pbw_varint(&w, 4, 0);
    pbw_str(&w, 8, self_name());
    shim_cm_async(h, CB_CREATED, EMSG_CREATE, false, NULL, &w, WAIT_MS, parse_created, c);
    return h;
}

static bool parse_joined(const CmReply *r, void *ctx, void *out, int *size) {
    EnterCtx *c = ctx;
    PbR in;
    PbField f;
    int32_t response = 5;
    if (*size < 24) return false;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) if (f.field == 3) response = pb_i32(&f);
    if (in.bad) return false;
    bool locked = false;
    if (response == 1) {
        apply_lobby_data(r->body, r->len, false);
        AcquireSRWLockExclusive(&g_lock);
        Lobby *l = find_locked(c->lobby);
        if (l) {
            l->joined = true;
            if (!find_member(l, self_id())) add_member(l, self_id(), self_name());
            l->num_members = l->n_m;
            locked = (l->flags & FLAG_LOCKED) != 0;
        }
        ReleaseSRWLockExclusive(&g_lock);
    }
    memset(out, 0, 24);
    memcpy(out, &c->lobby, 8);
    ((uint8_t *)out)[12] = locked;
    memcpy((uint8_t *)out + 16, &response, 4);
    *size = 24;
    shim_post_callback(CB_ENTER, out, 24);
    return true;
}

static uint64_t SHIM_TC join_lobby(void *self, uint64_t id) {
    (void)self;
    shim_trace("JoinLobby %llu", (unsigned long long)id);
    ensure_net();
    uint64_t h = shim_callresult_reserve();
    EnterCtx *c = calloc(1, sizeof *c);
    if (!c) { shim_callresult_fail_handle(h); return h; }
    c->lobby = id;
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_fixed64(&w, 2, id);
    pbw_str(&w, 3, self_name());
    shim_cm_async(h, CB_ENTER, EMSG_JOIN, false, NULL, &w, WAIT_MS, parse_joined, c);
    return h;
}

static void send_async(int32_t emsg, PbW *w) { shim_cm_post(emsg, w); }

static void SHIM_TC leave_lobby(void *self, uint64_t id) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    if (l) free_lobby(l);
    ReleaseSRWLockExclusive(&g_lock);
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_fixed64(&w, 2, id);
    send_async(EMSG_LEAVE, &w);
    shim_cm_flush(2000);
}

enum { FILTER_STRING = 0, FILTER_NUMERICAL = 1, FILTER_SLOTS = 2, FILTER_NEAR = 3, FILTER_DISTANCE = 4 };

static void add_filter(int type, const char *key, const char *val, int cmp) {
    AcquireSRWLockExclusive(&g_lock);
    if (g_n_filters < MAX_FILTERS && key) {
        Filter *f = &g_filters[g_n_filters++];
        f->type = type;
        f->cmp = cmp;
        snprintf(f->key, sizeof f->key, "%s", key);
        snprintf(f->val, sizeof f->val, "%s", val ? val : "");
    }
    ReleaseSRWLockExclusive(&g_lock);
}

static void SHIM_TC string_filter(void *self, const char *key, const char *val, int cmp) { (void)self; add_filter(FILTER_STRING, key, val, cmp); }
static void SHIM_TC numerical_filter(void *self, const char *key, int val, int cmp) { (void)self; char b[16]; snprintf(b, sizeof b, "%d", val); add_filter(FILTER_NUMERICAL, key, b, cmp); }
static void SHIM_TC near_filter(void *self, const char *key, int val) { (void)self; char b[16]; snprintf(b, sizeof b, "%d", val); add_filter(FILTER_NEAR, key, b, 0); }
static void SHIM_TC slots_filter(void *self, int slots) { (void)self; char b[16]; snprintf(b, sizeof b, "%d", slots); add_filter(FILTER_SLOTS, "", b, 0); g_want_slots = slots; }
static void SHIM_TC distance_filter(void *self, int distance) { (void)self; char b[16]; snprintf(b, sizeof b, "%d", distance); add_filter(FILTER_DISTANCE, "", b, 0); }
static void SHIM_TC count_filter(void *self, int count) { (void)self; g_want_count = count; }
static void SHIM_TC ignored_filter(void *self) { (void)self; }

static bool parse_list(const CmReply *r, void *ctx, void *out, int *size) {
    int slots = (int)(intptr_t)ctx;
    PbR in;
    PbField f;
    int32_t eresult = 2;
    uint64_t ids[MAX_LIST];
    int n = 0;
    if (*size < 4) return false;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field == 3) eresult = pb_i32(&f);
        else if (f.field == 4 && f.wt == 2 && n < MAX_LIST) {
            PbR lin;
            PbField g;
            uint64_t id = 0;
            int32_t max = 0, type = 0, flags = 0, members = 0;
            const uint8_t *meta = NULL;
            size_t meta_len = 0;
            pbr_init(&lin, f.data, f.len);
            while (pbr_next(&lin, &g)) {
                if (g.field == 1) id = g.v;
                else if (g.field == 2) max = pb_i32(&g);
                else if (g.field == 3) type = pb_i32(&g);
                else if (g.field == 4) flags = pb_i32(&g);
                else if (g.field == 5 && g.wt == 2) { meta = g.data; meta_len = g.len; }
                else if (g.field == 6) members = pb_i32(&g);
            }
            if (!id || (slots > 0 && max - members < slots)) continue;
            AcquireSRWLockExclusive(&g_lock);
            Lobby *l = get_locked(id);
            if (l) {
                l->type = type; l->flags = flags; l->limit = max; l->num_members = members;
                if (meta) { kv_decode(meta, meta_len, &l->data); }
                ids[n++] = id;
            }
            ReleaseSRWLockExclusive(&g_lock);
        }
    }
    if (in.bad) return false;
    shim_trace("RequestLobbyList response eresult=%d lobbies=%d", eresult, n);
    uint32_t count = 0;
    if (eresult == 1) {
        AcquireSRWLockExclusive(&g_lock);
        memcpy(g_list, ids, (size_t)n * sizeof *ids);
        g_n_list = n;
        ReleaseSRWLockExclusive(&g_lock);
        count = (uint32_t)n;
    }
    memcpy(out, &count, 4);
    *size = 4;
    return true;
}

static uint64_t SHIM_TC request_lobby_list(void *self) {
    (void)self;
    ensure_net();
    uint64_t h = shim_callresult_reserve();
    PbW w = {0};
    Filter fl[MAX_FILTERS];
    int n, want, slots;
    AcquireSRWLockExclusive(&g_lock);
    n = g_n_filters;
    memcpy(fl, g_filters, (size_t)n * sizeof *fl);
    want = g_want_count;
    slots = g_want_slots;
    g_n_filters = g_want_count = g_want_slots = 0;
    ReleaseSRWLockExclusive(&g_lock);
    pbw_varint(&w, 1, app_id());
    pbw_varint(&w, 3, (uint64_t)(want > 0 ? want : 50));
    pbw_varint(&w, 4, 0);
    for (int i = 0; i < n; i++) shim_trace("RequestLobbyList filter %s %s (cmp %d type %d)", fl[i].key, fl[i].val, fl[i].cmp, fl[i].type);
    shim_trace("RequestLobbyList app=%u max=%d slots=%d", app_id(), want, slots);
    for (int i = 0; i < n; i++) {
        PbW f = {0};
        pbw_str(&f, 1, fl[i].key);
        pbw_str(&f, 2, fl[i].val);
        pbw_varint(&f, 3, (uint64_t)(int64_t)fl[i].cmp);
        pbw_varint(&f, 4, (uint64_t)fl[i].type);
        pbw_msg(&w, 6, &f);
        pbw_free(&f);
    }
    shim_cm_async(h, CB_LIST, EMSG_LIST, false, NULL, &w, WAIT_MS, parse_list, (void *)(intptr_t)slots);
    return h;
}

static uint64_t *SHIM_TC lobby_by_index(void *self, uint64_t *ret, int i) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    *ret = i >= 0 && i < g_n_list ? g_list[i] : 0;
    ReleaseSRWLockExclusive(&g_lock);
    return ret;
}

static const char *copy_out(const char *s) {
    static char ring[VALUE_RING][VALUE_MAX];
    static LONG turn;
    char *out = ring[(unsigned)InterlockedIncrement(&turn) % VALUE_RING];
    snprintf(out, VALUE_MAX, "%s", s ? s : "");
    return out;
}

static int SHIM_TC num_members(void *self, uint64_t id) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    int n = l ? (l->n_m > l->num_members ? l->n_m : l->num_members) : 0;
    ReleaseSRWLockExclusive(&g_lock);
    return n;
}

static uint64_t *SHIM_TC member_by_index(void *self, uint64_t *ret, uint64_t id, int i) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    *ret = l && i >= 0 && i < l->n_m ? l->m[i].id : 0;
    ReleaseSRWLockExclusive(&g_lock);
    return ret;
}

static uint64_t *SHIM_TC lobby_owner(void *self, uint64_t *ret, uint64_t id) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    *ret = l ? l->owner : 0;
    ReleaseSRWLockExclusive(&g_lock);
    return ret;
}

static const char *SHIM_TC get_lobby_data(void *self, uint64_t id, const char *key) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    KV *e = l ? kv_find(&l->data, key) : NULL;
    const char *out = copy_out(e ? e->val : "");
    ReleaseSRWLockExclusive(&g_lock);
    return out;
}

static int SHIM_TC lobby_data_count(void *self, uint64_t id) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    int n = l ? l->data.n : 0;
    ReleaseSRWLockExclusive(&g_lock);
    return n;
}

static uint64_t SHIM_TC lobby_data_by_index(void *self, uint64_t id, int i, char *key, int cb_key, char *val, int cb_val) {
    (void)self;
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    if (l && i >= 0 && i < l->data.n) {
        if (key && cb_key > 0) snprintf(key, (size_t)cb_key, "%s", l->data.e[i].key);
        if (val && cb_val > 0) snprintf(val, (size_t)cb_val, "%s", l->data.e[i].val);
        ok = true;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static const char *SHIM_TC get_member_data(void *self, uint64_t id, uint64_t member, const char *key) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    Member *m = l ? find_member(l, member) : NULL;
    KV *e = m ? kv_find(&m->data, key) : NULL;
    const char *out = l ? copy_out(e ? e->val : "") : NULL;
    ReleaseSRWLockExclusive(&g_lock);
    return out;
}

static const char *SHIM_TC get_member_data_v001(void *self, uint64_t id, uint64_t member, const char *key) {
    const char *out = get_member_data(self, id, member, key);
    return out ? out : copy_out("");
}

static int SHIM_TC get_chat_entry(void *self, uint64_t id, int index, uint64_t *user, void *data, int cap, int *type) {
    (void)self;
    int n = 0;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    if (l && index >= 0 && (uint32_t)index < l->next_chat && (uint32_t)index + CHAT_RING >= l->next_chat) {
        Chat *c = &l->chat[index % CHAT_RING];
        n = (int)c->len < cap ? (int)c->len : cap;
        if (user) *user = c->user;
        if (type) *type = c->type;
        if (data && n > 0) memcpy(data, c->data, (size_t)n);
    }
    ReleaseSRWLockExclusive(&g_lock);
    return n;
}

static uint64_t SHIM_TC get_game_server(void *self, uint64_t id, uint32_t *ip, uint16_t *port, uint64_t *server) {
    (void)self;
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    if (l && l->has_server) {
        if (ip) *ip = l->server_ip;
        if (port) *port = (uint16_t)l->server_port;
        if (server) *server = l->server_id;
        ok = true;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static int SHIM_TC get_member_limit(void *self, uint64_t id) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    int n = l ? l->limit : 0;
    ReleaseSRWLockExclusive(&g_lock);
    return n;
}

static void push_lobby_state(uint64_t id) {
    PbW w = {0};
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    if (!l) { ReleaseSRWLockExclusive(&g_lock); return; }
    pbw_varint(&w, 1, app_id());
    pbw_fixed64(&w, 2, id);
    pbw_fixed64(&w, 3, 0);
    pbw_varint(&w, 4, (uint64_t)(int64_t)l->limit);
    pbw_varint(&w, 5, (uint64_t)(int64_t)l->type);
    pbw_varint(&w, 6, (uint64_t)(int64_t)l->flags);
    kv_encode(&l->data, &w, 7);
    ReleaseSRWLockExclusive(&g_lock);
    send_async(EMSG_SET_DATA, &w);
}

static uint64_t SHIM_TC set_lobby_data(void *self, uint64_t id, const char *key, const char *val) {
    (void)self;
    bool ok;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    ok = l && l->joined && l->owner == self_id() && kv_set(&l->data, key, val);
    ReleaseSRWLockExclusive(&g_lock);
    if (ok) push_lobby_state(id);
    return ok;
}

static uint64_t SHIM_TC delete_lobby_data(void *self, uint64_t id, const char *key) {
    (void)self;
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    if (l && l->joined && l->owner == self_id() && kv_find(&l->data, key)) { kv_del(&l->data, key); ok = true; }
    ReleaseSRWLockExclusive(&g_lock);
    if (ok) push_lobby_state(id);
    return ok;
}

static void SHIM_TC set_member_data(void *self, uint64_t id, const char *key, const char *val) {
    (void)self;
    PbW w = {0};
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    Member *m = l && l->joined ? find_member(l, self_id()) : NULL;
    if (m && kv_set(&m->data, key, val)) {
        pbw_varint(&w, 1, app_id());
        pbw_fixed64(&w, 2, id);
        pbw_fixed64(&w, 3, self_id());
        kv_encode(&m->data, &w, 7);
        ok = true;
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (ok) send_async(EMSG_SET_DATA, &w);
}

static uint64_t SHIM_TC request_lobby_data(void *self, uint64_t id) {
    (void)self;
    shim_trace("RequestLobbyData %llu", (unsigned long long)id);
    if (!id) return 0;
    ensure_net();
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_fixed64(&w, 2, id);
    send_async(EMSG_GET_DATA, &w);
    return 1;
}

static DWORD WINAPI friends_lobbies_thread(LPVOID unused) {
    (void)unused;
    static uint8_t reply[4 + 16 * 128];
    uint8_t req = 7;
    size_t n = 0;
    int32_t status = -1;
    if (!bridge_call_fresh(0xFFFF, 13, &req, 1, &status, reply, sizeof reply, &n, 5000) || status != 0 || n < 4) return 0;
    uint32_t count;
    memcpy(&count, reply, 4);
    if (count > 128 || n < 4 + 16 * (size_t)count) return 0;
    for (uint32_t i = 0; i < count; i++) {
        uint64_t lobby;
        memcpy(&lobby, reply + 12 + 16 * i, 8);
        request_lobby_data(NULL, lobby);
    }
    return 0;
}

static uint64_t SHIM_TC request_friends_lobbies(void *self) {
    (void)self;
    HANDLE t = CreateThread(NULL, 0, friends_lobbies_thread, NULL, 0, NULL);
    if (t) CloseHandle(t);
    return true;
}

static uint64_t SHIM_TC send_chat(void *self, uint64_t id, const void *data, int cb) {
    (void)self;
    shim_trace("SendLobbyChatMsg lobby=%llu bytes=%d", (unsigned long long)id, cb);
    bool known;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    known = l && l->joined;
    ReleaseSRWLockExclusive(&g_lock);
    if (!known || !data || cb <= 0 || cb > 4096) return 0;
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_fixed64(&w, 2, id);
    pbw_fixed64(&w, 3, 0);
    pbw_bytes(&w, 4, data, (size_t)cb);
    send_async(EMSG_SEND_CHAT, &w);
    return 1;
}

static uint64_t SHIM_TC set_lobby_owner(void *self, uint64_t id, uint64_t owner) {
    (void)self;
    bool ok;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    ok = l && l->joined && l->owner == self_id();
    ReleaseSRWLockExclusive(&g_lock);
    if (!ok) return 0;
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_fixed64(&w, 2, id);
    pbw_fixed64(&w, 3, owner);
    send_async(EMSG_SET_OWNER, &w);
    return 1;
}

static uint64_t SHIM_TC invite_user(void *self, uint64_t id, uint64_t user) {
    (void)self;
    bool known;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    known = l && l->joined;
    ReleaseSRWLockExclusive(&g_lock);
    if (!known) return 0;
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_fixed64(&w, 2, id);
    pbw_fixed64(&w, 3, user);
    send_async(EMSG_INVITE, &w);
    return 1;
}

static void SHIM_TC set_game_server(void *self, uint64_t id, uint32_t ip, uint32_t port, uint64_t server) {
    (void)self;
    bool known;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    known = l && l->joined;
    if (known) { l->server_ip = ip; l->server_port = port; l->server_id = server; l->has_server = true; }
    ReleaseSRWLockExclusive(&g_lock);
    if (!known) return;
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_fixed64(&w, 2, id);
    pbw_varint(&w, 3, ip);
    pbw_varint(&w, 4, port);
    pbw_fixed64(&w, 5, server);
    send_async(EMSG_SET_SERVER, &w);
}

static uint64_t change_setting(uint64_t id, int which, int value) {
    bool ok;
    AcquireSRWLockExclusive(&g_lock);
    Lobby *l = find_locked(id);
    ok = l && l->joined && l->owner == self_id();
    if (ok) {
        if (which == 0) l->limit = value;
        else if (which == 1) l->type = value;
        else l->flags = value ? (l->flags & ~FLAG_LOCKED) : (l->flags | FLAG_LOCKED);
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (ok) push_lobby_state(id);
    return ok;
}
static uint64_t SHIM_TC set_member_limit(void *self, uint64_t id, int limit) { (void)self; return change_setting(id, 0, limit); }
static uint64_t SHIM_TC set_lobby_type(void *self, uint64_t id, int type) { (void)self; return change_setting(id, 1, type); }
static uint64_t SHIM_TC set_joinable(void *self, uint64_t id, uint8_t joinable) { (void)self; return change_setting(id, 2, joinable); }

static uint64_t SHIM_TC set_linked_lobby(void *self, uint64_t id, uint64_t other) {
    (void)self;
    if (!id) return 0;
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_fixed64(&w, 2, id);
    pbw_fixed64(&w, 3, other);
    send_async(EMSG_LINK, &w);
    return 1;
}

void register_lobbies(void) {
    const char *p = "SteamMatchMaking0";
    shim_register_family(p, "CreateLobby", (void *)create_lobby);
    shim_register_family(p, "JoinLobby", (void *)join_lobby);
    shim_register_family(p, "LeaveLobby", (void *)leave_lobby);
    shim_register_family(p, "RequestLobbyList", (void *)request_lobby_list);
    shim_register_family(p, "AddRequestLobbyListStringFilter", (void *)string_filter);
    shim_register_family(p, "AddRequestLobbyListNumericalFilter", (void *)numerical_filter);
    shim_register_family(p, "AddRequestLobbyListNearValueFilter", (void *)near_filter);
    shim_register_family(p, "AddRequestLobbyListFilterSlotsAvailable", (void *)slots_filter);
    shim_register_family(p, "AddRequestLobbyListResultCountFilter", (void *)count_filter);
    shim_register_family(p, "AddRequestLobbyListDistanceFilter", (void *)distance_filter);
    shim_register_family(p, "AddRequestLobbyListCompatibleMembersFilter", (void *)ignored_filter);
    shim_register_family(p, "GetLobbyByIndex", (void *)lobby_by_index);
    shim_register_family(p, "GetNumLobbyMembers", (void *)num_members);
    shim_register_family(p, "GetLobbyMemberByIndex", (void *)member_by_index);
    shim_register_family(p, "GetLobbyOwner", (void *)lobby_owner);
    shim_register_family(p, "SetLobbyOwner", (void *)set_lobby_owner);
    shim_register_family(p, "InviteUserToLobby", (void *)invite_user);
    shim_register_family(p, "GetLobbyData", (void *)get_lobby_data);
    shim_register_family(p, "SetLobbyData", (void *)set_lobby_data);
    shim_register_family(p, "GetLobbyDataCount", (void *)lobby_data_count);
    shim_register_family(p, "GetLobbyDataByIndex", (void *)lobby_data_by_index);
    shim_register_family(p, "DeleteLobbyData", (void *)delete_lobby_data);
    shim_register_family(p, "GetLobbyMemberData", (void *)get_member_data);
    shim_register("SteamMatchMaking001", "GetLobbyMemberData", (void *)get_member_data_v001);
    shim_register_family(p, "SetLobbyMemberData", (void *)set_member_data);
    shim_register_family(p, "RequestLobbyData", (void *)request_lobby_data);
    shim_register_family(p, "RequestFriendsLobbies", (void *)request_friends_lobbies);
    shim_register_family(p, "SendLobbyChatMsg", (void *)send_chat);
    shim_register_family(p, "GetLobbyChatEntry", (void *)get_chat_entry);
    shim_register_family(p, "SetLobbyGameServer", (void *)set_game_server);
    shim_register_family(p, "GetLobbyGameServer", (void *)get_game_server);
    shim_register_family(p, "SetLobbyMemberLimit", (void *)set_member_limit);
    shim_register_family(p, "GetLobbyMemberLimit", (void *)get_member_limit);
    shim_register_family(p, "SetLobbyType", (void *)set_lobby_type);
    shim_register_family(p, "SetLobbyJoinable", (void *)set_joinable);
    shim_register_family(p, "SetLinkedLobby", (void *)set_linked_lobby);
    ensure_net();
}
