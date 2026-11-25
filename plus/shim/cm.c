#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "shim.h"

#define SLOT_CM_CALL 7
#define SLOT_CM_SUBSCRIBE 8
#define SLOT_CM_POLL 9
#define REPLY_CAP (4u * 1024u * 1024u)

static void put32(uint8_t *o, uint32_t v) { memcpy(o, &v, 4); }

static uint8_t *encode_call(bool reply, bool protobuf, bool gs, int32_t emsg, int wait_ms, const char *job, const uint8_t *body, size_t len, size_t *n) {
    size_t nl = job ? strlen(job) : 0;
    uint8_t *p = malloc(14 + nl + len + 1);
    if (!p) return NULL;
    p[0] = (uint8_t)((reply ? 1 : 0) | (protobuf ? 2 : 0) | 4 | (gs ? 8 : 0));
    p[1] = p[2] = p[3] = 0;
    put32(p + 4, (uint32_t)emsg);
    put32(p + 8, (uint32_t)wait_ms);
    p[12] = (uint8_t)nl;
    p[13] = (uint8_t)(nl >> 8);
    if (nl) memcpy(p + 14, job, nl);
    if (len) memcpy(p + 14 + nl, body, len);
    *n = 14 + nl + len;
    return p;
}

void shim_cm_reply_free(CmReply *r) { free(r->body); memset(r, 0, sizeof *r); }

bool shim_cm_call(int32_t emsg, bool protobuf, const char *job, const uint8_t *body, size_t len, int wait_ms, CmReply *reply) {
    size_t n;
    uint8_t *req = encode_call(true, protobuf, false, emsg, wait_ms, job, body, len, &n);
    uint8_t *buf = malloc(REPLY_CAP);
    if (!req || !buf) { free(req); free(buf); return false; }
    int32_t status = -1;
    size_t got = 0;
    bool ok = bridge_call_fresh(0xFFFF, SLOT_CM_CALL, req, n, &status, buf, REPLY_CAP, &got, wait_ms + 3000);
    free(req);
    if (!ok || status != 0 || got < 8) { free(buf); return false; }
    if (reply) {
        reply->emsg = (int32_t)(buf[0] | buf[1] << 8 | buf[2] << 16 | (uint32_t)buf[3] << 24);
        reply->eresult = (int32_t)(buf[4] | buf[5] << 8 | buf[6] << 16 | (uint32_t)buf[7] << 24);
        reply->len = got - 8;
        reply->body = malloc(reply->len + 1);
        if (!reply->body) { free(buf); return false; }
        memcpy(reply->body, buf + 8, reply->len);
    }
    free(buf);
    return true;
}

bool shim_cm_service(const char *method, const uint8_t *body, size_t len, int wait_ms, CmReply *reply) {
    return shim_cm_call(CM_EMSG_SERVICE_CALL, true, method, body, len, wait_ms, reply);
}

bool shim_cm_send(int32_t emsg, bool protobuf, const uint8_t *body, size_t len) {
    size_t n;
    uint8_t *req = encode_call(false, protobuf, false, emsg, 0, NULL, body, len, &n);
    if (!req) return false;
    int32_t status = -1;
    bool ok = bridge_call_fresh(0xFFFF, SLOT_CM_CALL, req, n, &status, NULL, 0, NULL, 5000);
    free(req);
    return ok && status >= 0;
}

bool shim_cm_gs_send(int32_t emsg, const uint8_t *body, size_t len) {
    size_t n;
    uint8_t *req = encode_call(false, true, true, emsg, 0, NULL, body, len, &n);
    if (!req) return false;
    int32_t status = -1;
    bool ok = bridge_call_fresh(0xFFFF, SLOT_CM_CALL, req, n, &status, NULL, 0, NULL, 5000);
    free(req);
    return ok && status >= 0;
}

