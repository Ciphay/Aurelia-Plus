#include <stdlib.h>
#include <string.h>
#include <winsock2.h>
#include <ws2tcpip.h>

#include "shim.h"

#define MAX_FRAME (16u * 1024u * 1024u)
#define T_HELLO 1
#define T_HELLO_OK 2
#define T_SNAPSHOT_REQ 3
#define T_SNAPSHOT 4
#define T_CALL 5
#define T_REPLY 6

static SOCKET g_sock = INVALID_SOCKET;

static void put32(uint8_t *o, uint32_t v) { o[0] = (uint8_t)v; o[1] = (uint8_t)(v >> 8); o[2] = (uint8_t)(v >> 16); o[3] = (uint8_t)(v >> 24); }
static uint32_t get32(const uint8_t *b) { return (uint32_t)b[0] | ((uint32_t)b[1] << 8) | ((uint32_t)b[2] << 16) | ((uint32_t)b[3] << 24); }

size_t bridge_encode_hello(uint8_t *out, size_t cap, const uint8_t token[32]) {
    if (cap < 39) return 0;
    put32(out, 35);
    out[4] = T_HELLO;
    out[5] = 1; out[6] = 0;
    memcpy(out + 7, token, 32);
    return 39;
}

size_t bridge_encode_snapshot_req(uint8_t *out, size_t cap) {
    if (cap < 5) return 0;
    put32(out, 1);
    out[4] = T_SNAPSHOT_REQ;
    return 5;
}

size_t bridge_encode_call(uint8_t *out, size_t cap, uint32_t seq, uint16_t iface, uint16_t slot, const uint8_t *payload, size_t len) {
    size_t body = 1 + 4 + 2 + 2 + len;
    if (cap < 4 + body) return 0;
    put32(out, (uint32_t)body);
    out[4] = T_CALL;
    put32(out + 5, seq);
    out[9] = (uint8_t)iface; out[10] = (uint8_t)(iface >> 8);
    out[11] = (uint8_t)slot; out[12] = (uint8_t)(slot >> 8);
    if (len) memcpy(out + 13, payload, len);
    return 4 + body;
}

int bridge_decode(const uint8_t *buf, size_t len, BridgeFrame *f, size_t *used) {
    if (len < 4) return 0;
    uint32_t blen = get32(buf);
    if (blen > MAX_FRAME || blen == 0) return -1;
    if (len < 4 + (size_t)blen) return 0;
    f->tag = buf[4];
    f->body = buf + 5;
    f->body_len = blen - 1;
    *used = 4 + (size_t)blen;
    return 1;
}

void bridge_close(void) {
    if (g_sock != INVALID_SOCKET) { closesocket(g_sock); g_sock = INVALID_SOCKET; }
}

static bool send_all_on(SOCKET s, const uint8_t *b, size_t n) {
    while (n) {
        int w = send(s, (const char *)b, (int)n, 0);
        if (w <= 0) return false;
        b += w; n -= (size_t)w;
    }
    return true;
}

static bool send_all(const uint8_t *b, size_t n) { return send_all_on(g_sock, b, n); }

static bool read_frame_on(SOCKET s, uint8_t **buf, size_t *have, size_t *cap, BridgeFrame *f, size_t *used, int timeout_ms) {
    DWORD deadline = GetTickCount() + (DWORD)timeout_ms;
    for (;;) {
        int r = bridge_decode(*buf, *have, f, used);
        if (r < 0) return false;
        if (r == 1) return true;
        int left = (int)(deadline - GetTickCount());
        if (left <= 0) return false;
        fd_set rd; FD_ZERO(&rd); FD_SET(s, &rd);
        struct timeval tv = { left / 1000, (left % 1000) * 1000 };
        if (select(0, &rd, NULL, NULL, &tv) <= 0) return false;
        if (*have + 65536 > *cap) {
            *cap = (*have + 65536) * 2;
            uint8_t *g = realloc(*buf, *cap);
            if (!g) return false;
            *buf = g;
        }
        int n = recv(s, (char *)*buf + *have, (int)(*cap - *have), 0);
        if (n <= 0) return false;
        *have += (size_t)n;
    }
}

static bool read_frame(uint8_t **buf, size_t *have, size_t *cap, BridgeFrame *f, size_t *used, int timeout_ms) {
    return read_frame_on(g_sock, buf, have, cap, f, used, timeout_ms);
}

static char g_host[32];
static uint16_t g_port;
static uint8_t g_token[32];

static SOCKET open_conn(const char *host, uint16_t port, const uint8_t token[32], int timeout_ms) {
    static bool wsa;
    if (!wsa) { WSADATA d; if (WSAStartup(MAKEWORD(2, 2), &d)) return INVALID_SOCKET; wsa = true; }
    SOCKET s = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (s == INVALID_SOCKET) return INVALID_SOCKET;
    u_long nb = 1;
    ioctlsocket(s, FIONBIO, &nb);
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET;
    a.sin_port = htons(port);
    a.sin_addr.s_addr = inet_addr(host);
    connect(s, (struct sockaddr *)&a, sizeof a);
    fd_set wr, ex; FD_ZERO(&wr); FD_ZERO(&ex); FD_SET(s, &wr); FD_SET(s, &ex);
    struct timeval tv = { timeout_ms / 1000, (timeout_ms % 1000) * 1000 };
    if (select(0, NULL, &wr, &ex, &tv) <= 0 || FD_ISSET(s, &ex) || !FD_ISSET(s, &wr)) { closesocket(s); return INVALID_SOCKET; }
    int err = 0, el = sizeof err;
    getsockopt(s, SOL_SOCKET, SO_ERROR, (char *)&err, &el);
    if (err) { closesocket(s); return INVALID_SOCKET; }
    uint8_t hello[39];
    bridge_encode_hello(hello, sizeof hello, token);
    uint8_t *buf = malloc(4096);
    size_t have = 0, cap = 4096, used = 0;
    BridgeFrame f;
    bool ok = buf && send_all_on(s, hello, sizeof hello) && read_frame_on(s, &buf, &have, &cap, &f, &used, timeout_ms) && f.tag == T_HELLO_OK;
    free(buf);
    if (!ok) { closesocket(s); return INVALID_SOCKET; }
    return s;
}

