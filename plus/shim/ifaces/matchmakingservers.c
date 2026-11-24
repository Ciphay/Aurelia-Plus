#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>

#include "../shim.h"

enum {
    MAX_REQUESTS = 16, MAX_SERVERS = 1024, MAX_QUERIES = 64, ITEM_SIZE = 372,
    RESPONSE_OK = 0, RESPONSE_FAILED = 1, RESPONSE_NO_SERVERS = 2,
    KIND_INTERNET = 0, KIND_LAN = 1, KIND_EMPTY = 2, KIND_FAVORITES = 3, KIND_HISTORY = 4, KIND_FRIENDS = 5, KIND_SPECTATOR = 6,
    EMSG_SERVER_QUERY = 6403, PING_TIMEOUT_MS = 2500, INFO_WANT = 512,
};
#define QUERY_INVALID ((uint32_t)-1)

#define IT_CONN_PORT 0
#define IT_QUERY_PORT 2
#define IT_IP 4
#define IT_PING 8
#define IT_OK 12
#define IT_NOREFRESH 13
#define IT_DIR 14
#define IT_MAP 46
#define IT_DESC 78
#define IT_APP 144
#define IT_PLAYERS 148
#define IT_MAX 152
#define IT_BOTS 156
#define IT_PASSWORD 160
#define IT_SECURE 161
#define IT_TIME 164
#define IT_VERSION 168
#define IT_NAME 172
#define IT_TAGS 236
#define IT_STEAMID 364

typedef struct {
    uintptr_t handle;
    void *response;
    int kind, generation;
    bool used, refreshing, cancelled;
    uint32_t app;
    char filter[2048];
    uint8_t *items;
    int n;
} Request;

typedef struct { uint32_t handle; int kind; void *response; bool used, cancelled; } Query;

static Request g_req[MAX_REQUESTS];
static Query g_query[MAX_QUERIES];
static uintptr_t g_next = 0x100;
static uint32_t g_next_query = 1;
static SRWLOCK g_lock = SRWLOCK_INIT;

static void put16(uint8_t *p, uint16_t v) { memcpy(p, &v, 2); }
static void put32(uint8_t *p, uint32_t v) { memcpy(p, &v, 4); }
static void put_str(uint8_t *item, int off, int cap, const char *s) { snprintf((char *)item + off, (size_t)cap, "%s", s ? s : ""); }

static const uint8_t A2S_INFO[] = { 0xFF, 0xFF, 0xFF, 0xFF, 'T', 'S', 'o', 'u', 'r', 'c', 'e', ' ', 'E', 'n', 'g', 'i', 'n', 'e', ' ', 'Q', 'u', 'e', 'r', 'y', 0 };

static void init_winsock(void) {
    static bool done;
    if (!done) { WSADATA d; WSAStartup(MAKEWORD(2, 2), &d); done = true; }
}

static const char *take_str(const uint8_t **p, const uint8_t *end) {
    const char *s = (const char *)*p;
    while (*p < end && **p) (*p)++;
    if (*p >= end) return NULL;
    (*p)++;
    return s;
}

static bool parse_info(const uint8_t *b, int n, uint8_t *item, int rtt_ms) {
    if (n < 6 || b[4] != 'I') return false;
    const uint8_t *p = b + 6, *end = b + n;
    const char *name = take_str(&p, end), *map = take_str(&p, end), *dir = take_str(&p, end), *game = take_str(&p, end);
    if (!name || !map || !dir || !game || end - p < 9) return false;
    uint16_t app16;
    memcpy(&app16, p, 2);
    p += 2;
    int players = *p++, max = *p++, bots = *p++;
    p++; p++;
    int visibility = *p++, vac = *p++;
    const char *version = take_str(&p, end);
    uint32_t app = app16;
    uint16_t game_port = 0;
    uint64_t steamid = 0;
    const char *tags = "";
    if (p < end) {
        uint8_t edf = *p++;
        if ((edf & 0x80) && end - p >= 2) { memcpy(&game_port, p, 2); p += 2; }
        if ((edf & 0x10) && end - p >= 8) { memcpy(&steamid, p, 8); p += 8; }
        if (edf & 0x40) { if (end - p >= 2) p += 2; take_str(&p, end); }
        if (edf & 0x20) { const char *t = take_str(&p, end); if (t) tags = t; }
        if ((edf & 0x01) && end - p >= 8) { uint64_t gid; memcpy(&gid, p, 8); app = (uint32_t)(gid & 0xFFFFFF); }
    }
    put_str(item, IT_NAME, 64, name);
    put_str(item, IT_MAP, 32, map);
    put_str(item, IT_DIR, 32, dir);
    put_str(item, IT_DESC, 64, game);
    put_str(item, IT_TAGS, 128, tags);
    put32(item + IT_APP, app);
    put32(item + IT_PLAYERS, (uint32_t)players);
    put32(item + IT_MAX, (uint32_t)max);
    put32(item + IT_BOTS, (uint32_t)bots);
    item[IT_PASSWORD] = visibility != 0;
    item[IT_SECURE] = vac != 0;
    if (version && atoi(version) > 0) put32(item + IT_VERSION, (uint32_t)atoi(version));
    if (game_port) put16(item + IT_CONN_PORT, game_port);
    if (steamid) memcpy(item + IT_STEAMID, &steamid, 8);
    put32(item + IT_PING, (uint32_t)(rtt_ms < 1 ? 1 : rtt_ms));
    item[IT_OK] = 1;
    return true;
}

