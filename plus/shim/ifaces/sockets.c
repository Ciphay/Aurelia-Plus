#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "../shim.h"

typedef int64_t i64a __attribute__((aligned(8)));

typedef struct NetMsg NetMsg;
struct NetMsg {
    void *data;
    int size;
    uint32_t conn;
    uint8_t identity[136];
    i64a conn_user;
    i64a time_received;
    i64a number;
    void (*free_data)(NetMsg *);
    void (*release)(NetMsg *);
    int channel;
    int flags;
    i64a user_data;
    uint16_t lane, pad;
};

enum {
    CB_STATUS = 1221, CB_MSG_REQUEST = 1251, CB_MSG_FAILED = 1252,
    NK_CHAN_BASE = 0x10000, NK_CHAN_END = 0x1FFFF, NK_CTRL = 0x20000, NK_DATA_BASE = 0x100000,
    NS_NONE = 0, NS_CONNECTING = 1, NS_CONNECTED = 3, NS_CLOSED_BY_PEER = 4, NS_PROBLEM = 5,
    NC_CONNECT = 1, NC_ACCEPT = 2, NC_CLOSE = 3,
    NK_SEND_RELIABLE = 8,
    NE_OK = 1, NE_FAIL = 2, NE_NOCONN = 3, NE_INVALID = 8, NE_STATE = 21,
    NID_STEAM = 16, NK_INFO_SIZE = 696, NK_STATUS_SIZE = 712,
    NEND_TIMEOUT = 4001, NEND_PEER = 4002, NEND_NO_LISTEN = 5001,
    NK_MAX_CONNS = 256, NK_MAX_LISTEN = 32, NK_MAX_GROUPS = 32, NK_MAX_MPEERS = 64, NK_MAX_PENDING_BYTES = 1 << 20,
};

typedef struct PendS { struct PendS *next; int len, flags; uint8_t *data; } PendS;
typedef struct {
    bool used, initiator, pair;
    uint32_t id, remote_id, listen, group, vport, partner;
    int state, end_reason;
    uint64_t peer;
    int64_t user_data, sent_no, recv_no;
    char name[128], end_debug[128];
    P2PMsg *lq, *lq_tail;
    PendS *ps, *ps_tail;
    int ps_bytes;
} Conn;
typedef struct { bool used, ip; uint32_t id, vport; uint16_t port; } Listen;
typedef struct { uint64_t id; bool accepted, requested; } MPeer;

static Conn g_c[NK_MAX_CONNS];
static Listen g_l[NK_MAX_LISTEN];
static bool g_g[NK_MAX_GROUPS + 1];
static MPeer g_mp[NK_MAX_MPEERS];
static uint32_t g_next = 1;
static SRWLOCK g_lock = SRWLOCK_INIT;

static uint64_t self_id(void) { const Snapshot *s = shim_snapshot(); return s ? s->steamid : 0; }

static uint64_t identity_sid(const uint8_t *id) {
    int type, cb;
    if (!id) return 0;
    memcpy(&type, id, 4);
    memcpy(&cb, id + 4, 4);
    if (type != NID_STEAM || cb != 8) return 0;
    uint64_t sid;
    memcpy(&sid, id + 8, 8);
    return sid;
}

static void fill_peer_identity(uint8_t *out, uint64_t peer);
static void fill_identity(uint8_t *out, uint64_t sid) {
    memset(out, 0, 136);
    int type = NID_STEAM, cb = 8;
    memcpy(out, &type, 4);
    memcpy(out + 4, &cb, 4);
    memcpy(out + 8, &sid, 8);
}

static void fill_ipaddr(uint8_t *a, uint32_t ip, uint16_t port) {
    memset(a, 0, 18);
    a[10] = a[11] = 0xFF;
    a[12] = (uint8_t)(ip >> 24); a[13] = (uint8_t)(ip >> 16); a[14] = (uint8_t)(ip >> 8); a[15] = (uint8_t)ip;
    memcpy(a + 16, &port, 2);
}
static void fill_peer_identity(uint8_t *out, uint64_t peer) {
    if (!p2p_is_direct(peer)) { fill_identity(out, peer); return; }
    uint32_t ip;
    uint16_t port;
    p2p_direct_addr(peer, &ip, &port);
    memset(out, 0, 136);
    int type = 1, cb = 18;
    memcpy(out, &type, 4);
    memcpy(out + 4, &cb, 4);
    fill_ipaddr(out + 8, ip, port);
}

static void release_msg(NetMsg *m) { free(m); }

static NetMsg *new_message(const void *data, int len) {
    NetMsg *m = calloc(1, sizeof *m + (size_t)len + 1);
    if (!m) return NULL;
    m->data = (uint8_t *)(m + 1);
    m->size = len;
    if (data && len) memcpy(m->data, data, (size_t)len);
    m->release = release_msg;
    LARGE_INTEGER f, n;
    QueryPerformanceFrequency(&f);
    QueryPerformanceCounter(&n);
    m->time_received = (int64_t)((double)n.QuadPart * 1000000.0 / (double)f.QuadPart);
    return m;
}

void *shim_net_allocate_message(int cb) { return cb < 0 ? NULL : new_message(NULL, cb); }