bool bridge_connect(const char *host, uint16_t port, const uint8_t token[32], int timeout_ms) {
    bridge_close();
    SOCKET s = open_conn(host, port, token, timeout_ms);
    if (s == INVALID_SOCKET) return false;
    g_sock = s;
    strncpy(g_host, host, sizeof g_host - 1);
    g_port = port;
    memcpy(g_token, token, 32);
    return true;
}

bool bridge_fetch_snapshot(char **json, size_t *len, int timeout_ms) {
    if (g_sock == INVALID_SOCKET) return false;
    uint8_t req[5];
    bridge_encode_snapshot_req(req, sizeof req);
    uint8_t *buf = malloc(65536);
    size_t have = 0, cap = 65536, used = 0;
    BridgeFrame f;
    bool ok = buf && send_all(req, sizeof req);
    while (ok && (ok = read_frame(&buf, &have, &cap, &f, &used, timeout_ms))) {
        if (f.tag == T_SNAPSHOT) {
            *json = malloc(f.body_len + 1);
            if (!*json) { ok = false; break; }
            memcpy(*json, f.body, f.body_len);
            (*json)[f.body_len] = 0;
            *len = f.body_len;
            break;
        }
        memmove(buf, buf + used, have - used);
        have -= used;
    }
    free(buf);
    return ok;
}

static volatile LONG g_ipc_calls;
uint32_t shim_ipc_call_count(void) { return (uint32_t)g_ipc_calls; }

static bool call_on(SOCKET s, uint16_t iface, uint16_t slot, const uint8_t *payload, size_t len, int32_t *status,
                    uint8_t *reply, size_t reply_cap, size_t *reply_len, int timeout_ms) {
    static uint32_t next_seq = 1;
    uint32_t seq = (uint32_t)InterlockedIncrement((volatile LONG *)&next_seq);
    size_t cap = 13 + len + 4;
    uint8_t *req = malloc(cap);
    size_t n = req ? bridge_encode_call(req, cap, seq, iface, slot, payload, len) : 0;
    bool ok = n && send_all_on(s, req, n);
    free(req);
    uint8_t *buf = ok ? malloc(4096) : NULL;
    size_t have = 0, bcap = 4096, used = 0;
    BridgeFrame f;
    ok = ok && buf;
    while (ok && (ok = read_frame_on(s, &buf, &have, &bcap, &f, &used, timeout_ms))) {
        if (f.tag == T_REPLY && f.body_len >= 8 && get32(f.body) == seq) {
            if (status) *status = (int32_t)get32(f.body + 4);
            size_t n = f.body_len - 8;
            if (reply) {
                if (n > reply_cap) { ok = false; break; }
                memcpy(reply, f.body + 8, n);
            }
            if (reply_len) *reply_len = n;
            break;
        }
        memmove(buf, buf + used, have - used);
        have -= used;
    }
    free(buf);
    return ok;
}

bool shim_bind_local_udp(uintptr_t sock, uint16_t port, bool use_binding_port) {
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(shim_local_ip_binding());
    a.sin_port = htons(port ? port : (use_binding_port ? shim_local_port_binding() : 0));
    if (bind((SOCKET)sock, (struct sockaddr *)&a, sizeof a) == 0) return true;
    if (a.sin_port || a.sin_addr.s_addr) {
        a.sin_addr.s_addr = htonl(INADDR_ANY);
        a.sin_port = htons(port);
        return bind((SOCKET)sock, (struct sockaddr *)&a, sizeof a) == 0;
    }
    return false;
}

bool bridge_reachable(void) {
    static volatile LONG stamp, last;
    if (g_sock == INVALID_SOCKET) return false;
    DWORD now = GetTickCount() | 1;
    if (stamp && now - (DWORD)stamp < 2000) return last != 0;
    SOCKET s = open_conn(g_host, g_port, g_token, 700);
    bool ok = s != INVALID_SOCKET;
    if (ok) closesocket(s);
    last = ok;
    stamp = (LONG)now;
    return ok;
}

bool bridge_call(uint16_t iface, uint16_t slot, const uint8_t *payload, size_t len, int32_t *status,
                 uint8_t *reply, size_t reply_cap, size_t *reply_len, int timeout_ms) {
    static SRWLOCK lock = SRWLOCK_INIT;
    if (g_sock == INVALID_SOCKET) return false;
    InterlockedIncrement(&g_ipc_calls);
    AcquireSRWLockExclusive(&lock);
    bool ok = call_on(g_sock, iface, slot, payload, len, status, reply, reply_cap, reply_len, timeout_ms);
    ReleaseSRWLockExclusive(&lock);
    return ok;
}

bool bridge_call_fresh(uint16_t iface, uint16_t slot, const uint8_t *payload, size_t len, int32_t *status,
                       uint8_t *reply, size_t reply_cap, size_t *reply_len, int timeout_ms) {
    if (g_sock == INVALID_SOCKET) return false;
    SOCKET s = open_conn(g_host, g_port, g_token, 2000);
    if (s == INVALID_SOCKET) return false;
    bool ok = call_on(s, iface, slot, payload, len, status, reply, reply_cap, reply_len, timeout_ms);
    closesocket(s);
    return ok;
}
