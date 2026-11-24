#define _CRT_RAND_S
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <winsock2.h>
#include <ws2tcpip.h>
#include <iphlpapi.h>
#include <windows.h>

#include "shim.h"

enum {
    EMSG_P2P_INFO = 5434, MAX_SESSIONS = 16, MAX_CAND = 8, WINDOW = 64, MTU = 1100,
    PKT_PUNCH = 1, PKT_PUNCH_ACK = 2, PKT_DATA = 3, PKT_ACK = 4, PKT_PING = 5, PKT_CLOSE = 6,
    S2_OFFERED = 1, S2_PUNCHING = 2, S2_CONNECTED = 3, S2_CLOSED = 4,
    SG_OFFER = 1, SG_ANSWER = 2, SG_CLOSE = 3,
    PUNCH_TIMEOUT_MS = 12000, IDLE_TIMEOUT_MS = 20000, RTO_MS = 250, MAX_TRIES = 40, FRAG = 1000, MAX_MESSAGE = 1 << 20, FLAG_MORE = 4,
};

typedef struct { uint32_t ip; uint16_t port; } Cand;
typedef struct { uint32_t seq; int len; uint8_t data[MTU + 16]; DWORD sent; int tries; bool used; } Unacked;
typedef struct { uint32_t seq; int len; uint8_t data[MTU + 16]; bool used; } Held;
typedef struct Pending { uint32_t stream; uint8_t flags; int len; uint8_t *data; struct Pending *next; } Pending;

typedef struct {
    bool used, initiator, direct;
    SOCKET sock;
    int state;
    uint64_t peer, token;
    Cand remote[MAX_CAND];
    int n_remote;
    struct sockaddr_in path;
    bool have_path;
    DWORD started, last_rx, last_tx;
    uint32_t next_seq, expect_seq;
    Unacked unacked[WINDOW];
    Held held[WINDOW];
    Pending *outq, *outq_tail;
    P2PMsg *inq, *inq_tail;
    uint8_t *frag;
    int frag_len, frag_cap;
} Session;

static Session g_s[MAX_SESSIONS];
static SOCKET g_sock = INVALID_SOCKET;
#define DIRECT_TAG (1ULL << 62)
typedef struct { SOCKET sk; uint16_t port; bool used; } Listener;
static Listener g_ls[8];
#define SK(s) ((s)->direct && (s)->sock != INVALID_SOCKET ? (s)->sock : g_sock)
static Cand g_local[MAX_CAND];
static int g_n_local;
static volatile LONG g_started, g_gathered;
static SRWLOCK g_lock = SRWLOCK_INIT;
static P2PEventFn g_events[4];
static uint16_t g_port;

static uint64_t self_id(void) { const Snapshot *s = shim_snapshot(); return s ? s->steamid : 0; }
static uint32_t app_id(void) { const Snapshot *s = shim_snapshot(); return s && s->has_ticket ? s->ticket.appid : 0; }
void p2p_set_event(P2PEventFn fn) {
    for (int i = 0; i < 4; i++) if (g_events[i] == fn) return;
    for (int i = 0; i < 4; i++) if (!g_events[i]) { g_events[i] = fn; return; }
}

static uint64_t random64(void) {
    unsigned int a = 0, b = 0;
    rand_s(&a);
    rand_s(&b);
    return ((uint64_t)a << 32) | b;
}

static void add_local(uint32_t ip, uint16_t port) {
    for (int i = 0; i < g_n_local; i++) if (g_local[i].ip == ip && g_local[i].port == port) return;
    if (g_n_local < MAX_CAND) g_local[g_n_local++] = (Cand){ ip, port };
}