static void fill_info(const Conn *c, uint8_t *info) {
    memset(info, 0, NK_INFO_SIZE);
    fill_peer_identity(info, c->peer);
    if (p2p_is_direct(c->peer)) { uint32_t ip; uint16_t port; p2p_direct_addr(c->peer, &ip, &port); fill_ipaddr(info + 148, ip, port); }
    memcpy(info + 136, &c->user_data, 8);
    memcpy(info + 144, &c->listen, 4);
    memcpy(info + 176, &c->state, 4);
    memcpy(info + 180, &c->end_reason, 4);
    memcpy(info + 184, c->end_debug, sizeof c->end_debug);
    snprintf((char *)info + 312, 128, "P2P Aurelia %llu", (unsigned long long)c->peer);
}

static void post_status(const Conn *c, int old_state) {
    uint8_t cb[NK_STATUS_SIZE];
    memset(cb, 0, sizeof cb);
    memcpy(cb, &c->id, 4);
    fill_info(c, cb + 8);
    memcpy(cb + 704, &old_state, 4);
    shim_post_callback(CB_STATUS, cb, sizeof cb);
}

static Conn *conn_locked(uint32_t id) {
    if (!id) return NULL;
    for (int i = 0; i < NK_MAX_CONNS; i++) if (g_c[i].used && g_c[i].id == id) return &g_c[i];
    return NULL;
}

static Conn *new_conn_locked(uint64_t peer) {
    for (int i = 0; i < NK_MAX_CONNS; i++) if (!g_c[i].used) {
        Conn *c = &g_c[i];
        memset(c, 0, sizeof *c);
        c->used = true; c->id = g_next++; c->peer = peer;
        return c;
    }
    return NULL;
}

static void free_conn_locked(Conn *c) {
    for (P2PMsg *m = c->lq; m;) { P2PMsg *n = m->next; p2p_free(m); m = n; }
    for (PendS *p = c->ps; p;) { PendS *n = p->next; free(p->data); free(p); p = n; }
    memset(c, 0, sizeof *c);
}

static void control_send(uint64_t peer, uint8_t type, uint32_t a, uint32_t b) {
    uint8_t buf[9];
    buf[0] = type;
    memcpy(buf + 1, &a, 4);
    memcpy(buf + 5, &b, 4);
    p2p_send(peer, NK_CTRL, 3, buf, sizeof buf);
}

static int send_on_conn(Conn *c, const void *data, int cb, int flags) {
    if (c->pair) {
        Conn *o = conn_locked(c->partner);
        if (!o || o->state != NS_CONNECTED) return NE_NOCONN;
        P2PMsg *m = calloc(1, sizeof *m);
        if (!m) return NE_FAIL;
        m->data = malloc((size_t)cb + 1);
        if (!m->data) { free(m); return NE_FAIL; }
        if (cb) memcpy(m->data, data, (size_t)cb);
        m->len = cb; m->flags = (uint8_t)flags;
        if (o->lq_tail) o->lq_tail->next = m; else o->lq = m;
        o->lq_tail = m;
        return NE_OK;
    }
    if (c->state == NS_CONNECTING && c->initiator) {
        if (c->ps_bytes + cb > NK_MAX_PENDING_BYTES) return NE_FAIL;
        PendS *p = calloc(1, sizeof *p);
        if (!p) return NE_FAIL;
        p->data = malloc((size_t)cb + 1);
        if (!p->data) { free(p); return NE_FAIL; }
        if (cb) memcpy(p->data, data, (size_t)cb);
        p->len = cb; p->flags = flags;
        if (c->ps_tail) c->ps_tail->next = p; else c->ps = p;
        c->ps_tail = p;
        c->ps_bytes += cb;
        return NE_OK;
    }
    if (c->state != NS_CONNECTED) return NE_NOCONN;
    return p2p_send(c->peer, NK_DATA_BASE + c->remote_id, (uint8_t)((flags & NK_SEND_RELIABLE) || cb > 1100 ? 1 : 0), data, cb) ? NE_OK : NE_FAIL;
}

static void on_ctrl(uint64_t peer, const uint8_t *b, int len) {
    if (len < 9) return;
    uint32_t a, x;
    memcpy(&a, b + 1, 4);
    memcpy(&x, b + 5, 4);
    Conn snap;
    bool post = false;
    int old = 0;
    AcquireSRWLockExclusive(&g_lock);
    if (b[0] == NC_CONNECT) {
        uint32_t lid = 0;
        bool direct = p2p_is_direct(peer);
        for (int i = 0; i < NK_MAX_LISTEN; i++)
            if (g_l[i].used && !lid && (direct ? (g_l[i].ip && x == 0xFFFFFFFFu) : (!g_l[i].ip && g_l[i].vport == x))) lid = g_l[i].id;
        bool dup = false;
        for (int i = 0; i < NK_MAX_CONNS; i++) dup = dup || (g_c[i].used && !g_c[i].initiator && g_c[i].peer == peer && g_c[i].remote_id == a);
        if (dup) { ReleaseSRWLockExclusive(&g_lock); return; }
        Conn *c = lid ? new_conn_locked(peer) : NULL;
        if (!c) {
            ReleaseSRWLockExclusive(&g_lock);
            control_send(peer, NC_CLOSE, a, NEND_NO_LISTEN);
            return;
        }
        c->remote_id = a; c->listen = lid; c->vport = x; c->state = NS_CONNECTING;
        snap = *c; post = true;
    } else if (b[0] == NC_ACCEPT) {
        Conn *c = conn_locked(a);
        if (c && c->initiator && c->state == NS_CONNECTING) {
            c->remote_id = x; c->state = NS_CONNECTED; old = NS_CONNECTING;
            PendS *ps = c->ps;
            c->ps = c->ps_tail = NULL;
            c->ps_bytes = 0;
            snap = *c; post = true;
            ReleaseSRWLockExclusive(&g_lock);
            post_status(&snap, old);
            for (PendS *p = ps; p;) {
                PendS *n = p->next;
                p2p_send(peer, NK_DATA_BASE + snap.remote_id, (uint8_t)((p->flags & NK_SEND_RELIABLE) || p->len > 1100 ? 1 : 0), p->data, p->len);
                free(p->data); free(p);
                p = n;
            }
            return;
        }
    } else if (b[0] == NC_CLOSE) {
        Conn *c = conn_locked(a);
        if (c && c->state != NS_CLOSED_BY_PEER && c->state != NS_PROBLEM) {
            old = c->state; c->state = NS_CLOSED_BY_PEER; c->end_reason = (int)x;
            snprintf(c->end_debug, sizeof c->end_debug, "closed by the peer");
            snap = *c; post = true;
        }
    }
    ReleaseSRWLockExclusive(&g_lock);
    if (post) post_status(&snap, old);
}