bool shim_cm_subscribe(int32_t emsg) {
    uint8_t p[9] = {0};
    int32_t status = -1;
    put32(p, (uint32_t)emsg);
    put32(p + 5, (uint32_t)GetCurrentProcessId());
    return bridge_call(0xFFFF, SLOT_CM_SUBSCRIBE, p, sizeof p, &status, NULL, 0, NULL, 3000) && status == 0;
}

static uint8_t *g_pending;
static size_t g_pending_len, g_pending_off;
static SRWLOCK g_poll_lock = SRWLOCK_INIT;

bool shim_cm_poll(int32_t *emsg, uint8_t **body, size_t *len) {
    bool got = false;
    AcquireSRWLockExclusive(&g_poll_lock);
    if (g_pending_off >= g_pending_len) {
        free(g_pending);
        g_pending = malloc(256 * 1024);
        g_pending_len = g_pending_off = 0;
        int32_t status = -1;
        size_t n = 0;
        uint8_t me[4];
        put32(me, (uint32_t)GetCurrentProcessId());
        if (g_pending && bridge_call(0xFFFF, SLOT_CM_POLL, me, sizeof me, &status, g_pending, 256 * 1024, &n, 2000) && status == 0) g_pending_len = n;
    }
    if (g_pending && g_pending_len - g_pending_off >= 8) {
        const uint8_t *p = g_pending + g_pending_off;
        uint32_t id, l;
        memcpy(&id, p, 4);
        memcpy(&l, p + 4, 4);
        if (l <= g_pending_len - g_pending_off - 8) {
            *body = malloc((size_t)l + 1);
            if (*body) {
                memcpy(*body, p + 8, l);
                *emsg = (int32_t)id;
                *len = l;
                g_pending_off += 8 + (size_t)l;
                got = true;
            }
        } else {
            g_pending_off = g_pending_len;
        }
    }
    ReleaseSRWLockExclusive(&g_poll_lock);
    return got;
}

typedef struct PostJob { int32_t emsg; PbW body; struct PostJob *next; } PostJob;
static PostJob *g_post_head, *g_post_tail;
static int g_post_busy;
static SRWLOCK g_post_lock = SRWLOCK_INIT;
static HANDLE g_post_wake;
static volatile LONG g_post_started;

static DWORD WINAPI post_thread(LPVOID unused) {
    (void)unused;
    for (;;) {
        WaitForSingleObject(g_post_wake, INFINITE);
        for (;;) {
            AcquireSRWLockExclusive(&g_post_lock);
            PostJob *j = g_post_head;
            if (j) { g_post_head = j->next; if (!g_post_head) g_post_tail = NULL; g_post_busy = 1; }
            ReleaseSRWLockExclusive(&g_post_lock);
            if (!j) break;
            shim_cm_send(j->emsg, true, j->body.p, j->body.len);
            pbw_free(&j->body);
            free(j);
            AcquireSRWLockExclusive(&g_post_lock);
            g_post_busy = 0;
            ReleaseSRWLockExclusive(&g_post_lock);
        }
    }
    return 0;
}

void shim_cm_post(int32_t emsg, PbW *body) {
    PostJob *j = calloc(1, sizeof *j);
    if (!j) { pbw_free(body); return; }
    j->emsg = emsg;
    j->body = *body;
    memset(body, 0, sizeof *body);
    if (!InterlockedCompareExchange(&g_post_started, 1, 0)) {
        g_post_wake = CreateEventA(NULL, FALSE, FALSE, NULL);
        HANDLE t = CreateThread(NULL, 0, post_thread, NULL, 0, NULL);
        if (t) CloseHandle(t);
    }
    AcquireSRWLockExclusive(&g_post_lock);
    if (g_post_tail) g_post_tail->next = j; else g_post_head = j;
    g_post_tail = j;
    ReleaseSRWLockExclusive(&g_post_lock);
    SetEvent(g_post_wake);
}