static bool stun_query(const char *host, uint16_t port, uint32_t *ip, uint16_t *pub_port) {
    struct hostent *he = gethostbyname(host);
    if (!he || !he->h_addr_list[0]) return false;
    struct sockaddr_in to = {0};
    to.sin_family = AF_INET;
    to.sin_port = htons(port);
    memcpy(&to.sin_addr, he->h_addr_list[0], 4);
    uint8_t req[20] = { 0x00, 0x01, 0x00, 0x00, 0x21, 0x12, 0xA4, 0x42 };
    for (int i = 8; i < 20; i++) req[i] = (uint8_t)(random64() & 0xFF);
    for (int attempt = 0; attempt < 3; attempt++) {
        sendto(g_sock, (const char *)req, sizeof req, 0, (struct sockaddr *)&to, sizeof to);
        DWORD start = GetTickCount();
        while (GetTickCount() - start < 700) {
            fd_set rd;
            FD_ZERO(&rd);
            FD_SET(g_sock, &rd);
            struct timeval tv = { 0, 100000 };
            if (select(0, &rd, NULL, NULL, &tv) <= 0) continue;
            uint8_t buf[512];
            struct sockaddr_in from;
            int fl = sizeof from;
            int n = recvfrom(g_sock, (char *)buf, sizeof buf, MSG_PEEK, (struct sockaddr *)&from, &fl);
            if (n < 20 || from.sin_addr.s_addr != to.sin_addr.s_addr || buf[0] != 0x01 || buf[1] != 0x01 || memcmp(buf + 8, req + 8, 12)) continue;
            n = recvfrom(g_sock, (char *)buf, sizeof buf, 0, (struct sockaddr *)&from, &fl);
            for (int o = 20; o + 4 <= n;) {
                uint16_t type = (uint16_t)(buf[o] << 8 | buf[o + 1]), len = (uint16_t)(buf[o + 2] << 8 | buf[o + 3]);
                if (type == 0x0020 && len >= 8 && o + 4 + len <= n) {
                    uint16_t xp = (uint16_t)(buf[o + 6] << 8 | buf[o + 7]);
                    uint32_t xa = (uint32_t)buf[o + 8] << 24 | buf[o + 9] << 16 | buf[o + 10] << 8 | buf[o + 11];
                    *pub_port = xp ^ 0x2112;
                    *ip = xa ^ 0x2112A442u;
                    return true;
                }
                o += 4 + ((len + 3) & ~3);
            }
        }
    }
    return false;
}

static void gather_candidates(void) {
    bool local_only = getenv("AURELIA_SHIM_P2P_LOCAL") != NULL;
    if (local_only) {
        add_local(0x7F000001u, g_port);
    } else {
        ULONG size = 16384;
        IP_ADAPTER_ADDRESSES *aa = malloc(size);
        if (aa && GetAdaptersAddresses(AF_INET, GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER, NULL, aa, &size) == NO_ERROR) {
            for (IP_ADAPTER_ADDRESSES *a = aa; a; a = a->Next) {
                if (a->OperStatus != IfOperStatusUp || a->IfType == IF_TYPE_SOFTWARE_LOOPBACK) continue;
                for (IP_ADAPTER_UNICAST_ADDRESS *u = a->FirstUnicastAddress; u; u = u->Next) {
                    if (u->Address.lpSockaddr->sa_family != AF_INET) continue;
                    add_local(ntohl(((struct sockaddr_in *)u->Address.lpSockaddr)->sin_addr.s_addr), g_port);
                }
            }
        }
        free(aa);
        uint32_t ip;
        uint16_t port;
        if (stun_query("stun.l.google.com", 19302, &ip, &port) || stun_query("stun.cloudflare.com", 3478, &ip, &port)) {
            add_local(ip, port);
            shim_log("P2P: public address %u.%u.%u.%u:%u (STUN)", ip >> 24, (ip >> 16) & 255, (ip >> 8) & 255, ip & 255, port);
        }
    }
    InterlockedExchange(&g_gathered, 1);
}

static void raw_send_on(SOCKET sk, const struct sockaddr_in *to, uint8_t type, uint64_t token, const void *body, int len) {
    uint8_t pkt[MTU + 64];
    if (len > MTU + 32) return;
    pkt[0] = 0xA7;
    pkt[1] = type;
    memcpy(pkt + 2, &token, 8);
    if (len) memcpy(pkt + 10, body, (size_t)len);
    sendto(sk, (const char *)pkt, 10 + len, 0, (const struct sockaddr *)to, sizeof *to);
}
static void raw_send(const struct sockaddr_in *to, uint8_t type, uint64_t token, const void *body, int len) { raw_send_on(g_sock, to, type, token, body, len); }

static struct sockaddr_in to_addr(const Cand *c) {
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET;
    a.sin_port = htons(c->port);
    a.sin_addr.s_addr = htonl(c->ip);
    return a;
}

#define DBG(...) do { if (getenv("AURELIA_P2P_DEBUG")) { fprintf(stderr, "[p2p %lu] ", (unsigned long)(GetTickCount() % 100000)); fprintf(stderr, __VA_ARGS__); fputc(10, stderr); fflush(stderr); } } while (0)
static void notify(int ev, uint64_t peer, int arg) { for (int i = 0; i < 4; i++) { P2PEventFn f = g_events[i]; if (f) f(ev, peer, arg); } }