static void on_event(int ev, uint64_t peer, int arg) {
    if (ev == 3) {
        P2PMsg *m;
        while ((m = p2p_take(peer, NK_CTRL, NK_CTRL))) { on_ctrl(peer, m->data, m->len); p2p_free(m); }
        bool ask = false;
        AcquireSRWLockExclusive(&g_lock);
        MPeer *mp = NULL, *freep = NULL;
        for (int i = 0; i < NK_MAX_MPEERS; i++) { if (g_mp[i].id == peer) mp = &g_mp[i]; else if (!g_mp[i].id && !freep) freep = &g_mp[i]; }
        if (!mp && freep) { mp = freep; mp->id = peer; }
        if (mp && !mp->accepted && !mp->requested && p2p_peek(peer, NK_CHAN_BASE, NK_CHAN_END, NULL) >= 0) { mp->requested = true; ask = true; }
        ReleaseSRWLockExclusive(&g_lock);
        if (ask) { uint8_t id[136]; fill_identity(id, peer); shim_post_callback(CB_MSG_REQUEST, id, sizeof id); }
    } else if (ev == 2) {
        Conn snaps[16];
        int olds[16], n = 0;
        AcquireSRWLockExclusive(&g_lock);
        for (int i = 0; i < NK_MAX_CONNS && n < 16; i++) {
            Conn *c = &g_c[i];
            if (!c->used || c->pair || c->peer != peer || (c->state != NS_CONNECTING && c->state != NS_CONNECTED)) continue;
            olds[n] = c->state;
            c->state = arg == 1 ? NS_PROBLEM : NS_CLOSED_BY_PEER;
            c->end_reason = arg == 1 ? NEND_TIMEOUT : NEND_PEER;
            snprintf(c->end_debug, sizeof c->end_debug, arg == 1 ? "no response from the peer" : "session ended by the peer");
            snaps[n++] = *c;
        }
        bool wanted = false;
        for (int i = 0; i < NK_MAX_MPEERS; i++) if (g_mp[i].id == peer) { wanted = g_mp[i].accepted; memset(&g_mp[i], 0, sizeof g_mp[i]); }
        ReleaseSRWLockExclusive(&g_lock);
        for (int i = 0; i < n; i++) post_status(&snaps[i], olds[i]);
        if (wanted && arg == 1) {
            Conn tmp = {0};
            tmp.peer = peer; tmp.state = NS_PROBLEM; tmp.end_reason = NEND_TIMEOUT;
            uint8_t info[NK_INFO_SIZE];
            fill_info(&tmp, info);
            shim_post_callback(CB_MSG_FAILED, info, sizeof info);
        }
    }
}

static void start_net(void) {
    p2p_start();
    p2p_set_event(on_event);
}

static MPeer *mpeer_locked(uint64_t id, bool create) {
    for (int i = 0; i < NK_MAX_MPEERS; i++) if (g_mp[i].id == id) return &g_mp[i];
    if (!create) return NULL;
    for (int i = 0; i < NK_MAX_MPEERS; i++) if (!g_mp[i].id) { g_mp[i].id = id; return &g_mp[i]; }
    return NULL;
}

static int SHIM_TC msg_send_to_user(void *self, const uint8_t *identity, const void *data, uint32_t cb, int flags, int channel) {
    GNS_DELEGATE(shim_gns_msg_send(identity, data, cb, flags, channel));
    (void)self;
    uint64_t peer = identity_sid(identity);
    if (!peer || channel < 0 || channel > 0xFFFE || (cb && !data)) return NE_INVALID;
    start_net();
    AcquireSRWLockExclusive(&g_lock);
    MPeer *mp = mpeer_locked(peer, true);
    if (mp) mp->accepted = true;
    ReleaseSRWLockExclusive(&g_lock);
    return p2p_send(peer, NK_CHAN_BASE + (uint32_t)channel, (uint8_t)((flags & NK_SEND_RELIABLE) || cb > 1100 ? 1 : 0), data, (int)cb) ? NE_OK : NE_FAIL;
}