typedef struct { uint8_t *item; struct sockaddr_in to; DWORD sent; bool done, ok, challenged; } Probe;

static void probe_send(SOCKET s, Probe *p, const uint8_t *extra4) {
    uint8_t pkt[sizeof A2S_INFO + 4];
    size_t n = sizeof A2S_INFO;
    memcpy(pkt, A2S_INFO, n);
    if (extra4) { memcpy(pkt + n, extra4, 4); n += 4; }
    sendto(s, (const char *)pkt, (int)n, 0, (struct sockaddr *)&p->to, sizeof p->to);
}

static void probe_batch(Probe *list, int n, int timeout_ms, void (*notify)(void *ctx, int i, bool ok), void *ctx) {
    init_winsock();
    SOCKET s = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    if (s == INVALID_SOCKET) { for (int i = 0; i < n; i++) notify(ctx, i, false); return; }
    if (shim_local_ip_binding()) shim_bind_local_udp((uintptr_t)s, 0, false);
    DWORD start = GetTickCount();
    for (int i = 0; i < n; i++) { list[i].sent = GetTickCount(); probe_send(s, &list[i], NULL); }
    int open = n;
    while (open > 0) {
        int left = timeout_ms - (int)(GetTickCount() - start);
        if (left <= 0) break;
        fd_set rd;
        FD_ZERO(&rd);
        FD_SET(s, &rd);
        struct timeval tv = { left / 1000, (left % 1000) * 1000 };
        if (select(0, &rd, NULL, NULL, &tv) <= 0) break;
        uint8_t buf[1400];
        struct sockaddr_in from;
        int fl = sizeof from;
        int got = recvfrom(s, (char *)buf, sizeof buf, 0, (struct sockaddr *)&from, &fl);
        if (got < 5) continue;
        for (int i = 0; i < n; i++) {
            Probe *p = &list[i];
            if (p->done || p->to.sin_addr.s_addr != from.sin_addr.s_addr || p->to.sin_port != from.sin_port) continue;
            if (buf[4] == 'A' && got >= 9 && !p->challenged) { p->challenged = true; probe_send(s, p, buf + 5); }
            else if (parse_info(buf, got, p->item, (int)(GetTickCount() - p->sent))) { p->done = p->ok = true; open--; notify(ctx, i, true); }
            break;
        }
    }
    closesocket(s);
    for (int i = 0; i < n; i++) if (!list[i].done) notify(ctx, i, false);
}

static int a2s_exchange(uint32_t ip, uint16_t port, uint8_t kind, uint8_t *out, int cap) {
    init_winsock();
    SOCKET s = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    if (s == INVALID_SOCKET) return -1;
    if (shim_local_ip_binding()) shim_bind_local_udp((uintptr_t)s, 0, false);
    struct sockaddr_in to = {0};
    to.sin_family = AF_INET;
    to.sin_port = htons(port);
    to.sin_addr.s_addr = htonl(ip);
    uint8_t req[9] = { 0xFF, 0xFF, 0xFF, 0xFF, kind, 0xFF, 0xFF, 0xFF, 0xFF };
    sendto(s, (const char *)req, sizeof req, 0, (struct sockaddr *)&to, sizeof to);
    int result = -1, total = 0, have = 0;
    uint32_t split_id = 0;
    uint8_t *parts[16] = {0};
    int part_len[16] = {0};
    DWORD start = GetTickCount();
    bool challenged = false;
    for (;;) {
        int left = 3000 - (int)(GetTickCount() - start);
        if (left <= 0) break;
        fd_set rd;
        FD_ZERO(&rd);
        FD_SET(s, &rd);
        struct timeval tv = { left / 1000, (left % 1000) * 1000 };
        if (select(0, &rd, NULL, NULL, &tv) <= 0) break;
        uint8_t buf[1500];
        struct sockaddr_in from;
        int fl = sizeof from;
        int got = recvfrom(s, (char *)buf, sizeof buf, 0, (struct sockaddr *)&from, &fl);
        if (got < 5 || from.sin_addr.s_addr != to.sin_addr.s_addr || from.sin_port != to.sin_port) continue;
        if (buf[0] == 0xFF && buf[1] == 0xFF && buf[2] == 0xFF && buf[3] == 0xFF) {
            if (buf[4] == 'A' && got >= 9 && !challenged) {
                challenged = true;
                memcpy(req + 5, buf + 5, 4);
                sendto(s, (const char *)req, sizeof req, 0, (struct sockaddr *)&to, sizeof to);
            } else if (buf[4] != 'A' && got <= cap) {
                memcpy(out, buf, (size_t)got);
                result = got;
                break;
            }
        } else if (buf[0] == 0xFE && buf[1] == 0xFF && buf[2] == 0xFF && buf[3] == 0xFF && got > 12) {
            uint32_t id;
            memcpy(&id, buf + 4, 4);
            int count = buf[8], number = buf[9];
            if ((id & 0x80000000u) || count < 1 || count > 16 || number >= count) continue;
            if (!total) { total = count; split_id = id; }
            if (id != split_id || parts[number]) continue;
            parts[number] = malloc((size_t)got - 12);
            if (!parts[number]) continue;
            memcpy(parts[number], buf + 12, (size_t)got - 12);
            part_len[number] = got - 12;
            if (++have == total) {
                int sum = 0;
                for (int i = 0; i < total; i++) sum += part_len[i];
                if (sum <= cap) { int o = 0; for (int i = 0; i < total; i++) { memcpy(out + o, parts[i], (size_t)part_len[i]); o += part_len[i]; } result = sum; }
                break;
            }
        }
    }
    for (int i = 0; i < 16; i++) free(parts[i]);
    closesocket(s);
    return result;
}