static Session *by_peer_locked(uint64_t peer) {
    for (int i = 0; i < MAX_SESSIONS; i++) if (g_s[i].used && g_s[i].peer == peer && g_s[i].state != S2_CLOSED) return &g_s[i];
    return NULL;
}
static Session *by_token_locked(uint64_t token) {
    for (int i = 0; i < MAX_SESSIONS; i++) if (g_s[i].used && g_s[i].token == token) return &g_s[i];
    return NULL;
}

static Session *by_token_sock_locked(uint64_t token, SOCKET sk) {
    for (int i = 0; i < MAX_SESSIONS; i++) if (g_s[i].used && g_s[i].token == token && SK(&g_s[i]) == sk) return &g_s[i];
    return NULL;
}

static void drop_queues(Session *s) {
    for (Pending *p = s->outq; p;) { Pending *n = p->next; free(p->data); free(p); p = n; }
    for (P2PMsg *m = s->inq; m;) { P2PMsg *n = m->next; free(m->data); free(m); m = n; }
    free(s->frag);
    s->frag = NULL;
    s->frag_len = s->frag_cap = 0;
    s->outq = s->outq_tail = NULL;
    s->inq = s->inq_tail = NULL;
}

typedef struct { uint64_t peer, token; uint8_t type; } SignalJob;
static void signal_now(uint64_t peer, uint8_t type, uint64_t token);

static DWORD WINAPI signal_thread(LPVOID arg) {
    SignalJob *j = arg;
    for (int i = 0; i < 100 && !g_gathered; i++) Sleep(50);
    signal_now(j->peer, j->type, j->token);
    free(j);
    return 0;
}

static void signal_send(uint64_t peer, uint8_t type, uint64_t token) {
    if (g_gathered) { signal_now(peer, type, token); return; }
    SignalJob *j = malloc(sizeof *j);
    if (!j) return;
    *j = (SignalJob){ peer, token, type };
    HANDLE t = CreateThread(NULL, 0, signal_thread, j, 0, NULL);
    if (t) CloseHandle(t); else free(j);
}

static void signal_now(uint64_t peer, uint8_t type, uint64_t token) {
    uint8_t env[16 + MAX_CAND * 6];
    int n = 0;
    memcpy(env, "AP2P", 4); n = 4;
    env[n++] = 1;
    env[n++] = type;
    memcpy(env + n, &token, 8); n += 8;
    int nc = type == SG_CLOSE ? 0 : g_n_local;
    env[n++] = (uint8_t)nc;
    for (int i = 0; i < nc; i++) { memcpy(env + n, &g_local[i].ip, 4); memcpy(env + n + 4, &g_local[i].port, 2); n += 6; }
    PbW w = {0};
    pbw_fixed64(&w, 1, peer);
    pbw_fixed64(&w, 2, self_id());
    pbw_varint(&w, 3, app_id());
    pbw_bytes(&w, 6, env, (size_t)n);
    shim_cm_post(EMSG_P2P_INFO, &w);
}

static void begin_punching_locked(Session *s) {
    s->state = S2_PUNCHING;
    s->started = GetTickCount();
}