static int SHIM_TC msg_receive(void *self, int channel, NetMsg **out, int max) {
    GNS_DELEGATE(shim_gns_msg_receive(channel, (void **)out, max));
    (void)self;
    if (!out || max <= 0 || channel < 0 || channel > 0xFFFE) return 0;
    start_net();
    uint64_t peers[NK_MAX_MPEERS];
    int np = 0;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < NK_MAX_MPEERS; i++) if (g_mp[i].id && g_mp[i].accepted) peers[np++] = g_mp[i].id;
    ReleaseSRWLockExclusive(&g_lock);
    int n = 0;
    for (int i = 0; i < np && n < max; i++) {
        P2PMsg *m;
        while (n < max && (m = p2p_take(peers[i], NK_CHAN_BASE + (uint32_t)channel, NK_CHAN_BASE + (uint32_t)channel))) {
            NetMsg *nm = new_message(m->data, m->len);
            if (nm) {
                fill_identity(nm->identity, m->peer);
                nm->channel = channel;
                nm->flags = (m->flags & 1) ? NK_SEND_RELIABLE : 0;
                out[n++] = nm;
            }
            p2p_free(m);
        }
    }
    return n;
}

static uint64_t SHIM_TC msg_accept(void *self, const uint8_t *identity) {
    GNS_DELEGATE((uint64_t)shim_gns_msg_accept(identity));
    (void)self;
    uint64_t peer = identity_sid(identity);
    if (!peer) return 0;
    start_net();
    AcquireSRWLockExclusive(&g_lock);
    MPeer *mp = mpeer_locked(peer, true);
    if (mp) mp->accepted = true;
    ReleaseSRWLockExclusive(&g_lock);
    return mp != NULL;
}

static uint64_t SHIM_TC msg_close_session(void *self, const uint8_t *identity) {
    GNS_DELEGATE((uint64_t)shim_gns_msg_close_session(identity));
    (void)self;
    uint64_t peer = identity_sid(identity);
    if (!peer) return 0;
    AcquireSRWLockExclusive(&g_lock);
    MPeer *mp = mpeer_locked(peer, false);
    bool known = mp != NULL;
    if (mp) memset(mp, 0, sizeof *mp);
    ReleaseSRWLockExclusive(&g_lock);
    P2PMsg *m;
    while ((m = p2p_take(peer, NK_CHAN_BASE, NK_CHAN_END))) p2p_free(m);
    return known;
}

static uint64_t SHIM_TC msg_close_channel(void *self, const uint8_t *identity, int channel) {
    GNS_DELEGATE((uint64_t)shim_gns_msg_close_channel(identity, channel));
    (void)self;
    uint64_t peer = identity_sid(identity);
    if (!peer || channel < 0 || channel > 0xFFFE) return 0;
    bool any = false;
    P2PMsg *m;
    while ((m = p2p_take(peer, NK_CHAN_BASE + (uint32_t)channel, NK_CHAN_BASE + (uint32_t)channel))) { p2p_free(m); any = true; }
    return any;
}

static int SHIM_TC msg_session_info(void *self, const uint8_t *identity, uint8_t *info, uint8_t *status) {
    GNS_DELEGATE(shim_gns_msg_session_info(identity, info, status));
    (void)self;
    uint64_t peer = identity_sid(identity);
    int st = peer ? p2p_state(peer) : 0;
    Conn tmp = {0};
    tmp.peer = peer;
    tmp.state = st == 2 ? NS_CONNECTED : st == 1 ? NS_CONNECTING : NS_NONE;
    if (info) fill_info(&tmp, info);
    if (status) { memset(status, 0, 64); memcpy(status, &tmp.state, 4); }
    return tmp.state;
}

static uint32_t SHIM_TC sock_listen_p2p(void *self, int vport, int n, const void *opts) {
    GNS_DELEGATE(shim_gns_listen_p2p(vport, n, opts));
    (void)self; (void)n; (void)opts;
    start_net();
    uint32_t id = 0;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < NK_MAX_LISTEN && !id; i++) if (!g_l[i].used) { g_l[i].used = true; g_l[i].id = g_next++; g_l[i].vport = (uint32_t)vport; id = g_l[i].id; }
    ReleaseSRWLockExclusive(&g_lock);
    return id;
}

static uint32_t SHIM_TC sock_connect_p2p(void *self, const uint8_t *identity, int vport, int n, const void *opts) {
    GNS_DELEGATE(shim_gns_connect_p2p(identity, vport, n, opts));
    (void)self; (void)n; (void)opts;
    uint64_t peer = identity_sid(identity);
    if (!peer || peer == self_id()) return 0;
    start_net();
    AcquireSRWLockExclusive(&g_lock);
    Conn *c = new_conn_locked(peer);
    Conn snap = {0};
    if (c) { c->initiator = true; c->vport = (uint32_t)vport; c->state = NS_CONNECTING; snap = *c; }
    ReleaseSRWLockExclusive(&g_lock);
    if (!c) return 0;
    post_status(&snap, NS_NONE);
    control_send(peer, NC_CONNECT, snap.id, (uint32_t)vport);
    return snap.id;
}

static uint32_t SHIM_TC sock_listen_ip(void *self, const uint8_t *addr, int n, const void *opts) {
    GNS_DELEGATE(shim_gns_listen_ip(addr, n, opts));
    (void)self; (void)n; (void)opts;
    uint16_t want = 0;
    if (addr) memcpy(&want, addr + 16, 2);
    start_net();
    int port = p2p_listen_ip(want);
    if (port < 0) return 0;
    uint32_t id = 0;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < NK_MAX_LISTEN && !id; i++) if (!g_l[i].used) { g_l[i].used = true; g_l[i].ip = true; g_l[i].port = (uint16_t)port; g_l[i].id = g_next++; id = g_l[i].id; }
    ReleaseSRWLockExclusive(&g_lock);
    if (!id) p2p_unlisten_ip((uint16_t)port);
    return id;
}