typedef void (SHIM_TC *ListFn)(void *self, void *request, int arg);

static Request *request_locked(uintptr_t h) {
    Request *r = &g_req[h % MAX_REQUESTS];
    return r->used && r->handle == h ? r : NULL;
}

static void call_list(void *response, void *handle, int arg, int slot) {
    AcquireSRWLockExclusive(&g_lock);
    Request *r = request_locked((uintptr_t)handle);
    bool ok = r && !r->cancelled && r->response == response;
    ReleaseSRWLockExclusive(&g_lock);
    if (!ok || !response) return;
    ((ListFn)(*(void ***)response)[slot])(response, handle, arg);
}
static void deferred_responded(void *response, void *handle, int idx) { call_list(response, handle, idx, 0); }
static void deferred_failed(void *response, void *handle, int idx) { call_list(response, handle, idx, 1); }
static void deferred_complete(void *response, void *handle, int code) { call_list(response, handle, code, 2); }

typedef struct { uintptr_t handle; int generation; } Gather;

static bool current(const Request *r, int generation) { return r && !r->cancelled && r->generation == generation; }

static void notify_game(Gather *g, int i, bool ok) {
    AcquireSRWLockExclusive(&g_lock);
    Request *r = request_locked(g->handle);
    bool live = current(r, g->generation);
    void *response = live ? r->response : NULL;
    ReleaseSRWLockExclusive(&g_lock);
    if (live) shim_defer(ok ? deferred_responded : deferred_failed, response, (void *)g->handle, i);
}

typedef struct { uint32_t ip, query_port, game_port, players, max_players, bots, flags, app, revision; uint64_t steamid; char dir[32], map[32], name[64], desc[64], tags[128]; bool has[16]; } MasterServer;

static void read_master_server(const uint8_t *data, size_t len, char **table, int n_table, MasterServer *m) {
    PbR in;
    PbField f;
    pbr_init(&in, data, len);
    while (pbr_next(&in, &f)) {
        const char *txt = NULL;
        switch (f.field) {
        case 1: m->ip = (uint32_t)f.v; break;
        case 2: m->query_port = (uint32_t)f.v; break;
        case 4: { PbR ip; PbField g; pbr_init(&ip, f.data, f.len); while (pbr_next(&ip, &g)) if (g.field == 1 && g.wt == 5) m->ip = (uint32_t)g.v; break; }
        case 6: m->steamid = f.v; break;
        case 7: m->revision = (uint32_t)f.v; break;
        case 8: m->players = (uint32_t)f.v; break;
        case 9: m->game_port = (uint32_t)f.v; break;
        case 11: m->flags = (uint32_t)f.v; break;
        case 12: m->app = (uint32_t)f.v; break;
        case 13: m->max_players = (uint32_t)f.v; break;
        case 14: m->bots = (uint32_t)f.v; break;
        default: break;
        }
        int base = f.field;
        bool by_index = false;
        if (base == 17 || base == 19 || base == 21 || base == 23 || base == 27) { base--; by_index = true; }
        if (base == 16 || base == 18 || base == 20 || base == 22 || base == 26) {
            char *dst = base == 16 ? m->dir : base == 18 ? m->map : base == 20 ? m->name : base == 22 ? m->desc : m->tags;
            size_t cap = base == 16 || base == 18 ? 32 : base == 26 ? 128 : 64;
            if (!by_index && f.wt == 2) { size_t n = f.len < cap - 1 ? f.len : cap - 1; memcpy(dst, f.data, n); dst[n] = 0; txt = dst; }
            else if (by_index && f.wt == 0 && f.v < (uint64_t)n_table) { snprintf(dst, cap, "%s", table[f.v]); txt = dst; }
            (void)txt;
        }
    }
}

static void fill_item(uint8_t *item, const MasterServer *m) {
    memset(item, 0, ITEM_SIZE);
    put16(item + IT_CONN_PORT, (uint16_t)(m->game_port ? m->game_port : m->query_port));
    put16(item + IT_QUERY_PORT, (uint16_t)m->query_port);
    put32(item + IT_IP, m->ip);
    put_str(item, IT_DIR, 32, m->dir);
    put_str(item, IT_MAP, 32, m->map);
    put_str(item, IT_DESC, 64, m->desc);
    put32(item + IT_APP, m->app);
    put32(item + IT_PLAYERS, m->players);
    put32(item + IT_MAX, m->max_players);
    put32(item + IT_BOTS, m->bots);
    item[IT_PASSWORD] = (m->flags & 16) != 0;
    item[IT_SECURE] = (m->flags & 2) != 0;
    put32(item + IT_VERSION, m->revision);
    put_str(item, IT_NAME, 64, m->name);
    put_str(item, IT_TAGS, 128, m->tags);
    memcpy(item + IT_STEAMID, &m->steamid, 8);
}

static void finish_request(Request *r, uintptr_t handle, int generation, int code) {
    AcquireSRWLockExclusive(&g_lock);
    bool live = current(r, generation);
    void *response = live ? r->response : NULL;
    if (live) r->refreshing = false;
    ReleaseSRWLockExclusive(&g_lock);
    if (live) shim_defer(deferred_complete, response, (void *)handle, code);
}

static DWORD WINAPI collect_thread(LPVOID arg);

