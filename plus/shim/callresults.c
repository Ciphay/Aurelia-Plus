#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "shim.h"

#define SLOTS 64

typedef struct { uint64_t handle; int callback; int size; uint8_t *data; bool failed; } Result;

static Result g_results[SLOTS];
static uint64_t g_next = 0x1000;
static SRWLOCK g_lock = SRWLOCK_INIT;

uint64_t shim_callresult_reserve(void) {
    AcquireSRWLockExclusive(&g_lock);
    uint64_t handle = ++g_next;
    Result *slot = &g_results[handle % SLOTS];
    free(slot->data);
    memset(slot, 0, sizeof *slot);
    slot->handle = handle;
    ReleaseSRWLockExclusive(&g_lock);
    return handle;
}

void shim_callresult_complete(uint64_t handle, int callback_id, const void *data, int size) {
    if (size < 0) size = 0;
    uint8_t *copy = malloc((size_t)size + 1);
    if (!copy) return;
    if (size) memcpy(copy, data, (size_t)size);
    AcquireSRWLockExclusive(&g_lock);
    Result *slot = &g_results[handle % SLOTS];
    if (slot->handle != handle) { ReleaseSRWLockExclusive(&g_lock); free(copy); return; }
    free(slot->data);
    slot->callback = callback_id;
    slot->size = size;
    slot->data = copy;
    ReleaseSRWLockExclusive(&g_lock);
    uint8_t note[16] = {0};
    int32_t cb = callback_id;
    uint32_t sz = (uint32_t)size;
    memcpy(note, &handle, 8);
    memcpy(note + 8, &cb, 4);
    memcpy(note + 12, &sz, 4);
    shim_post_callback(703, note, sizeof note);
}

uint64_t shim_callresult_fail(void) {
    uint64_t handle = shim_callresult_reserve();
    uint8_t *none = malloc(1);
    if (!none) return handle;
    AcquireSRWLockExclusive(&g_lock);
    Result *slot = &g_results[handle % SLOTS];
    if (slot->handle == handle) { slot->data = none; slot->size = 0; slot->callback = 0; slot->failed = true; none = NULL; }
    ReleaseSRWLockExclusive(&g_lock);
    free(none);
    uint8_t note[16] = {0};
    memcpy(note, &handle, 8);
    shim_post_callback(703, note, sizeof note);
    return handle;
}

void shim_callresult_fail_handle(uint64_t handle) {
    uint8_t *none = malloc(1);
    if (!none) return;
    AcquireSRWLockExclusive(&g_lock);
    Result *slot = &g_results[handle % SLOTS];
    bool mine = slot->handle == handle;
    if (mine) { free(slot->data); slot->data = none; slot->size = 0; slot->callback = 0; slot->failed = true; none = NULL; }
    ReleaseSRWLockExclusive(&g_lock);
    free(none);
    if (!mine) return;
    uint8_t note[16] = {0};
    memcpy(note, &handle, 8);
    shim_post_callback(703, note, sizeof note);
}

uint64_t shim_callresult_post(int callback_id, const void *data, int size) {
    uint64_t handle = shim_callresult_reserve();
    shim_callresult_complete(handle, callback_id, data, size);
    return handle;
}

static Result *find_locked(uint64_t handle) {
    Result *r = &g_results[handle % SLOTS];
    return r->data && r->handle == handle ? r : NULL;
}

bool shim_callresult_completed(uint64_t handle, bool *failed) {
    AcquireSRWLockExclusive(&g_lock);
    Result *r = find_locked(handle);
    bool done = r != NULL;
    if (failed) *failed = r && r->failed;
    ReleaseSRWLockExclusive(&g_lock);
    return done;
}

bool shim_callresult_take(uint64_t handle, void *buf, int cap, int expected_callback, bool *failed) {
    bool ok = false;
    if (failed) *failed = false;
    AcquireSRWLockExclusive(&g_lock);
    Result *r = find_locked(handle);
    if (r && r->failed) {
        if (failed) *failed = true;
        free(r->data);
        memset(r, 0, sizeof *r);
        ok = true;
    } else if (r && r->callback == expected_callback && (cap >= r->size || expected_callback == 3401)) {
        if (r->size && buf) memcpy(buf, r->data, (size_t)(cap < r->size ? cap : r->size));
        free(r->data);
        memset(r, 0, sizeof *r);
        ok = true;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}