bool shim_cm_flush(int wait_ms) {
    DWORD deadline = GetTickCount() + (DWORD)wait_ms;
    for (;;) {
        AcquireSRWLockExclusive(&g_post_lock);
        bool idle = !g_post_head && !g_post_busy;
        ReleaseSRWLockExclusive(&g_post_lock);
        if (idle) return true;
        if ((int)(deadline - GetTickCount()) <= 0) return false;
        Sleep(5);
    }
}

typedef struct {
    uint64_t handle;
    int callback_id, wait_ms;
    int32_t emsg;
    bool service;
    char *job;
    PbW body;
    CmParse parse;
    void *ctx;
} AsyncJob;

static DWORD WINAPI async_thread(LPVOID arg) {
    AsyncJob *j = arg;
    CmReply r = {0};
    bool ok = j->service ? shim_cm_service(j->job, j->body.p, j->body.len, j->wait_ms, &r)
                         : shim_cm_call(j->emsg, true, NULL, j->body.p, j->body.len, j->wait_ms, &r);
    int size = 16384;
    uint8_t *out = calloc(1, (size_t)size);
    if (out && ok && j->parse && j->parse(&r, j->ctx, out, &size)) shim_callresult_complete(j->handle, j->callback_id, out, size);
    else shim_callresult_fail_handle(j->handle);
    shim_flush_after_result();
    free(out);
    shim_cm_reply_free(&r);
    pbw_free(&j->body);
    free(j->job);
    free(j->ctx);
    free(j);
    return 0;
}

void shim_cm_async(uint64_t handle, int callback_id, int32_t emsg, bool service, const char *job, PbW *body, int wait_ms, CmParse parse, void *ctx) {
    AsyncJob *j = calloc(1, sizeof *j);
    if (!j) { pbw_free(body); free(ctx); shim_callresult_fail_handle(handle); return; }
    j->handle = handle; j->callback_id = callback_id; j->emsg = emsg; j->service = service;
    j->job = job ? _strdup(job) : NULL;
    j->body = *body;
    memset(body, 0, sizeof *body);
    j->wait_ms = wait_ms; j->parse = parse; j->ctx = ctx;
    HANDLE t = CreateThread(NULL, 0, async_thread, j, 0, NULL);
    if (!t) { async_thread(j); return; }
    CloseHandle(t);
}

typedef struct { int32_t emsg; ShimPushFn fn; } PushHandler;
static PushHandler g_handlers[32];
static int g_n_handlers;
static volatile LONG g_pump_started;
static SRWLOCK g_handlers_lock = SRWLOCK_INIT;

static DWORD WINAPI pump_thread(LPVOID unused) {
    (void)unused;
    for (;;) {
        int32_t emsg;
        uint8_t *body;
        size_t len;
        bool any = false;
        while (shim_cm_poll(&emsg, &body, &len)) {
            any = true;
            ShimPushFn fns[8];
            int n = 0;
            AcquireSRWLockShared(&g_handlers_lock);
            for (int i = 0; i < g_n_handlers && n < 8; i++) if (g_handlers[i].emsg == emsg) fns[n++] = g_handlers[i].fn;
            ReleaseSRWLockShared(&g_handlers_lock);
            for (int i = 0; i < n; i++) fns[i](body, len);
            free(body);
        }
        Sleep(any ? 10 : 100);
    }
    return 0;
}

bool shim_cm_on_push(int32_t emsg, ShimPushFn fn) {
    bool first = true;
    AcquireSRWLockExclusive(&g_handlers_lock);
    for (int i = 0; i < g_n_handlers; i++) if (g_handlers[i].emsg == emsg) first = false;
    bool room = g_n_handlers < 32;
    if (room) g_handlers[g_n_handlers++] = (PushHandler){ emsg, fn };
    ReleaseSRWLockExclusive(&g_handlers_lock);
    if (!room) return false;
    if (first) shim_cm_subscribe(emsg);
    if (!InterlockedCompareExchange(&g_pump_started, 1, 0)) {
        HANDLE t = CreateThread(NULL, 0, pump_thread, NULL, 0, NULL);
        if (t) CloseHandle(t);
    }
    return true;
}