static void start_collect(uintptr_t handle) {
    HANDLE t = CreateThread(NULL, 0, collect_thread, (LPVOID)handle, 0, NULL);
    if (t) CloseHandle(t);
}

typedef struct { uintptr_t handle; int generation, base; uint8_t *scratch; } BatchCtx;

static void batch_notify(void *c, int i, bool ok) {
    BatchCtx *b = c;
    int idx = b->base + i;
    AcquireSRWLockExclusive(&g_lock);
    Request *r = request_locked(b->handle);
    if (current(r, b->generation) && ok && idx < r->n) {
        uint8_t *item = r->items + (size_t)idx * ITEM_SIZE;
        uint16_t qp;
        uint32_t ip;
        memcpy(&qp, item + IT_QUERY_PORT, 2);
        memcpy(&ip, item + IT_IP, 4);
        memcpy(item, b->scratch + (size_t)i * ITEM_SIZE, ITEM_SIZE);
        put16(item + IT_QUERY_PORT, qp);
        put32(item + IT_IP, ip);
    }
    ReleaseSRWLockExclusive(&g_lock);
    Gather g = { b->handle, b->generation };
    notify_game(&g, idx, ok);
}

static void probe_range(uintptr_t handle, int generation, int first, int count) {
    Probe *probes = calloc((size_t)count, sizeof *probes);
    uint8_t *scratch = malloc((size_t)count * ITEM_SIZE);
    if (!probes || !scratch) { free(probes); free(scratch); return; }
    AcquireSRWLockExclusive(&g_lock);
    Request *r = request_locked(handle);
    if (!current(r, generation) || first + count > r->n) { ReleaseSRWLockExclusive(&g_lock); free(probes); free(scratch); return; }
    for (int i = 0; i < count; i++) {
        uint8_t *item = r->items + (size_t)(first + i) * ITEM_SIZE;
        uint16_t qp;
        uint32_t ip;
        memcpy(&qp, item + IT_QUERY_PORT, 2);
        memcpy(&ip, item + IT_IP, 4);
        memcpy(scratch + (size_t)i * ITEM_SIZE, item, ITEM_SIZE);
        probes[i].item = scratch + (size_t)i * ITEM_SIZE;
        probes[i].to.sin_family = AF_INET;
        probes[i].to.sin_port = htons(qp);
        probes[i].to.sin_addr.s_addr = htonl(ip);
    }
    ReleaseSRWLockExclusive(&g_lock);
    BatchCtx ctx = { handle, generation, first, scratch };
    probe_batch(probes, count, PING_TIMEOUT_MS, batch_notify, &ctx);
    free(probes);
    free(scratch);
}

static void collect_lan(Request *r, uintptr_t handle, int generation) {
    init_winsock();
    SOCKET s = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    if (s == INVALID_SOCKET) return;
    BOOL on = TRUE;
    setsockopt(s, SOL_SOCKET, SO_BROADCAST, (const char *)&on, sizeof on);
    struct sockaddr_in to = {0};
    to.sin_family = AF_INET;
    to.sin_addr.s_addr = INADDR_BROADCAST;
    for (int port = 27015; port <= 27020; port++) { to.sin_port = htons((u_short)port); sendto(s, (const char *)A2S_INFO, sizeof A2S_INFO, 0, (struct sockaddr *)&to, sizeof to); }
    DWORD start = GetTickCount();
    for (;;) {
        int left = 2000 - (int)(GetTickCount() - start);
        if (left <= 0) break;
        fd_set rd;
        FD_ZERO(&rd);
        FD_SET(s, &rd);
        struct timeval tv = { left / 1000, (left % 1000) * 1000 };
        if (select(0, &rd, NULL, NULL, &tv) <= 0) break;
        uint8_t buf[1400], item[ITEM_SIZE];
        struct sockaddr_in from;
        int fl = sizeof from;
        int got = recvfrom(s, (char *)buf, sizeof buf, 0, (struct sockaddr *)&from, &fl);
        memset(item, 0, sizeof item);
        if (got < 6 || buf[4] != 'I' || !parse_info(buf, got, item, 1)) continue;
        put16(item + IT_QUERY_PORT, ntohs(from.sin_port));
        put32(item + IT_IP, ntohl(from.sin_addr.s_addr));
        if (!*(uint16_t *)(item + IT_CONN_PORT)) put16(item + IT_CONN_PORT, ntohs(from.sin_port));
        AcquireSRWLockExclusive(&g_lock);
        bool live = current(r, generation) && r->n < MAX_SERVERS;
        int idx = live ? r->n++ : -1;
        void *response = live ? r->response : NULL;
        if (live) memcpy(r->items + (size_t)idx * ITEM_SIZE, item, ITEM_SIZE);
        ReleaseSRWLockExclusive(&g_lock);
        if (live) shim_defer(deferred_responded, response, (void *)handle, idx);
    }
    closesocket(s);
}

