#include <string.h>
#include <windows.h>

#include "shim.h"

#define QUEUE_CAP 128
#define MAX_PARAM 4096

typedef struct { int id; int size; uint8_t data[MAX_PARAM]; } Entry;

static Entry g_queue[QUEUE_CAP];
static int g_head, g_count;
static Entry g_current;
static SRWLOCK g_lock = SRWLOCK_INIT;

void shim_post_callback(int id, const void *data, int size) {
    if (size < 0) size = 0;
    if (size > MAX_PARAM) size = MAX_PARAM;
    AcquireSRWLockExclusive(&g_lock);
    if (g_count == QUEUE_CAP) { g_head = (g_head + 1) % QUEUE_CAP; g_count--; }
    Entry *e = &g_queue[(g_head + g_count) % QUEUE_CAP];
    e->id = id;
    e->size = size;
    if (size) memcpy(e->data, data, (size_t)size);
    g_count++;
    ReleaseSRWLockExclusive(&g_lock);
}

#define MAX_DEFERRED 8192
typedef struct { ShimDeferred fn; void *a, *b; int c; } Deferred;
static Deferred g_deferred[MAX_DEFERRED];
static int g_def_head, g_def_count;

void shim_defer(ShimDeferred fn, void *a, void *b, int c) {
    AcquireSRWLockExclusive(&g_lock);
    if (g_def_count < MAX_DEFERRED) {
        g_deferred[(g_def_head + g_def_count) % MAX_DEFERRED] = (Deferred){ fn, a, b, c };
        g_def_count++;
    }
    ReleaseSRWLockExclusive(&g_lock);
}

static void run_deferred(void) {
    for (;;) {
        Deferred d;
        AcquireSRWLockExclusive(&g_lock);
        if (!g_def_count) { ReleaseSRWLockExclusive(&g_lock); return; }
        d = g_deferred[g_def_head];
        g_def_head = (g_def_head + 1) % MAX_DEFERRED;
        g_def_count--;
        ReleaseSRWLockExclusive(&g_lock);
        d.fn(d.a, d.b, d.c);
    }
}

bool shim_next_callback(ShimCallbackMsg *out) {
    shim_poll_connection();
    run_deferred();
    bool got = false;
    AcquireSRWLockExclusive(&g_lock);
    if (g_count) {
        g_current = g_queue[g_head];
        g_head = (g_head + 1) % QUEUE_CAP;
        g_count--;
        out->user = 1;
        out->id = g_current.id;
        out->param = g_current.data;
        out->size = g_current.size;
        got = true;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return got;
}

void shim_free_last_callback(void) {}
