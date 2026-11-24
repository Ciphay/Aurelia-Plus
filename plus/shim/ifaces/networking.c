#include <stdlib.h>
#include <string.h>

#include "../shim.h"

#define MAX_SOCKETS 32
#define MAX_LISTEN 8
#define MAX_QUEUE 64

typedef struct Msg { uint8_t *data; uint32_t len; } Msg;
typedef struct {
    bool used;
    int peer;
    int listen;
    Msg q[MAX_QUEUE];
    int head, count;
} Sock;
typedef struct { bool used; uint32_t port; int pending[MAX_SOCKETS]; int n_pending; } Listen;

static Sock g_socks[MAX_SOCKETS];
static Listen g_listen[MAX_LISTEN];

static Sock *sock_of(uint32_t h) { return h >= 1 && h <= MAX_SOCKETS && g_socks[h - 1].used ? &g_socks[h - 1] : NULL; }
static Listen *listen_of(uint32_t h) { return h >= 1 && h <= MAX_LISTEN && g_listen[h - 1].used ? &g_listen[h - 1] : NULL; }

static int new_sock(int listen) {
    for (int i = 0; i < MAX_SOCKETS; i++) {
        if (g_socks[i].used) continue;
        memset(&g_socks[i], 0, sizeof g_socks[i]);
        g_socks[i].used = true; g_socks[i].peer = -1; g_socks[i].listen = listen;
        return i;
    }
    return -1;
}
static void drain_sock(Sock *s) {
    while (s->count) { free(s->q[s->head].data); s->head = (s->head + 1) % MAX_QUEUE; s->count--; }
}

static uint32_t SHIM_TC create_listen(void *self, int virtual_port, uint32_t ip, uint32_t port, uint8_t relay) {
    (void)self; (void)virtual_port; (void)ip; (void)relay;
    for (int i = 0; i < MAX_LISTEN; i++) {
        if (g_listen[i].used) continue;
        memset(&g_listen[i], 0, sizeof g_listen[i]);
        g_listen[i].used = true; g_listen[i].port = port & 0xffff;
        return (uint32_t)(i + 1);
    }
    return 0;
}

static uint32_t SHIM_TC create_connection(void *self, uint32_t ip, uint32_t port, int timeout) {
    (void)self; (void)timeout;
    if (ip != 0 && ip != 0x7f000001u && ip != 0x0100007fu) return 0;
    for (int i = 0; i < MAX_LISTEN; i++) {
        Listen *l = &g_listen[i];
        if (!l->used || l->port != (port & 0xffff) || l->n_pending >= MAX_SOCKETS) continue;
        int c = new_sock(-1), s = new_sock(i);
        if (c < 0 || s < 0) { if (c >= 0) g_socks[c].used = false; if (s >= 0) g_socks[s].used = false; return 0; }
        g_socks[c].peer = s; g_socks[s].peer = c;
        l->pending[l->n_pending++] = s;
        return (uint32_t)(c + 1);
    }
    return 0;
}

static uint32_t SHIM_TC create_p2p(void *self, uint64_t steamid, int virtual_port, int timeout, uint8_t relay) {
    (void)self; (void)steamid; (void)virtual_port; (void)timeout; (void)relay;
    return 0;
}

static uint64_t SHIM_TC destroy_socket(void *self, uint32_t h, uint8_t notify) {
    (void)self; (void)notify;
    Sock *s = sock_of(h);
    if (!s) return 0;
    if (s->peer >= 0) g_socks[s->peer].peer = -1;
    drain_sock(s);
    s->used = false;
    return 1;
}
static uint64_t SHIM_TC destroy_listen(void *self, uint32_t h, uint8_t notify) {
    (void)self; (void)notify;
    Listen *l = listen_of(h);
    if (!l) return 0;
    l->used = false;
    return 1;
}