static void collect_internet(Request *r, uintptr_t handle, int generation, uint32_t app, const char *filter) {
    PbW w = {0};
    pbw_varint(&w, 1, app);
    pbw_varint(&w, 2, 0);
    pbw_varint(&w, 3, 255);
    pbw_str(&w, 4, filter);
    pbw_varint(&w, 5, INFO_WANT);
    CmReply reply = {0};
    bool ok = shim_cm_call(EMSG_SERVER_QUERY, true, NULL, w.p, w.len, 20000, &reply);
    pbw_free(&w);
    if (!ok) { shim_trace("internet servers app=%u filter=%s: no response from Steam", app, filter); finish_request(r, handle, generation, RESPONSE_FAILED); return; }
    PbR in;
    PbField f;
    char *table[512];
    int n_table = 0;
    MasterServer def;
    memset(&def, 0, sizeof def);
    pbr_init(&in, reply.body, reply.len);
    while (pbr_next(&in, &f)) {
        if (f.field == 4 && f.wt == 2 && n_table < 512) {
            table[n_table] = malloc(f.len + 1);
            if (table[n_table]) { memcpy(table[n_table], f.data, f.len); table[n_table][f.len] = 0; n_table++; }
        }
    }
    pbr_init(&in, reply.body, reply.len);
    while (pbr_next(&in, &f)) if (f.field == 3 && f.wt == 2) read_master_server(f.data, f.len, table, n_table, &def);
    int count = 0;
    pbr_init(&in, reply.body, reply.len);
    while (pbr_next(&in, &f)) {
        if (f.field != 1 || f.wt != 2 || count >= MAX_SERVERS) continue;
        MasterServer m = def;
        m.steamid = 0;
        read_master_server(f.data, f.len, table, n_table, &m);
        if (!m.ip || !m.query_port) continue;
        AcquireSRWLockExclusive(&g_lock);
        bool live = current(r, generation);
        if (live) { fill_item(r->items + (size_t)count * ITEM_SIZE, &m); r->n = ++count; }
        ReleaseSRWLockExclusive(&g_lock);
        if (!live) break;
    }
    for (int i = 0; i < n_table; i++) free(table[i]);
    shim_cm_reply_free(&reply);
    shim_trace("internet servers app=%u filter=%s -> %d", app, filter, count);
    if (!count) { finish_request(r, handle, generation, RESPONSE_NO_SERVERS); return; }
    probe_range(handle, generation, 0, count);
    finish_request(r, handle, generation, RESPONSE_OK);
}

typedef struct { uint64_t steamid; uint32_t app, ip, port, qport; } FriendServer;
static FriendServer g_fsrv[256];
static int g_n_fsrv;
static volatile LONG g_friends_hooked;

static void on_persona(const uint8_t *body, size_t len) {
    PbR in;
    PbField f;
    pbr_init(&in, body, len);
    while (pbr_next(&in, &f)) {
        if (f.field != 2 || f.wt != 2) continue;
        PbR fin;
        PbField g;
        FriendServer s;
        memset(&s, 0, sizeof s);
        pbr_init(&fin, f.data, f.len);
        while (pbr_next(&fin, &g)) {
            if (g.field == 1) s.steamid = g.v;
            else if (g.field == 3) s.app = (uint32_t)g.v;
            else if (g.field == 4) s.ip = (uint32_t)g.v;
            else if (g.field == 5) s.port = (uint32_t)g.v;
            else if (g.field == 20) s.qport = (uint32_t)g.v;
        }
        if (!s.steamid) continue;
        AcquireSRWLockExclusive(&g_lock);
        int at = -1;
        for (int i = 0; i < g_n_fsrv; i++) if (g_fsrv[i].steamid == s.steamid) at = i;
        if (at < 0 && g_n_fsrv < 256) at = g_n_fsrv++;
        if (at >= 0) g_fsrv[at] = s;
        ReleaseSRWLockExclusive(&g_lock);
    }
}