static void on_signal(const uint8_t *body, size_t len) {
    PbR r;
    PbField f;
    uint64_t dest = 0, src = 0;
    const uint8_t *env = NULL;
    size_t env_len = 0;
    pbr_init(&r, body, len);
    while (pbr_next(&r, &f)) {
        if (f.field == 1) dest = f.v;
        else if (f.field == 2) src = f.v;
        else if (f.field == 6 && f.wt == 2) { env = f.data; env_len = f.len; }
    }
    DBG("signal received dest=%llu src=%llu bytes=%lu me=%llu", (unsigned long long)dest, (unsigned long long)src, (unsigned long)env_len, (unsigned long long)self_id());
    shim_log("P2P signal received: dest=%llu src=%llu bytes=%lu (me=%llu)", (unsigned long long)dest, (unsigned long long)src, (unsigned long)env_len, (unsigned long long)self_id());
    if (!env || env_len < 15 || memcmp(env, "AP2P", 4) || env[4] != 1 || dest != self_id() || !src) return;
    uint8_t type = env[5];
    uint64_t token;
    memcpy(&token, env + 6, 8);
    int nc = env[14];
    if (env_len < 15 + (size_t)nc * 6) return;
    AcquireSRWLockExclusive(&g_lock);
    Session *s = by_token_locked(token);
    if (type == SG_CLOSE) {
        if (s && s->state != S2_CLOSED) { s->state = S2_CLOSED; ReleaseSRWLockExclusive(&g_lock); notify(2, src, 0); return; }
        ReleaseSRWLockExclusive(&g_lock);
        return;
    }
    bool answer_needed = false;
    if (!s && type == SG_OFFER) {
        Session *old = by_peer_locked(src);
        if (old) { old->state = S2_CLOSED; drop_queues(old); old->used = false; }
        for (int i = 0; i < MAX_SESSIONS && !s; i++) if (!g_s[i].used) s = &g_s[i];
        if (s) {
            memset(s, 0, sizeof *s);
            s->used = true; s->peer = src; s->token = token; s->initiator = false;
            s->last_rx = GetTickCount();
            answer_needed = true;
        }
    }
    if (s && s->peer == src) {
        for (int i = 0; i < nc && s->n_remote < MAX_CAND; i++) {
            Cand c;
            memcpy(&c.ip, env + 15 + i * 6, 4);
            memcpy(&c.port, env + 15 + i * 6 + 4, 2);
            bool dup = false;
            for (int k = 0; k < s->n_remote; k++) dup = dup || (s->remote[k].ip == c.ip && s->remote[k].port == c.port);
            if (!dup) s->remote[s->n_remote++] = c;
        }
        if (s->state < S2_PUNCHING) begin_punching_locked(s);
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (answer_needed) signal_send(src, SG_ANSWER, token);
}

static void deliver_locked(Session *s, uint32_t stream, uint8_t flags, const uint8_t *data, int len) {
    P2PMsg *m = calloc(1, sizeof *m);
    if (!m) return;
    m->data = malloc((size_t)len + 1);
    if (!m->data) { free(m); return; }
    memcpy(m->data, data, (size_t)len);
    m->len = len; m->peer = s->peer; m->stream = stream; m->flags = flags;
    if (s->inq_tail) s->inq_tail->next = m; else s->inq = m;
    s->inq_tail = m;
}

static void deliver_frag_locked(Session *s, uint32_t stream, uint8_t flags, const uint8_t *data, int len) {
    if (!(flags & FLAG_MORE) && !s->frag_len) { deliver_locked(s, stream, flags, data, len); return; }
    if (s->frag_len + len > MAX_MESSAGE) { s->frag_len = 0; return; }
    if (s->frag_len + len > s->frag_cap) {
        int cap = s->frag_cap ? s->frag_cap * 2 : 4096;
        while (cap < s->frag_len + len) cap *= 2;
        uint8_t *nb = realloc(s->frag, (size_t)cap);
        if (!nb) { s->frag_len = 0; return; }
        s->frag = nb; s->frag_cap = cap;
    }
    memcpy(s->frag + s->frag_len, data, (size_t)len);
    s->frag_len += len;
    if (flags & FLAG_MORE) return;
    deliver_locked(s, stream, (uint8_t)(flags & ~FLAG_MORE), s->frag, s->frag_len);
    s->frag_len = 0;
}

static void send_ack_locked(Session *s) {
    uint32_t a = s->expect_seq;
    raw_send_on(SK(s), &s->path, PKT_ACK, s->token, &a, 4);
}

static void handle_data_locked(Session *s, const uint8_t *b, int n, int *new_data) {
    if (n < 9) return;
    uint32_t seq, stream;
    memcpy(&seq, b, 4);
    memcpy(&stream, b + 4, 4);
    uint8_t flags = b[8];
    const uint8_t *pl = b + 9;
    int pn = n - 9;
    if (!(flags & 1)) { deliver_locked(s, stream, flags, pl, pn); *new_data = 1; return; }
    if ((int32_t)(seq - s->expect_seq) < 0) { send_ack_locked(s); return; }
    if (seq == s->expect_seq) {
        deliver_frag_locked(s, stream, flags, pl, pn);
        s->expect_seq++;
        for (;;) {
            Held *h = &s->held[s->expect_seq % WINDOW];
            if (!h->used || h->seq != s->expect_seq) break;
            uint32_t st2;
            memcpy(&st2, h->data + 4, 4);
            deliver_frag_locked(s, st2, h->data[8], h->data + 9, h->len - 9);
            h->used = false;
            s->expect_seq++;
        }
        *new_data = 1;
    } else if (seq - s->expect_seq < WINDOW) {
        Held *h = &s->held[seq % WINDOW];
        if (n <= (int)sizeof h->data) { memcpy(h->data, b, (size_t)n); h->len = n; h->seq = seq; h->used = true; }
    }
    send_ack_locked(s);
}

static DWORD WINAPI recv_thread(LPVOID arg) {
    SOCKET sk = (SOCKET)(uintptr_t)arg;
    uint8_t buf[2048];
    for (;;) {
        struct sockaddr_in from;
        int fl = sizeof from;
        int n = recvfrom(sk, (char *)buf, sizeof buf, 0, (struct sockaddr *)&from, &fl);
        if (n < 10) { if (n < 0) { int e = WSAGetLastError(); if (e == WSAENOTSOCK || e == WSAEINTR || e == WSAESHUTDOWN) return 0; } continue; }
        if (buf[0] != 0xA7) continue;
        uint8_t type = buf[1];
        uint64_t token;
        memcpy(&token, buf + 2, 8);
        int ev = 0;
        uint64_t peer = 0;
        int new_data = 0;
        AcquireSRWLockExclusive(&g_lock);
        Session *s = by_token_sock_locked(token, sk);
        if (!s && sk != g_sock && type == PKT_PUNCH) {
            uint64_t id = DIRECT_TAG | ((uint64_t)ntohl(from.sin_addr.s_addr) << 16) | ntohs(from.sin_port);
            for (int i = 0; i < MAX_SESSIONS; i++) if (g_s[i].used && g_s[i].peer == id && g_s[i].state != S2_CLOSED) { g_s[i].state = S2_CLOSED; drop_queues(&g_s[i]); g_s[i].used = false; }
            for (int i = 0; i < MAX_SESSIONS && !s; i++) if (!g_s[i].used) s = &g_s[i];
            if (s) {
                memset(s, 0, sizeof *s);
                s->used = true; s->direct = true; s->sock = sk; s->peer = id; s->token = token; s->state = S2_PUNCHING;
                s->last_rx = GetTickCount(); s->started = s->last_rx;
            }
        }
        if (s && s->state != S2_CLOSED) {
            s->last_rx = GetTickCount();
            peer = s->peer;
            switch (type) {
            case PKT_PUNCH:
                raw_send_on(sk, &from, PKT_PUNCH_ACK, token, NULL, 0);
                if (!s->have_path || s->state < S2_CONNECTED) { s->path = from; s->have_path = true; }
                if (s->direct && s->state != S2_CONNECTED) { s->state = S2_CONNECTED; ev = 1; }
                break;
            case PKT_PUNCH_ACK:
                if (s->state != S2_CONNECTED) { s->path = from; s->have_path = true; s->state = S2_CONNECTED; ev = 1; }
                break;
            case PKT_DATA:
                if (s->state == S2_PUNCHING || s->state == S2_OFFERED) { s->state = S2_CONNECTED; s->path = from; s->have_path = true; ev = 1; }
                if (s->state == S2_CONNECTED) handle_data_locked(s, buf + 10, n - 10, &new_data);
                break;
            case PKT_ACK:
                if (n >= 14) {
                    uint32_t ack;
                    memcpy(&ack, buf + 10, 4);
                    for (int i = 0; i < WINDOW; i++) if (s->unacked[i].used && (int32_t)(s->unacked[i].seq - ack) < 0) s->unacked[i].used = false;
                }
                break;
            case PKT_PING: raw_send_on(sk, &from, PKT_ACK, token, &s->expect_seq, 4); break;
            case PKT_CLOSE: DBG("received CLOSE"); s->state = S2_CLOSED; ev = 2; break;
            default: break;
            }
        }
        ReleaseSRWLockExclusive(&g_lock);
        if (ev) notify(ev, peer, 0);
        if (new_data) notify(3, peer, 0);
    }
}

static bool queue_send_locked(Session *s, uint32_t stream, uint8_t flags, const void *data, int len);

static DWORD WINAPI tick_thread(LPVOID unused) {
    (void)unused;
    for (;;) {
        Sleep(50);
        int n_closed = 0;
        uint64_t closed_peer[MAX_SESSIONS];
        AcquireSRWLockExclusive(&g_lock);
        DWORD now = GetTickCount();
        for (int i = 0; i < MAX_SESSIONS; i++) {
            Session *s = &g_s[i];
            if (!s->used) continue;
            if (s->state == S2_PUNCHING || s->state == S2_OFFERED) {
                for (int k = 0; k < s->n_remote; k++) { struct sockaddr_in a = to_addr(&s->remote[k]); raw_send(&a, PKT_PUNCH, s->token, NULL, 0); }
                if ((int32_t)(now - s->started) > PUNCH_TIMEOUT_MS) {
                    DBG("closing: punch timeout");
                    s->state = S2_CLOSED; closed_peer[n_closed++] = s->peer;
                }
            } else if (s->state == S2_CONNECTED) {
                while (s->outq) {
                    Pending *p = s->outq;
                    if (!queue_send_locked(s, p->stream, p->flags, p->data, p->len)) break;
                    s->outq = p->next;
                    if (!s->outq) s->outq_tail = NULL;
                    free(p->data);
                    free(p);
                }
                for (int k = 0; k < WINDOW; k++) {
                    Unacked *u = &s->unacked[k];
                    if (!u->used || (int32_t)(now - u->sent) < RTO_MS) continue;
                    if (++u->tries > MAX_TRIES) { DBG("closing: retries seq=%u", u->seq); s->state = S2_CLOSED; closed_peer[n_closed++] = s->peer; break; }
                    u->sent = now;
                    raw_send_on(SK(s), &s->path, PKT_DATA, s->token, u->data, u->len);
                }
                if ((int32_t)(now - s->last_tx) > 2000) { raw_send_on(SK(s), &s->path, PKT_PING, s->token, NULL, 0); s->last_tx = now; }
                if ((int32_t)(now - s->last_rx) > IDLE_TIMEOUT_MS && s->state == S2_CONNECTED) { DBG("closing: idle"); s->state = S2_CLOSED; closed_peer[n_closed++] = s->peer; }
            }
        }
        ReleaseSRWLockExclusive(&g_lock);
        for (int i = 0; i < n_closed; i++) notify(2, closed_peer[i], 1);
    }
    return 0;
}

static bool queue_send_locked(Session *s, uint32_t stream, uint8_t flags, const void *data, int len) {
    if (len > MTU) return false;
    uint8_t body[MTU + 16];
    uint32_t seq = (flags & 1) ? s->next_seq : 0;
    memcpy(body, &seq, 4);
    memcpy(body + 4, &stream, 4);
    body[8] = flags;
    if (len) memcpy(body + 9, data, (size_t)len);
    if (flags & 1) {
        Unacked *u = &s->unacked[seq % WINDOW];
        if (u->used) return false;
        memcpy(u->data, body, (size_t)len + 9);
        u->len = len + 9; u->seq = seq; u->sent = GetTickCount(); u->tries = 0; u->used = true;
        s->next_seq++;
    }
    raw_send_on(SK(s), &s->path, PKT_DATA, s->token, body, len + 9);
    s->last_tx = GetTickCount();
    return true;
}

static DWORD WINAPI start_thread(LPVOID unused) {
    (void)unused;
    gather_candidates();
    DBG("candidates ready: %d (port %u)", g_n_local, g_port);
    HANDLE t = CreateThread(NULL, 0, recv_thread, (LPVOID)(uintptr_t)g_sock, 0, NULL);
    if (t) CloseHandle(t);
    t = CreateThread(NULL, 0, tick_thread, NULL, 0, NULL);
    if (t) CloseHandle(t);
    shim_cm_on_push(EMSG_P2P_INFO, on_signal);
    return 0;
}

void p2p_start(void) {
    if (InterlockedCompareExchange(&g_started, 1, 0)) return;
    WSADATA d;
    WSAStartup(MAKEWORD(2, 2), &d);
    g_sock = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    int buf = 1 << 20;
    setsockopt(g_sock, SOL_SOCKET, SO_RCVBUF, (const char *)&buf, sizeof buf);
    setsockopt(g_sock, SOL_SOCKET, SO_SNDBUF, (const char *)&buf, sizeof buf);
    struct sockaddr_in a = {0};
    shim_bind_local_udp((uintptr_t)g_sock, 0, true);
    int al = sizeof a;
    getsockname(g_sock, (struct sockaddr *)&a, &al);
    g_port = ntohs(a.sin_port);
    HANDLE t = CreateThread(NULL, 0, start_thread, NULL, 0, NULL);
    if (t) CloseHandle(t);
}

static Session *ensure_locked(uint64_t peer, bool *offered) {
    Session *s = by_peer_locked(peer);
    *offered = false;
    if (s) return s;
    if (peer & DIRECT_TAG) return NULL;
    for (int i = 0; i < MAX_SESSIONS && !s; i++) if (!g_s[i].used || g_s[i].state == S2_CLOSED) s = &g_s[i];
    if (!s) return NULL;
    if (s->used) drop_queues(s);
    memset(s, 0, sizeof *s);
    s->used = true; s->initiator = true; s->peer = peer; s->token = random64(); s->state = S2_OFFERED;
    s->started = GetTickCount(); s->last_rx = s->started;
    *offered = true;
    return s;
}

static bool enqueue_locked(Session *s, uint32_t stream, uint8_t flags, const void *data, int len) {
    Pending *p = calloc(1, sizeof *p);
    if (!p) return false;
    p->data = malloc((size_t)len + 1);
    if (!p->data) { free(p); return false; }
    if (len) memcpy(p->data, data, (size_t)len);
    p->stream = stream; p->flags = flags; p->len = len;
    if (s->outq_tail) s->outq_tail->next = p; else s->outq = p;
    s->outq_tail = p;
    return true;
}

static bool send_one_locked(Session *s, uint32_t stream, uint8_t flags, const void *data, int len) {
    if (s->state == S2_CONNECTED && !s->outq && queue_send_locked(s, stream, flags, data, len)) return true;
    if (!(flags & 1) && s->state == S2_CONNECTED) return false;
    return enqueue_locked(s, stream, flags, data, len);
}

bool p2p_send(uint64_t peer, uint32_t stream, uint8_t flags, const void *data, int len) {
    if (!peer || len < 0 || len > MAX_MESSAGE || peer == self_id()) return false;
    if (len > MTU && !(flags & 1)) return false;
    p2p_start();
    bool offered;
    uint64_t token = 0;
    AcquireSRWLockExclusive(&g_lock);
    Session *s = ensure_locked(peer, &offered);
    bool ok = s != NULL;
    if (s) {
        token = s->token;
        if (len <= MTU) ok = send_one_locked(s, stream, flags, data, len);
        else {
            const uint8_t *d = data;
            for (int off = 0; off < len && ok; off += FRAG) {
                int n = len - off < FRAG ? len - off : FRAG;
                ok = send_one_locked(s, stream, (uint8_t)(flags | (off + n < len ? FLAG_MORE : 0)), d + off, n);
            }
        }
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (offered) signal_send(peer, SG_OFFER, token);
    return ok;
}

P2PMsg *p2p_take(uint64_t peer, uint32_t lo, uint32_t hi) {
    P2PMsg *found = NULL;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < MAX_SESSIONS && !found; i++) {
        Session *s = &g_s[i];
        if (!s->used || (peer && s->peer != peer)) continue;
        P2PMsg *prev = NULL;
        for (P2PMsg *m = s->inq; m; prev = m, m = m->next) {
            if (m->stream < lo || m->stream > hi) continue;
            if (prev) prev->next = m->next; else s->inq = m->next;
            if (s->inq_tail == m) s->inq_tail = prev;
            m->next = NULL;
            found = m;
            break;
        }
    }
    ReleaseSRWLockExclusive(&g_lock);
    return found;
}

int p2p_peek(uint64_t peer, uint32_t lo, uint32_t hi, uint64_t *from) {
    int len = -1;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < MAX_SESSIONS && len < 0; i++) {
        Session *s = &g_s[i];
        if (!s->used || (peer && s->peer != peer)) continue;
        for (P2PMsg *m = s->inq; m; m = m->next)
            if (m->stream >= lo && m->stream <= hi) { len = m->len; if (from) *from = m->peer; break; }
    }
    ReleaseSRWLockExclusive(&g_lock);
    return len;
}