static uint32_t SHIM_TC sock_connect_ip(void *self, const uint8_t *addr, int n, const void *opts) {
    GNS_DELEGATE(shim_gns_connect_ip(addr, n, opts));
    (void)self; (void)n; (void)opts;
    if (!addr) return 0;
    bool mapped = addr[10] == 0xFF && addr[11] == 0xFF, zero = true;
    for (int i = 0; i < 16; i++) if (addr[i]) zero = false;
    if (!mapped && !zero) return 0;
    uint32_t ip = zero ? 0x7F000001u : ((uint32_t)addr[12] << 24 | (uint32_t)addr[13] << 16 | (uint32_t)addr[14] << 8 | addr[15]);
    uint16_t port;
    memcpy(&port, addr + 16, 2);
    if (!port) return 0;
    start_net();
    uint64_t peer = p2p_connect_ip(ip, port);
    if (!peer) return 0;
    AcquireSRWLockExclusive(&g_lock);
    Conn *c = new_conn_locked(peer);
    Conn snap = {0};
    if (c) { c->initiator = true; c->vport = 0xFFFFFFFFu; c->state = NS_CONNECTING; snap = *c; }
    ReleaseSRWLockExclusive(&g_lock);
    if (!c) return 0;
    post_status(&snap, NS_NONE);
    control_send(peer, NC_CONNECT, snap.id, 0xFFFFFFFFu);
    return snap.id;
}

static int SHIM_TC sock_accept(void *self, uint32_t id) {
    GNS_DELEGATE(shim_gns_accept(id));
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Conn *c = conn_locked(id);
    if (!c) { ReleaseSRWLockExclusive(&g_lock); return NE_INVALID; }
    if (c->initiator || c->state != NS_CONNECTING) { ReleaseSRWLockExclusive(&g_lock); return NE_STATE; }
    c->state = NS_CONNECTED;
    Conn snap = *c;
    ReleaseSRWLockExclusive(&g_lock);
    control_send(snap.peer, NC_ACCEPT, snap.remote_id, snap.id);
    post_status(&snap, NS_CONNECTING);
    return NE_OK;
}

static uint64_t SHIM_TC sock_close_conn(void *self, uint32_t id, int reason, const char *debug, uint8_t linger) {
    GNS_DELEGATE((uint64_t)shim_gns_close_conn(id, reason, debug, linger));
    (void)self; (void)reason; (void)debug; (void)linger;
    AcquireSRWLockExclusive(&g_lock);
    Conn *c = conn_locked(id);
    if (!c) { ReleaseSRWLockExclusive(&g_lock); return 0; }
    Conn snap = *c;
    Conn *o = c->pair ? conn_locked(c->partner) : NULL;
    Conn osnap = {0};
    int oold = 0;
    if (o && o->state == NS_CONNECTED) { oold = o->state; o->state = NS_CLOSED_BY_PEER; o->end_reason = reason; osnap = *o; }
    free_conn_locked(c);
    ReleaseSRWLockExclusive(&g_lock);
    if (osnap.id) post_status(&osnap, oold);
    if (!snap.pair && snap.remote_id && snap.state != NS_CLOSED_BY_PEER && snap.state != NS_PROBLEM) control_send(snap.peer, NC_CLOSE, snap.remote_id, (uint32_t)reason);
    if (!snap.pair) { P2PMsg *m; while ((m = p2p_take(snap.peer, NK_DATA_BASE + snap.id, NK_DATA_BASE + snap.id))) p2p_free(m); }
    return 1;
}

static uint64_t SHIM_TC sock_close_listen(void *self, uint32_t id) {
    GNS_DELEGATE((uint64_t)shim_gns_close_listen(id));
    (void)self;
    Conn snaps[16];
    int n = 0;
    AcquireSRWLockExclusive(&g_lock);
    bool found = false;
    uint16_t ipport = 0;
    for (int i = 0; i < NK_MAX_LISTEN; i++) if (g_l[i].used && g_l[i].id == id) { if (g_l[i].ip) ipport = g_l[i].port; memset(&g_l[i], 0, sizeof g_l[i]); found = true; }
    for (int i = 0; i < NK_MAX_CONNS && n < 16; i++) {
        Conn *c = &g_c[i];
        if (found && c->used && c->listen == id && !c->initiator && c->state == NS_CONNECTING) { snaps[n++] = *c; free_conn_locked(c); }
    }
    ReleaseSRWLockExclusive(&g_lock);
    for (int i = 0; i < n; i++) control_send(snaps[i].peer, NC_CLOSE, snaps[i].remote_id, NEND_NO_LISTEN);
    if (ipport) p2p_unlisten_ip(ipport);
    return found;
}