static void collect_known(Request *r, uintptr_t handle, int generation, uint32_t app, int kind) {
    ShimServerEntry entries[MAX_SERVERS];
    int n = 0;
    if (kind == KIND_FRIENDS) {
        if (!InterlockedCompareExchange(&g_friends_hooked, 1, 0)) shim_cm_on_push(766, on_persona);
        const Snapshot *snap = shim_snapshot();
        PbW w = {0};
        pbw_varint(&w, 1, 8 | 2 | 1 | 256);
        for (size_t i = 0; snap && i < snap->n_friends && i < 256; i++) pbw_fixed64(&w, 2, snap->friends[i].steamid);
        shim_cm_send(815, true, w.p, w.len);
        pbw_free(&w);
        Sleep(2500);
        AcquireSRWLockExclusive(&g_lock);
        for (int i = 0; i < g_n_fsrv && n < MAX_SERVERS; i++) {
            const FriendServer *s = &g_fsrv[i];
            if (!s->ip || !s->port || (app && s->app != app)) continue;
            entries[n++] = (ShimServerEntry){ s->app, s->ip, (uint16_t)s->port, (uint16_t)(s->qport ? s->qport : s->port), 0, 0 };
        }
        ReleaseSRWLockExclusive(&g_lock);
    } else {
        n = shim_serverlist_get(entries, MAX_SERVERS, kind == KIND_FAVORITES ? 1 : 2, app);
    }
    if (!n) { finish_request(r, handle, generation, RESPONSE_NO_SERVERS); return; }
    AcquireSRWLockExclusive(&g_lock);
    bool live = current(r, generation);
    if (live) {
        for (int i = 0; i < n; i++) {
            uint8_t *item = r->items + (size_t)i * ITEM_SIZE;
            memset(item, 0, ITEM_SIZE);
            put16(item + IT_CONN_PORT, entries[i].conn_port);
            put16(item + IT_QUERY_PORT, entries[i].query_port);
            put32(item + IT_IP, entries[i].ip);
            put32(item + IT_APP, entries[i].appid);
            put32(item + IT_TIME, entries[i].last_played);
        }
        r->n = n;
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (!live) return;
    probe_range(handle, generation, 0, n);
    finish_request(r, handle, generation, RESPONSE_OK);
}

static DWORD WINAPI collect_thread(LPVOID arg) {
    uintptr_t handle = (uintptr_t)arg;
    AcquireSRWLockExclusive(&g_lock);
    Request *r = request_locked(handle);
    int generation = r ? r->generation : 0, kind = r ? r->kind : 0;
    uint32_t app = r ? r->app : 0;
    char filter[2048];
    if (r) snprintf(filter, sizeof filter, "%s", r->filter);
    ReleaseSRWLockExclusive(&g_lock);
    if (!r) return 0;
    if (kind == KIND_INTERNET) collect_internet(r, handle, generation, app, filter);
    else if (kind == KIND_SPECTATOR) { strncat(filter, "\\proxy\\1", sizeof filter - strlen(filter) - 1); collect_internet(r, handle, generation, app, filter); }
    else if (kind == KIND_FAVORITES || kind == KIND_HISTORY || kind == KIND_FRIENDS) collect_known(r, handle, generation, app, kind);
    else if (kind == KIND_LAN) { collect_lan(r, handle, generation); finish_request(r, handle, generation, RESPONSE_OK); }
    else finish_request(r, handle, generation, RESPONSE_NO_SERVERS);
    return 0;
}

static uintptr_t new_request(void *response, int kind, uint32_t app, const char *filter) {
    AcquireSRWLockExclusive(&g_lock);
    uintptr_t h = g_next++;
    Request *r = &g_req[h % MAX_REQUESTS];
    free(r->items);
    memset(r, 0, sizeof *r);
    r->items = calloc(MAX_SERVERS, ITEM_SIZE);
    if (!r->items) { ReleaseSRWLockExclusive(&g_lock); return 0; }
    r->handle = h;
    r->response = response;
    r->kind = kind;
    r->app = app;
    r->used = true;
    r->refreshing = true;
    r->generation = 1;
    snprintf(r->filter, sizeof r->filter, "%s", filter ? filter : "");
    ReleaseSRWLockExclusive(&g_lock);
    start_collect(h);
    return h;
}

static void build_filter(char *out, size_t cap, uint32_t app, void **filters, uint32_t n) {
    size_t o = (size_t)snprintf(out, cap, "\\appid\\%u", app);
    for (uint32_t i = 0; filters && i < n && i < 64; i++) {
        const char *key = filters[i] ? (const char *)filters[i] : NULL;
        if (!key || !*key || o + 520 >= cap) continue;
        o += (size_t)snprintf(out + o, cap - o, "\\%s\\%s", key, key + 256);
    }
}

static uint64_t SHIM_TC request_internet(void *self, uint32_t appid, void **filters, uint32_t n, void *response) {
    (void)self;
    char filter[2048];
    build_filter(filter, sizeof filter, appid, filters, n);
    return new_request(response, KIND_INTERNET, appid, filter);
}
static uint64_t SHIM_TC request_lan(void *self, uint32_t appid, void *response) { (void)self; return new_request(response, KIND_LAN, appid, ""); }
static uint64_t SHIM_TC request_favorites(void *self, uint32_t appid, void **filters, uint32_t n, void *response) { (void)self; (void)filters; (void)n; return new_request(response, KIND_FAVORITES, appid, ""); }
static uint64_t SHIM_TC request_history(void *self, uint32_t appid, void **filters, uint32_t n, void *response) { (void)self; (void)filters; (void)n; return new_request(response, KIND_HISTORY, appid, ""); }
static uint64_t SHIM_TC request_friends(void *self, uint32_t appid, void **filters, uint32_t n, void *response) { (void)self; (void)filters; (void)n; return new_request(response, KIND_FRIENDS, appid, ""); }
static uint64_t SHIM_TC request_spectator(void *self, uint32_t appid, void **filters, uint32_t n, void *response) {
    (void)self;
    char filter[2048];
    build_filter(filter, sizeof filter, appid, filters, n);
    return new_request(response, KIND_SPECTATOR, appid, filter);
}

static void SHIM_TC release_request(void *self, void *handle) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Request *r = request_locked((uintptr_t)handle);
    if (r) { r->cancelled = true; r->used = false; }
    ReleaseSRWLockExclusive(&g_lock);
}
static void SHIM_TC cancel_query(void *self, void *handle) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Request *r = request_locked((uintptr_t)handle);
    if (r) { r->generation++; r->refreshing = false; }
    ReleaseSRWLockExclusive(&g_lock);
}
static void SHIM_TC refresh_query(void *self, void *handle) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Request *r = request_locked((uintptr_t)handle);
    bool go = r != NULL;
    if (go) { r->generation++; r->refreshing = true; r->n = 0; memset(r->items, 0, (size_t)MAX_SERVERS * ITEM_SIZE); }
    ReleaseSRWLockExclusive(&g_lock);
    if (go) start_collect((uintptr_t)handle);
}
static uint64_t SHIM_TC is_refreshing(void *self, void *handle) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Request *r = request_locked((uintptr_t)handle);
    bool yes = r && r->refreshing;
    ReleaseSRWLockExclusive(&g_lock);
    return yes;
}
static int SHIM_TC server_count(void *self, void *handle) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Request *r = request_locked((uintptr_t)handle);
    int n = r ? r->n : 0;
    ReleaseSRWLockExclusive(&g_lock);
    return n;
}
static void *SHIM_TC server_details(void *self, void *handle, int server) {
    (void)self;
    void *p = NULL;
    AcquireSRWLockExclusive(&g_lock);
    Request *r = request_locked((uintptr_t)handle);
    if (r && server >= 0 && server < r->n) p = r->items + (size_t)server * ITEM_SIZE;
    ReleaseSRWLockExclusive(&g_lock);
    if (!p) { static uint8_t empty[ITEM_SIZE]; p = empty; }
    return p;
}

