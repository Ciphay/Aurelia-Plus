#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "../shim.h"

enum { CB_REQUEST = 1202, CB_FAIL = 1203, MAX_PEERS = 64, ERR_TIMEOUT = 4, SEND_RELIABLE = 2, SEND_RELIABLE_BUFFERED = 3, MAX_CHANNEL = 0xFFFF };

typedef struct { uint64_t id; bool accepted, requested; } Peer;
static Peer g_peers[MAX_PEERS];
static SRWLOCK g_lock = SRWLOCK_INIT;

static Peer *peer_locked(uint64_t id, bool create) {
    if (!id) return NULL;
    for (int i = 0; i < MAX_PEERS; i++) if (g_peers[i].id == id) return &g_peers[i];
    if (!create) return NULL;
    for (int i = 0; i < MAX_PEERS; i++) if (!g_peers[i].id) { g_peers[i].id = id; return &g_peers[i]; }
    return NULL;
}

static void on_event(int ev, uint64_t peer, int arg) {
    if (ev == 3) {
        bool ask = false;
        AcquireSRWLockExclusive(&g_lock);
        Peer *p = peer_locked(peer, true);
        if (p && !p->accepted && !p->requested && p2p_peek(peer, 0, MAX_CHANNEL, NULL) >= 0) { p->requested = true; ask = true; }
        ReleaseSRWLockExclusive(&g_lock);
        if (ask) shim_post_callback(CB_REQUEST, &peer, 8);
    } else if (ev == 2) {
        AcquireSRWLockExclusive(&g_lock);
        Peer *p = peer_locked(peer, false);
        bool wanted = p && p->accepted;
        if (p) memset(p, 0, sizeof *p);
        ReleaseSRWLockExclusive(&g_lock);
        if (wanted && arg == 1) {
            uint8_t cb[16] = {0};
            memcpy(cb, &peer, 8);
            cb[8] = ERR_TIMEOUT;
            shim_post_callback(CB_FAIL, cb, sizeof cb);
        }
    }
}

static bool accepted(uint64_t id) {
    AcquireSRWLockExclusive(&g_lock);
    Peer *p = peer_locked(id, false);
    bool ok = p && p->accepted;
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static uint64_t first_accepted_with_data(int channel) {
    for (int i = 0; i < MAX_PEERS; i++) {
        uint64_t id = g_peers[i].id;
        if (id && g_peers[i].accepted && p2p_peek(id, (uint32_t)channel, (uint32_t)channel, NULL) >= 0) return id;
    }
    return 0;
}

static uint64_t SHIM_TC send_p2p(void *self, uint64_t remote, const void *data, uint32_t cb, int type, int channel) {
    (void)self;
    shim_trace("SendP2PPacket to=%llu bytes=%u type=%d channel=%d", (unsigned long long)remote, cb, type, channel);
    if (channel < 0 || channel > MAX_CHANNEL || (cb && !data) || cb > 1100) return 0;
    AcquireSRWLockExclusive(&g_lock);
    Peer *p = peer_locked(remote, true);
    if (p) p->accepted = true;
    ReleaseSRWLockExclusive(&g_lock);
    p2p_set_event(on_event);
    return p2p_send(remote, (uint32_t)channel, (uint8_t)(type == SEND_RELIABLE || type == SEND_RELIABLE_BUFFERED ? 1 : 0), data, (int)cb);
}
static uint64_t SHIM_TC send_p2p_v3(void *self, uint64_t remote, const void *data, uint32_t cb, int type) { return send_p2p(self, remote, data, cb, type, 0); }

static uint64_t SHIM_TC is_available(void *self, uint32_t *size, int channel) {
    (void)self;
    p2p_start();
    p2p_set_event(on_event);
    AcquireSRWLockExclusive(&g_lock);
    uint64_t id = first_accepted_with_data(channel);
    ReleaseSRWLockExclusive(&g_lock);
    int n = id ? p2p_peek(id, (uint32_t)channel, (uint32_t)channel, NULL) : -1;
    if (n < 0) { if (size) *size = 0; return 0; }
    if (size) *size = (uint32_t)n;
    return 1;
}
static uint64_t SHIM_TC is_available_v3(void *self, uint32_t *size) { return is_available(self, size, 0); }

static uint64_t SHIM_TC read_p2p(void *self, void *dest, uint32_t cap, uint32_t *size, uint64_t *remote, int channel) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    uint64_t id = first_accepted_with_data(channel);
    ReleaseSRWLockExclusive(&g_lock);
    if (!id) return 0;
    P2PMsg *m = p2p_take(id, (uint32_t)channel, (uint32_t)channel);
    if (!m) return 0;
    uint32_t n = (uint32_t)m->len < cap ? (uint32_t)m->len : cap;
    if (dest && n) memcpy(dest, m->data, n);
    if (size) *size = n;
    if (remote) *remote = m->peer;
    shim_trace("ReadP2PPacket from=%llu bytes=%u channel=%d", (unsigned long long)m->peer, n, channel);
    p2p_free(m);
    return 1;
}
static uint64_t SHIM_TC read_p2p_v3(void *self, void *dest, uint32_t cap, uint32_t *size, uint64_t *remote) { return read_p2p(self, dest, cap, size, remote, 0); }