static uint64_t SHIM_TC sock_set_user_data(void *self, uint32_t id, int64_t v) {
    GNS_DELEGATE((uint64_t)shim_gns_set_user_data(id, v));
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Conn *c = conn_locked(id);
    if (c) c->user_data = v;
    ReleaseSRWLockExclusive(&g_lock);
    return c != NULL;
}
static int64_t SHIM_TC sock_get_user_data(void *self, uint32_t id) {
    GNS_DELEGATE(shim_gns_get_user_data(id));
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Conn *c = conn_locked(id);
    int64_t v = c ? c->user_data : -1;
    ReleaseSRWLockExclusive(&g_lock);
    return v;
}
static void SHIM_TC sock_set_name(void *self, uint32_t id, const char *name) {
    GNS_DELEGATE_VOID(shim_gns_set_name(id, name));
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Conn *c = conn_locked(id);
    if (c && name) snprintf(c->name, sizeof c->name, "%s", name);
    ReleaseSRWLockExclusive(&g_lock);
}
static uint64_t SHIM_TC sock_get_name(void *self, uint32_t id, char *out, int cap) {
    GNS_DELEGATE((uint64_t)shim_gns_get_name(id, out, cap));
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Conn *c = conn_locked(id);
    bool ok = c && out && cap > 0;
    if (ok) snprintf(out, (size_t)cap, "%s", c->name);
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static int SHIM_TC sock_send(void *self, uint32_t id, const void *data, uint32_t cb, int flags, int64_t *number) {
    GNS_DELEGATE(shim_gns_send(id, data, cb, flags, number));
    (void)self;
    if (cb && !data) return NE_INVALID;
    AcquireSRWLockExclusive(&g_lock);
    Conn *c = conn_locked(id);
    int r = c ? send_on_conn(c, data, (int)cb, flags) : NE_INVALID;
    if (c && r == NE_OK && number) *number = ++c->sent_no;
    ReleaseSRWLockExclusive(&g_lock);
    return r;
}

static void send_messages(int n, NetMsg *const *msgs, int64_t *results) {
    GNS_DELEGATE_VOID(shim_gns_send_messages(n, (void *const *)msgs, results));
    for (int i = 0; i < n; i++) {
        NetMsg *m = msgs ? msgs[i] : NULL;
        if (!m) { if (results) results[i] = -NE_INVALID; continue; }
        int64_t number = 0;
        int r = sock_send(NULL, m->conn, m->data, (uint32_t)m->size, m->flags, &number);
        if (results) results[i] = r == NE_OK ? number : -r;
        if (m->release) m->release(m);
    }
}
static void SHIM_TC sock_send_messages(void *self, int n, NetMsg *const *msgs, int64_t *results) { (void)self; send_messages(n, msgs, results); }
static void SHIM_TC sock_send_messages13(void *self, int n, NetMsg *const *msgs, int64_t *results, uint32_t unused) { (void)self; (void)unused; send_messages(n, msgs, results); }

static int SHIM_TC sock_flush(void *self, uint32_t id) {
    GNS_DELEGATE(shim_gns_flush(id));
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    bool known = conn_locked(id) != NULL;
    ReleaseSRWLockExclusive(&g_lock);
    return known ? NE_OK : NE_INVALID;
}

static int fetch(Conn snap, NetMsg **out, int max) {
    int n = 0;
    if (snap.pair) {
        while (n < max) {
            AcquireSRWLockExclusive(&g_lock);
            Conn *c = conn_locked(snap.id);
            P2PMsg *m = c ? c->lq : NULL;
            if (m) { c->lq = m->next; if (!c->lq) c->lq_tail = NULL; m->next = NULL; }
            int64_t no = c && m ? ++c->recv_no : 0;
            ReleaseSRWLockExclusive(&g_lock);
            if (!m) break;
            NetMsg *nm = new_message(m->data, m->len);
            if (nm) { nm->conn = snap.id; fill_peer_identity(nm->identity, snap.peer); nm->flags = m->flags; nm->conn_user = snap.user_data; nm->number = no; out[n++] = nm; }
            p2p_free(m);
        }
        return n;
    }
    P2PMsg *m;
    while (n < max && (m = p2p_take(snap.peer, NK_DATA_BASE + snap.id, NK_DATA_BASE + snap.id))) {
        NetMsg *nm = new_message(m->data, m->len);
        if (nm) {
            AcquireSRWLockExclusive(&g_lock);
            Conn *c = conn_locked(snap.id);
            nm->number = c ? ++c->recv_no : 0;
            ReleaseSRWLockExclusive(&g_lock);
            nm->conn = snap.id; fill_peer_identity(nm->identity, snap.peer); nm->flags = (m->flags & 1) ? NK_SEND_RELIABLE : 0; nm->conn_user = snap.user_data;
            out[n++] = nm;
        }
        p2p_free(m);
    }
    return n;
}

static int SHIM_TC sock_receive(void *self, uint32_t id, NetMsg **out, int max) {
    GNS_DELEGATE(shim_gns_receive(id, (void **)out, max));
    (void)self;
    if (!out || max <= 0) return 0;
    AcquireSRWLockExclusive(&g_lock);
    Conn *c = conn_locked(id);
    Conn snap = c ? *c : (Conn){0};
    ReleaseSRWLockExclusive(&g_lock);
    return c ? fetch(snap, out, max) : -1;
}

static int SHIM_TC sock_receive_group(void *self, uint32_t group, NetMsg **out, int max) {
    GNS_DELEGATE(shim_gns_receive_group(group, (void **)out, max));
    (void)self;
    if (!out || max <= 0 || group == 0) return 0;
    uint32_t ids[NK_MAX_CONNS];
    int ni = 0;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < NK_MAX_CONNS; i++) if (g_c[i].used && g_c[i].group == group) ids[ni++] = g_c[i].id;
    ReleaseSRWLockExclusive(&g_lock);
    int n = 0;
    for (int i = 0; i < ni && n < max; i++) {
        AcquireSRWLockExclusive(&g_lock);
        Conn *c = conn_locked(ids[i]);
        Conn snap = c ? *c : (Conn){0};
        ReleaseSRWLockExclusive(&g_lock);
        if (c) n += fetch(snap, out + n, max - n);
    }
    return n;
}