typedef struct { uintptr_t handle; int generation, index; } RefreshCtx;

static DWORD WINAPI refresh_server_thread(LPVOID arg) {
    RefreshCtx *c = arg;
    probe_range(c->handle, c->generation, c->index, 1);
    free(c);
    return 0;
}

static void SHIM_TC refresh_server(void *self, void *handle, int server) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Request *r = request_locked((uintptr_t)handle);
    bool ok = r && !r->cancelled && server >= 0 && server < r->n;
    int generation = r ? r->generation : 0;
    ReleaseSRWLockExclusive(&g_lock);
    if (!ok) return;
    RefreshCtx *c = calloc(1, sizeof *c);
    if (!c) return;
    c->handle = (uintptr_t)handle; c->generation = generation; c->index = server;
    HANDLE t = CreateThread(NULL, 0, refresh_server_thread, c, 0, NULL);
    if (!t) { free(c); return; }
    CloseHandle(t);
}

static void noop_notify(void *ctx, int i, bool ok) { (void)ctx; (void)i; (void)ok; }

typedef struct { uint32_t handle; int kind; void *response; uint32_t ip; uint16_t port; } QueryCtx;

typedef void (SHIM_TC *PingOkFn)(void *self, void *item);
typedef void (SHIM_TC *Fn0)(void *self);
typedef void (SHIM_TC *PlayerFn)(void *self, const char *name, int score, float time);
typedef void (SHIM_TC *RuleFn)(void *self, const char *key, const char *value);

static bool query_live(uint32_t handle) {
    AcquireSRWLockExclusive(&g_lock);
    Query *q = &g_query[handle % MAX_QUERIES];
    bool live = q->used && q->handle == handle && !q->cancelled;
    ReleaseSRWLockExclusive(&g_lock);
    return live;
}

static void query_done(uint32_t handle) {
    AcquireSRWLockExclusive(&g_lock);
    Query *q = &g_query[handle % MAX_QUERIES];
    if (q->handle == handle) memset(q, 0, sizeof *q);
    ReleaseSRWLockExclusive(&g_lock);
}

static void ping_responded(void *response, void *item, int handle) {
    if (query_live((uint32_t)handle)) ((PingOkFn)(*(void ***)response)[0])(response, item);
    free(item);
    query_done((uint32_t)handle);
}
static void ping_failed(void *response, void *unused, int handle) {
    (void)unused;
    if (query_live((uint32_t)handle)) ((Fn0)(*(void ***)response)[1])(response);
    query_done((uint32_t)handle);
}

static DWORD WINAPI ping_thread(LPVOID arg) {
    QueryCtx *c = arg;
    uint8_t *item = calloc(1, ITEM_SIZE);
    Probe p;
    memset(&p, 0, sizeof p);
    p.item = item;
    p.to.sin_family = AF_INET;
    p.to.sin_port = htons(c->port);
    p.to.sin_addr.s_addr = htonl(c->ip);
    bool ok = false;
    if (item) {
        put16(item + IT_QUERY_PORT, c->port);
        put16(item + IT_CONN_PORT, c->port);
        put32(item + IT_IP, c->ip);
        probe_batch(&p, 1, PING_TIMEOUT_MS, noop_notify, NULL);
        ok = p.ok;
    }
    if (ok) shim_defer(ping_responded, c->response, item, (int)c->handle);
    else { free(item); shim_defer(ping_failed, c->response, NULL, (int)c->handle); }
    free(c);
    return 0;
}

static void players_done(void *response, void *unused, int handle) {
    (void)unused;
    if (query_live((uint32_t)handle)) ((Fn0)(*(void ***)response)[2])(response);
    query_done((uint32_t)handle);
}
static void players_failed(void *response, void *unused, int handle) {
    (void)unused;
    if (query_live((uint32_t)handle)) ((Fn0)(*(void ***)response)[1])(response);
    query_done((uint32_t)handle);
}
typedef struct { void *response; char name[64]; int score; float time; uint32_t handle; } PlayerEntry;
static void player_entry(void *entry, void *unused, int unused2) {
    (void)unused; (void)unused2;
    PlayerEntry *e = entry;
    if (query_live(e->handle)) ((PlayerFn)(*(void ***)e->response)[0])(e->response, e->name, e->score, e->time);
    free(e);
}

static DWORD WINAPI players_thread(LPVOID arg) {
    QueryCtx *c = arg;
    uint8_t buf[4096];
    int n = a2s_exchange(c->ip, c->port, 0x55, buf, sizeof buf);
    if (n < 6 || buf[4] != 'D') { shim_defer(players_failed, c->response, NULL, (int)c->handle); free(c); return 0; }
    const uint8_t *p = buf + 5, *end = buf + n;
    int count = *p++;
    for (int i = 0; i < count && end - p > 1; i++) {
        p++;
        const char *name = take_str(&p, end);
        if (!name || end - p < 8) break;
        int32_t score;
        float time;
        memcpy(&score, p, 4);
        memcpy(&time, p + 4, 4);
        p += 8;
        PlayerEntry *e = calloc(1, sizeof *e);
        if (!e) break;
        e->response = c->response; e->handle = c->handle; e->score = score; e->time = time;
        snprintf(e->name, sizeof e->name, "%s", name);
        shim_defer(player_entry, e, NULL, 0);
    }
    shim_defer(players_done, c->response, NULL, (int)c->handle);
    free(c);
    return 0;
}