static uint64_t SHIM_TC accept_session(void *self, uint64_t remote) {
    (void)self;
    shim_trace("AcceptP2PSessionWithUser %llu", (unsigned long long)remote);
    p2p_start();
    p2p_set_event(on_event);
    AcquireSRWLockExclusive(&g_lock);
    Peer *p = peer_locked(remote, true);
    if (p) p->accepted = true;
    ReleaseSRWLockExclusive(&g_lock);
    return p != NULL;
}

static uint64_t SHIM_TC close_session(void *self, uint64_t remote) {
    (void)self;
    shim_trace("CloseP2PSessionWithUser %llu", (unsigned long long)remote);
    bool known;
    AcquireSRWLockExclusive(&g_lock);
    Peer *p = peer_locked(remote, false);
    known = p != NULL;
    if (p) memset(p, 0, sizeof *p);
    ReleaseSRWLockExclusive(&g_lock);
    if (known) p2p_close(remote);
    return known;
}

static uint64_t SHIM_TC close_channel(void *self, uint64_t remote, int channel) {
    (void)self;
    P2PMsg *m;
    bool any = false;
    while ((m = p2p_take(remote, (uint32_t)channel, (uint32_t)channel))) { p2p_free(m); any = true; }
    return any || accepted(remote);
}

static uint64_t SHIM_TC session_state(void *self, uint64_t remote, uint8_t *out) {
    (void)self;
    if (!out) return 0;
    int st = p2p_state(remote);
    memset(out, 0, 20);
    out[0] = st == 2;
    out[1] = st == 1;
    return st != 0;
}

static uint64_t SHIM_TC allow_relay(void *self, uint8_t allow) { (void)self; (void)allow; return 1; }

void register_p2pnet(void) {
    p2p_set_event(on_event);
    const char *p = "SteamNetworking0";
    shim_register_family_ab(p, "SendP2PPacket", 20, (void *)send_p2p_v3);
    shim_register_family_ab(p, "SendP2PPacket", 24, (void *)send_p2p);
    shim_register_family_ab(p, "IsP2PPacketAvailable", 4, (void *)is_available_v3);
    shim_register_family_ab(p, "IsP2PPacketAvailable", 8, (void *)is_available);
    shim_register_family_ab(p, "ReadP2PPacket", 16, (void *)read_p2p_v3);
    shim_register_family_ab(p, "ReadP2PPacket", 20, (void *)read_p2p);
    shim_register_family(p, "AcceptP2PSessionWithUser", (void *)accept_session);
    shim_register_family(p, "CloseP2PSessionWithUser", (void *)close_session);
    shim_register_family(p, "CloseP2PChannelWithUser", (void *)close_channel);
    shim_register_family(p, "GetP2PSessionState", (void *)session_state);
    shim_register_family(p, "AllowP2PPacketRelay", (void *)allow_relay);
}