static uint64_t SHIM_TC sock_get_info(void *self, uint32_t id, uint8_t *info) {
    GNS_DELEGATE((uint64_t)shim_gns_get_info(id, info));
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Conn *c = conn_locked(id);
    if (c && info) fill_info(c, info);
    ReleaseSRWLockExclusive(&g_lock);
    return c != NULL;
}

static int SHIM_TC sock_realtime(void *self, uint32_t id, uint8_t *status, int lanes, void *lane_status) {
    GNS_DELEGATE(shim_gns_realtime(id, status, lanes, lane_status));
    (void)self; (void)lanes; (void)lane_status;
    AcquireSRWLockExclusive(&g_lock);
    Conn *c = conn_locked(id);
    int st = c ? c->state : -1;
    ReleaseSRWLockExclusive(&g_lock);
    if (st < 0) return NE_NOCONN;
    if (status) { memset(status, 0, 64); memcpy(status, &st, 4); }
    return NE_OK;
}
static uint64_t SHIM_TC sock_quick(void *self, uint32_t id, uint8_t *status) { return sock_realtime(self, id, status, 0, NULL) == NE_OK; }

static int SHIM_TC sock_detailed(void *self, uint32_t id, char *out, int cap) {
    GNS_DELEGATE(shim_gns_detailed(id, out, cap));
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Conn *c = conn_locked(id);
    int st = c ? c->state : -1;
    uint64_t peer = c ? c->peer : 0;
    ReleaseSRWLockExclusive(&g_lock);
    if (st < 0) return -1;
    if (out && cap > 0) snprintf(out, (size_t)cap, "P2P Aurelia Plus peer=%llu state=%d", (unsigned long long)peer, st);
    return 0;
}

static uint64_t SHIM_TC sock_listen_address(void *self, uint32_t id, uint8_t *addr) {
    GNS_DELEGATE((uint64_t)shim_gns_listen_address(id, addr));
    (void)self;
    uint16_t port = 0;
    bool ok = false;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < NK_MAX_LISTEN; i++) if (g_l[i].used && g_l[i].id == id && g_l[i].ip) { port = g_l[i].port; ok = true; }
    ReleaseSRWLockExclusive(&g_lock);
    if (ok && addr) { memset(addr, 0, 18); memcpy(addr + 16, &port, 2); }
    return ok;
}