static uint64_t SHIM_TC send_on_socket(void *self, uint32_t h, const void *data, uint32_t cb, uint8_t reliable) {
    (void)self; (void)reliable;
    Sock *s = sock_of(h);
    if (!s || s->peer < 0 || !data) return 0;
    Sock *p = &g_socks[s->peer];
    if (p->count == MAX_QUEUE) return 0;
    uint8_t *copy = malloc(cb ? cb : 1);
    if (!copy) return 0;
    memcpy(copy, data, cb);
    Msg *m = &p->q[(p->head + p->count++) % MAX_QUEUE];
    m->data = copy; m->len = cb;
    return 1;
}
static uint64_t SHIM_TC is_data_available_on_socket(void *self, uint32_t h, uint32_t *size) {
    (void)self;
    Sock *s = sock_of(h);
    if (!s || !s->count) return 0;
    if (size) *size = s->q[s->head].len;
    return 1;
}
static uint64_t SHIM_TC retrieve_from_socket(void *self, uint32_t h, void *dest, uint32_t cap, uint32_t *size) {
    (void)self;
    Sock *s = sock_of(h);
    if (!s || !s->count) return 0;
    Msg m = s->q[s->head];
    if (m.len > cap) return 0;
    memcpy(dest, m.data, m.len);
    if (size) *size = m.len;
    free(m.data);
    s->head = (s->head + 1) % MAX_QUEUE; s->count--;
    return 1;
}

static int ready_socket(Listen *l, int idx) {
    for (int i = 0; i < MAX_SOCKETS; i++)
        if (g_socks[i].used && g_socks[i].listen == idx && g_socks[i].count) return i;
    (void)l;
    return -1;
}
static uint64_t SHIM_TC is_data_available(void *self, uint32_t h, uint32_t *size, uint32_t *sock) {
    (void)self;
    Listen *l = listen_of(h);
    if (!l) return 0;
    int i = ready_socket(l, (int)h - 1);
    if (i < 0) return 0;
    if (size) *size = g_socks[i].q[g_socks[i].head].len;
    if (sock) *sock = (uint32_t)(i + 1);
    return 1;
}
static uint64_t SHIM_TC retrieve_data(void *self, uint32_t h, void **dest, uint32_t *size, uint32_t *sock) {
    (void)self;
    Listen *l = listen_of(h);
    if (!l || !dest) return 0;
    int i = ready_socket(l, (int)h - 1);
    if (i < 0) return 0;
    Sock *s = &g_socks[i];
    Msg m = s->q[s->head];
    *dest = m.data;
    if (size) *size = m.len;
    if (sock) *sock = (uint32_t)(i + 1);
    s->head = (s->head + 1) % MAX_QUEUE; s->count--;
    return 1;
}

static uint64_t SHIM_TC get_socket_info(void *self, uint32_t h, uint64_t *remote, int *status, uint32_t *ip, uint16_t *port) {
    (void)self;
    Sock *s = sock_of(h);
    if (!s) return 0;
    if (remote) *remote = 0;
    if (status) *status = s->peer >= 0 ? 1 : 3;
    if (ip) *ip = 0x7f000001u;
    if (port) *port = 0;
    return 1;
}
static uint64_t SHIM_TC get_listen_socket_info(void *self, uint32_t h, uint32_t *ip, uint16_t *port) {
    (void)self;
    Listen *l = listen_of(h);
    if (!l) return 0;
    if (ip) *ip = 0x7f000001u;
    if (port) *port = (uint16_t)l->port;
    return 1;
}
static int SHIM_TC get_socket_connection_type(void *self, uint32_t h) { (void)self; return sock_of(h) ? 1 : 0; }
static int SHIM_TC get_max_packet_size(void *self, uint32_t h) { (void)self; (void)h; return 1200; }

void register_networking(void) {
    const char *p = "SteamNetworking0";
    shim_register_family(p, "CreateListenSocket", (void *)create_listen);
    shim_register_family(p, "CreateConnectionSocket", (void *)create_connection);
    shim_register_family(p, "CreateP2PConnectionSocket", (void *)create_p2p);
    shim_register_family(p, "DestroySocket", (void *)destroy_socket);
    shim_register_family(p, "DestroyListenSocket", (void *)destroy_listen);
    shim_register_family(p, "SendDataOnSocket", (void *)send_on_socket);
    shim_register_family(p, "IsDataAvailableOnSocket", (void *)is_data_available_on_socket);
    shim_register_family(p, "RetrieveDataFromSocket", (void *)retrieve_from_socket);
    shim_register_family(p, "IsDataAvailable", (void *)is_data_available);
    shim_register_family(p, "RetrieveData", (void *)retrieve_data);
    shim_register_family(p, "GetSocketInfo", (void *)get_socket_info);
    shim_register_family(p, "GetListenSocketInfo", (void *)get_listen_socket_info);
    shim_register_family(p, "GetSocketConnectionType", (void *)get_socket_connection_type);
    shim_register_family(p, "GetMaxPacketSize", (void *)get_max_packet_size);
}