void p2p_free(P2PMsg *m) { if (m) { free(m->data); free(m); } }

int p2p_state(uint64_t peer) {
    int st = 0;
    AcquireSRWLockExclusive(&g_lock);
    Session *s = by_peer_locked(peer);
    if (s) st = s->state == S2_CONNECTED ? 2 : 1;
    ReleaseSRWLockExclusive(&g_lock);
    return st;
}

void p2p_close(uint64_t peer) {
    uint64_t token = 0;
    AcquireSRWLockExclusive(&g_lock);
    Session *s = by_peer_locked(peer);
    if (s) {
        token = s->token;
        if (s->have_path) raw_send_on(SK(s), &s->path, PKT_CLOSE, s->token, NULL, 0);
        s->state = S2_CLOSED;
        drop_queues(s);
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (token) signal_send(peer, SG_CLOSE, token);
}

uint16_t p2p_local_port(void) { return g_port; }

void p2p_interface_requested(const char *name) {
    if (name && !strncmp(name, "SteamMatchMaking", 16)) serverlists_preload();
    if (name && !strncmp(name, "STEAMUSERSTATS_INTERFACE_VERSION", 32)) userstats_preload();
    if (name && !strncmp(name, "SteamUser0", 10)) shim_microtxn_capture_start();
    if (name && !strncmp(name, "STEAMAPPS_INTERFACE_VERSION", 27)) shim_overlay_start();
    if (!name) return;
    if (!strncmp(name, "SteamNetworking0", 16) || !strncmp(name, "SteamNetworkingMessages", 23) || !strncmp(name, "SteamNetworkingSockets", 22)) {
        if (!getenv("AURELIA_SHIM_NO_P2P")) p2p_start();
    }
}

int p2p_listen_ip(uint16_t port) {
    p2p_start();
    SOCKET sk = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    if (sk == INVALID_SOCKET) return -1;
    int buf = 1 << 20;
    setsockopt(sk, SOL_SOCKET, SO_RCVBUF, (const char *)&buf, sizeof buf);
    struct sockaddr_in a = {0};
    int al = sizeof a;
    if (!shim_bind_local_udp((uintptr_t)sk, port, false) || getsockname(sk, (struct sockaddr *)&a, &al) != 0) { closesocket(sk); return -1; }
    int slot = -1;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < 8 && slot < 0; i++) if (!g_ls[i].used) { slot = i; g_ls[i].used = true; g_ls[i].sk = sk; g_ls[i].port = ntohs(a.sin_port); }
    ReleaseSRWLockExclusive(&g_lock);
    if (slot < 0) { closesocket(sk); return -1; }
    HANDLE t = CreateThread(NULL, 0, recv_thread, (LPVOID)(uintptr_t)sk, 0, NULL);
    if (t) CloseHandle(t);
    return ntohs(a.sin_port);
}