static uint64_t SHIM_TC sock_socket_pair(void *self, uint32_t *a, uint32_t *b, uint8_t loopback, const uint8_t *ida, const uint8_t *idb) {
    GNS_DELEGATE((uint64_t)shim_gns_socket_pair(a, b, loopback, ida, idb));
    (void)self; (void)loopback;
    if (!a || !b) return 0;
    AcquireSRWLockExclusive(&g_lock);
    Conn *x = new_conn_locked(identity_sid(ida) ? identity_sid(ida) : self_id());
    Conn *y = x ? new_conn_locked(identity_sid(idb) ? identity_sid(idb) : self_id()) : NULL;
    if (x && !y) free_conn_locked(x);
    if (x && y) {
        x->pair = y->pair = true; x->state = y->state = NS_CONNECTED; x->partner = y->id; y->partner = x->id;
        *a = x->id; *b = y->id;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return x && y;
}

static uint64_t SHIM_TC sock_get_identity(void *self, uint8_t *id) {
    GNS_DELEGATE((uint64_t)shim_gns_get_identity(id));
    (void)self;
    if (!id) return 0;
    fill_identity(id, self_id());
    return 1;
}

enum { AVAIL_CURRENT = 100 };
static int SHIM_TC sock_init_auth(void *self) { (void)self; GNS_DELEGATE(shim_gns_init_auth()); return AVAIL_CURRENT; }
static int SHIM_TC sock_auth_status(void *self, uint8_t *out) {
    GNS_DELEGATE(shim_gns_auth_status(out));
    (void)self;
    if (out) { memset(out, 0, 4 + 256); int v = AVAIL_CURRENT; memcpy(out, &v, 4); snprintf((char *)out + 4, 256, "authentication not required (Aurelia Plus P2P transport)"); }
    return AVAIL_CURRENT;
}

static uint32_t SHIM_TC sock_create_group(void *self) {
    GNS_DELEGATE(shim_gns_create_group());
    (void)self;
    uint32_t id = 0;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 1; i <= NK_MAX_GROUPS && !id; i++) if (!g_g[i]) { g_g[i] = true; id = (uint32_t)i; }
    ReleaseSRWLockExclusive(&g_lock);
    return id;
}
static uint64_t SHIM_TC sock_destroy_group(void *self, uint32_t id) {
    GNS_DELEGATE((uint64_t)shim_gns_destroy_group(id));
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    bool ok = id >= 1 && id <= NK_MAX_GROUPS && g_g[id];
    if (ok) {
        g_g[id] = false;
        for (int i = 0; i < NK_MAX_CONNS; i++) if (g_c[i].used && g_c[i].group == id) g_c[i].group = 0;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}
static uint64_t SHIM_TC sock_set_group(void *self, uint32_t conn, uint32_t group) {
    GNS_DELEGATE((uint64_t)shim_gns_set_group(conn, group));
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Conn *c = conn_locked(conn);
    bool ok = c && (group == 0 || (group <= NK_MAX_GROUPS && g_g[group]));
    if (ok) c->group = group;
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static uint32_t SHIM_TC sock_listen_p2p_old(void *self, int vport) { return sock_listen_p2p(self, vport, 0, NULL); }
static uint32_t SHIM_TC sock_connect_p2p_old(void *self, const uint8_t *identity, int vport) { return sock_connect_p2p(self, identity, vport, 0, NULL); }
static uint32_t SHIM_TC sock_listen_ip_old(void *self, const uint8_t *addr) { return sock_listen_ip(self, addr, 0, NULL); }
static uint32_t SHIM_TC sock_connect_ip_old(void *self, const uint8_t *addr) { return sock_connect_ip(self, addr, 0, NULL); }
static int SHIM_TC sock_send_old(void *self, uint32_t id, const void *data, uint32_t cb, int flags) { return sock_send(self, id, data, cb, flags, NULL); }

static void SHIM_TC sock_noop(void *self) { (void)self; }
static void SHIM_TC sock_run_callbacks(void *self) { (void)self; GNS_DELEGATE_VOID(shim_gns_run_callbacks()); }

void register_sockets(void) {
    p2p_set_event(on_event);
    const char *m = "SteamNetworkingMessages0";
    shim_register_family(m, "SendMessageToUser", (void *)msg_send_to_user);
    shim_register_family(m, "ReceiveMessagesOnChannel", (void *)msg_receive);
    shim_register_family(m, "AcceptSessionWithUser", (void *)msg_accept);
    shim_register_family(m, "CloseSessionWithUser", (void *)msg_close_session);
    shim_register_family(m, "CloseChannelWithUser", (void *)msg_close_channel);
    shim_register_family(m, "GetSessionConnectionInfo", (void *)msg_session_info);

    const char *s = "SteamNetworkingSockets0";
    shim_register_family_ab(s, "CreateListenSocketIP", 12, (void *)sock_listen_ip);
    shim_register_family_ab(s, "CreateListenSocketIP", 4, (void *)sock_listen_ip_old);
    shim_register_family_ab(s, "ConnectByIPAddress", 12, (void *)sock_connect_ip);
    shim_register_family_ab(s, "ConnectByIPAddress", 4, (void *)sock_connect_ip_old);
    shim_register_family_ab(s, "CreateListenSocketP2P", 12, (void *)sock_listen_p2p);
    shim_register_family_ab(s, "CreateListenSocketP2P", 4, (void *)sock_listen_p2p_old);
    shim_register_family_ab(s, "ConnectP2P", 16, (void *)sock_connect_p2p);
    shim_register_family_ab(s, "ConnectP2P", 8, (void *)sock_connect_p2p_old);
    shim_register_family(s, "AcceptConnection", (void *)sock_accept);
    shim_register_family(s, "CloseConnection", (void *)sock_close_conn);
    shim_register_family(s, "CloseListenSocket", (void *)sock_close_listen);
    shim_register_family(s, "SetConnectionUserData", (void *)sock_set_user_data);
    shim_register_family(s, "GetConnectionUserData", (void *)sock_get_user_data);
    shim_register_family(s, "SetConnectionName", (void *)sock_set_name);
    shim_register_family(s, "GetConnectionName", (void *)sock_get_name);
    shim_register_family_ab(s, "SendMessageToConnection", 20, (void *)sock_send);
    shim_register_family_ab(s, "SendMessageToConnection", 16, (void *)sock_send_old);
    shim_register_family_ab(s, "SendMessages", 12, (void *)sock_send_messages);
    shim_register_family_ab(s, "SendMessages", 16, (void *)sock_send_messages13);
    shim_register_family(s, "FlushMessagesOnConnection", (void *)sock_flush);
    shim_register_family(s, "ReceiveMessagesOnConnection", (void *)sock_receive);
    shim_register_family(s, "GetConnectionInfo", (void *)sock_get_info);
    shim_register_family(s, "GetConnectionRealTimeStatus", (void *)sock_realtime);
    shim_register_family(s, "GetQuickConnectionStatus", (void *)sock_quick);
    shim_register_family(s, "GetDetailedConnectionStatus", (void *)sock_detailed);
    shim_register_family(s, "GetListenSocketAddress", (void *)sock_listen_address);
    shim_register_family(s, "CreateSocketPair", (void *)sock_socket_pair);
    shim_register_family(s, "GetIdentity", (void *)sock_get_identity);
    shim_register_family(s, "InitAuthentication", (void *)sock_init_auth);
    shim_register_family(s, "GetAuthenticationStatus", (void *)sock_auth_status);
    shim_register_family(s, "CreatePollGroup", (void *)sock_create_group);
    shim_register_family(s, "DestroyPollGroup", (void *)sock_destroy_group);
    shim_register_family(s, "SetConnectionPollGroup", (void *)sock_set_group);
    shim_register_family(s, "ReceiveMessagesOnPollGroup", (void *)sock_receive_group);
    shim_register_family(s, "RunCallbacks", (void *)sock_run_callbacks);
}