typedef struct { void *response; char key[256]; char value[1024]; uint32_t handle; } RuleEntry;
static void rule_entry(void *entry, void *unused, int unused2) {
    (void)unused; (void)unused2;
    RuleEntry *e = entry;
    if (query_live(e->handle)) ((RuleFn)(*(void ***)e->response)[0])(e->response, e->key, e->value);
    free(e);
}

static DWORD WINAPI rules_thread(LPVOID arg) {
    QueryCtx *c = arg;
    uint8_t *buf = malloc(65536);
    int n = buf ? a2s_exchange(c->ip, c->port, 0x56, buf, 65536) : -1;
    if (n < 7 || buf[4] != 'E') { free(buf); shim_defer(players_failed, c->response, NULL, (int)c->handle); free(c); return 0; }
    const uint8_t *p = buf + 5, *end = buf + n;
    uint16_t count;
    memcpy(&count, p, 2);
    p += 2;
    for (int i = 0; i < count; i++) {
        const char *k = take_str(&p, end), *v = k ? take_str(&p, end) : NULL;
        if (!k || !v) break;
        RuleEntry *e = calloc(1, sizeof *e);
        if (!e) break;
        e->response = c->response; e->handle = c->handle;
        snprintf(e->key, sizeof e->key, "%.255s", k);
        snprintf(e->value, sizeof e->value, "%.1023s", v);
        shim_defer(rule_entry, e, NULL, 0);
    }
    free(buf);
    shim_defer(players_done, c->response, NULL, (int)c->handle);
    free(c);
    return 0;
}

static uint32_t quiet_query(int kind) {
    AcquireSRWLockExclusive(&g_lock);
    uint32_t h = g_next_query++;
    g_query[h % MAX_QUERIES] = (Query){ h, kind, NULL, false, false };
    ReleaseSRWLockExclusive(&g_lock);
    return h;
}

static uint32_t start_query(int kind, uint32_t ip, uint16_t port, void *response, LPTHREAD_START_ROUTINE fn) {
    shim_trace("server query kind=%d %u.%u.%u.%u:%u", kind, ip >> 24, (ip >> 16) & 255, (ip >> 8) & 255, ip & 255, port);
    if (!response) return quiet_query(kind);
    QueryCtx *c = calloc(1, sizeof *c);
    if (!c) return QUERY_INVALID;
    AcquireSRWLockExclusive(&g_lock);
    uint32_t h = g_next_query++;
    g_query[h % MAX_QUERIES] = (Query){ h, kind, response, true, false };
    ReleaseSRWLockExclusive(&g_lock);
    c->handle = h; c->kind = kind; c->response = response; c->ip = ip; c->port = port;
    HANDLE t = CreateThread(NULL, 0, fn, c, 0, NULL);
    if (!t) { free(c); query_done(h); return QUERY_INVALID; }
    CloseHandle(t);
    return h;
}

static uint32_t SHIM_TC ping_server(void *self, uint32_t ip, uint16_t port, void *response) { (void)self; return start_query(0, ip, port, response, ping_thread); }
static uint32_t SHIM_TC player_details(void *self, uint32_t ip, uint16_t port, void *response) { (void)self; return start_query(1, ip, port, response, players_thread); }
static uint32_t SHIM_TC server_rules(void *self, uint32_t ip, uint16_t port, void *response) { (void)self; return start_query(2, ip, port, response, rules_thread); }
static uint32_t SHIM_TC server_friends(void *self, uint32_t ip, uint16_t port, void *response) { (void)self; (void)ip; (void)port; (void)response; return quiet_query(3); }
static void SHIM_TC cancel_server_query(void *self, uint32_t handle) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Query *q = &g_query[handle % MAX_QUERIES];
    if (q->used && q->handle == handle) q->cancelled = true;
    ReleaseSRWLockExclusive(&g_lock);
}

void register_matchmakingservers(void) {
    const char *p = "SteamMatchMakingServers";
    shim_register_family(p, "RequestInternetServerList", (void *)request_internet);
    shim_register_family(p, "RequestFriendsServerList", (void *)request_friends);
    shim_register_family(p, "RequestFavoritesServerList", (void *)request_favorites);
    shim_register_family(p, "RequestHistoryServerList", (void *)request_history);
    shim_register_family(p, "RequestSpectatorServerList", (void *)request_spectator);
    shim_register_family(p, "RequestLANServerList", (void *)request_lan);
    shim_register_family(p, "ReleaseRequest", (void *)release_request);
    shim_register_family(p, "CancelQuery", (void *)cancel_query);
    shim_register_family(p, "RefreshQuery", (void *)refresh_query);
    shim_register_family(p, "IsRefreshing", (void *)is_refreshing);
    shim_register_family(p, "GetServerCount", (void *)server_count);
    shim_register_family(p, "GetServerDetails", (void *)server_details);
    shim_register_family(p, "RefreshServer", (void *)refresh_server);
    shim_register_family(p, "PingServer", (void *)ping_server);
    shim_register_family(p, "PlayerDetails", (void *)player_details);
    shim_register_family(p, "ServerRules", (void *)server_rules);
    shim_register_family(p, "ServerFriends", (void *)server_friends);
    shim_register_family(p, "CancelServerQuery", (void *)cancel_server_query);
}