void p2p_unlisten_ip(uint16_t port) {
    SOCKET sk = INVALID_SOCKET;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < 8; i++) if (g_ls[i].used && g_ls[i].port == port) { sk = g_ls[i].sk; g_ls[i].used = false; }
    for (int i = 0; i < MAX_SESSIONS; i++) if (g_s[i].used && g_s[i].direct && g_s[i].sock == sk && g_s[i].state != S2_CLOSED) g_s[i].state = S2_CLOSED;
    ReleaseSRWLockExclusive(&g_lock);
    if (sk != INVALID_SOCKET) closesocket(sk);
}

uint64_t p2p_connect_ip(uint32_t ip, uint16_t port) {
    p2p_start();
    uint64_t id = DIRECT_TAG | ((uint64_t)ip << 16) | port;
    AcquireSRWLockExclusive(&g_lock);
    Session *s = by_peer_locked(id);
    if (!s) {
        for (int i = 0; i < MAX_SESSIONS && !s; i++) if (!g_s[i].used || g_s[i].state == S2_CLOSED) s = &g_s[i];
        if (s) {
            if (s->used) drop_queues(s);
            memset(s, 0, sizeof *s);
            s->used = true; s->initiator = true; s->direct = true; s->sock = INVALID_SOCKET; s->peer = id; s->token = random64();
            s->remote[0].ip = ip; s->remote[0].port = port; s->n_remote = 1;
            s->state = S2_PUNCHING; s->started = GetTickCount(); s->last_rx = s->started;
        }
    }
    ReleaseSRWLockExclusive(&g_lock);
    return s ? id : 0;
}

bool p2p_is_direct(uint64_t peer) { return (peer & DIRECT_TAG) != 0; }
void p2p_direct_addr(uint64_t peer, uint32_t *ip, uint16_t *port) { *ip = (uint32_t)((peer >> 16) & 0xFFFFFFFF); *port = (uint16_t)(peer & 0xFFFF); }
