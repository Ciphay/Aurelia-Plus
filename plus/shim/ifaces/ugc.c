#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <windows.h>

#include "../shim.h"

enum {
    SLOT_UGC = 10, OP_LIST = 1, OP_STATE = 2, OP_DOWNLOAD = 3, OP_SUBSCRIBE = 4, OP_UNSUBSCRIBE = 5,
    CB_SUBSCRIBED = 1313, CB_UNSUBSCRIBED = 1315, CB_ITEM_INSTALLED = 3405, CB_DOWNLOAD_RESULT = 3406,
    STATE_SUBSCRIBED = 1, STATE_INSTALLED = 4, STATE_DOWNLOADING = 16,
    LIST_TTL_MS = 60000, MAX_IDS = 4096, MAX_DOWNLOADS = 16,
};

static uint64_t g_ids[MAX_IDS];
static uint32_t g_n_ids;
static DWORD g_list_at;
static bool g_list_valid;
static uint64_t g_downloading[MAX_DOWNLOADS];
static SRWLOCK g_lock = SRWLOCK_INIT;

static uint32_t app_id(void) { const Snapshot *s = shim_snapshot(); return s && s->has_ticket ? s->ticket.appid : 0; }

static bool ugc_call(uint8_t op, uint64_t id, int timeout_ms, uint8_t *reply, size_t cap, size_t *n) {
    uint8_t req[13];
    uint32_t app = app_id();
    int32_t status = -1;
    req[0] = op;
    memcpy(req + 1, &app, 4);
    memcpy(req + 5, &id, 8);
    return bridge_call_fresh(0xFFFF, SLOT_UGC, req, sizeof req, &status, reply, cap, n, timeout_ms) && status == 0;
}

static void refresh_list(bool force) {
    AcquireSRWLockExclusive(&g_lock);
    bool fresh = g_list_valid && !force && GetTickCount() - g_list_at < LIST_TTL_MS;
    ReleaseSRWLockExclusive(&g_lock);
    if (fresh) return;
    size_t cap = 4 + 8 * (size_t)MAX_IDS, n = 0;
    uint8_t *buf = malloc(cap);
    if (!buf) return;
    if (ugc_call(OP_LIST, 0, 15000, buf, cap, &n) && n >= 4) {
        uint32_t count;
        memcpy(&count, buf, 4);
        if (count > MAX_IDS) count = MAX_IDS;
        if (n >= 4 + 8 * (size_t)count) {
            AcquireSRWLockExclusive(&g_lock);
            memcpy(g_ids, buf + 4, 8 * (size_t)count);
            g_n_ids = count;
            g_list_valid = true;
            g_list_at = GetTickCount();
            ReleaseSRWLockExclusive(&g_lock);
        }
    }
    free(buf);
}

static bool is_subscribed(uint64_t id) {
    bool yes = false;
    AcquireSRWLockExclusive(&g_lock);
    for (uint32_t i = 0; i < g_n_ids; i++) yes = yes || g_ids[i] == id;
    ReleaseSRWLockExclusive(&g_lock);
    return yes;
}

static bool is_downloading(uint64_t id) {
    bool yes = false;
    if (!id) return false;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < MAX_DOWNLOADS; i++) yes = yes || g_downloading[i] == id;
    ReleaseSRWLockExclusive(&g_lock);
    return yes;
}

static bool set_downloading(uint64_t id, bool on) {
    bool ok = !on;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < MAX_DOWNLOADS; i++) {
        if (on && g_downloading[i] == id) { ok = false; break; }
        if (!on && g_downloading[i] == id) { g_downloading[i] = 0; break; }
    }
    if (on) for (int i = 0; i < MAX_DOWNLOADS; i++) if (!g_downloading[i]) { g_downloading[i] = id; ok = true; break; }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

typedef struct { bool installed; uint64_t size; uint32_t time; char folder[1024]; } Installed;

static bool read_installed(uint64_t id, Installed *out) {
    uint8_t buf[2048];
    size_t n = 0;
    memset(out, 0, sizeof *out);
    if (!ugc_call(OP_STATE, id, 10000, buf, sizeof buf, &n) || n < 15) return false;
    uint16_t len;
    memcpy(&len, buf + 13, 2);
    if (n < 15u + len) return false;
    out->installed = buf[0] != 0;
    memcpy(&out->size, buf + 1, 8);
    memcpy(&out->time, buf + 9, 4);
    if (out->installed) {
        char path[1024];
        size_t m = len < sizeof path - 1 ? len : sizeof path - 1;
        memcpy(path, buf + 15, m);
        path[m] = 0;
        if (!shim_win_path(path, out->folder, sizeof out->folder)) snprintf(out->folder, sizeof out->folder, "%s", path);
    }
    return true;
}

static uint32_t SHIM_TC num_subscribed(void *self) {
    (void)self;
    refresh_list(false);
    AcquireSRWLockExclusive(&g_lock);
    uint32_t n = g_n_ids;
    ReleaseSRWLockExclusive(&g_lock);
    return n;
}

static uint32_t SHIM_TC subscribed_items(void *self, uint64_t *out, uint32_t max) {
    (void)self;
    refresh_list(false);
    AcquireSRWLockExclusive(&g_lock);
    uint32_t n = g_n_ids < max ? g_n_ids : max;
    if (out && n) memcpy(out, g_ids, 8 * (size_t)n);
    ReleaseSRWLockExclusive(&g_lock);
    return n;
}

static uint32_t SHIM_TC num_subscribed_021(void *self, uint8_t include_disabled) { (void)include_disabled; return num_subscribed(self); }
static uint32_t SHIM_TC subscribed_items_021(void *self, uint64_t *out, uint32_t max, uint8_t include_disabled) { (void)include_disabled; return subscribed_items(self, out, max); }

static uint32_t SHIM_TC item_state(void *self, uint64_t id) {
    (void)self;
    if (!id) return 0;
    refresh_list(false);
    Installed in;
    uint32_t state = 0;
    if (is_subscribed(id)) state |= STATE_SUBSCRIBED;
    if (read_installed(id, &in) && in.installed) state |= STATE_INSTALLED;
    if (is_downloading(id)) state |= STATE_DOWNLOADING;
    return state;
}

static uint64_t install_info(uint64_t id, uint64_t *size, char *folder, uint32_t cap, uint32_t *when) {
    Installed in;
    if (!read_installed(id, &in) || !in.installed || !folder || !cap || strlen(in.folder) + 1 > cap) return 0;
    if (size) *size = in.size;
    memcpy(folder, in.folder, strlen(in.folder) + 1);
    if (when) *when = in.time;
    return 1;
}

static uint64_t SHIM_TC item_install_info(void *self, uint64_t id, uint64_t *size, char *folder, uint32_t cap, uint32_t *when) {
    (void)self;
    return install_info(id, size, folder, cap, when);
}

static uint64_t SHIM_TC item_install_info_002(void *self, uint64_t id, uint64_t *size, char *folder, uint32_t cap) {
    (void)self;
    return install_info(id, size, folder, cap, NULL);
}

static uint64_t SHIM_TC item_download_info(void *self, uint64_t id, uint64_t *done, uint64_t *total) {
    (void)self;
    if (!is_downloading(id)) return 0;
    if (done) *done = 0;
    if (total) *total = 0;
    return 1;
}

typedef struct { uint64_t id; uint64_t call; int op; } UgcJob;

static DWORD WINAPI download_thread(LPVOID arg) {
    UgcJob *j = arg;
    int32_t result = 2;
    uint8_t buf[8];
    size_t n = 0;
    if (ugc_call(OP_DOWNLOAD, j->id, 600000, buf, sizeof buf, &n) && n >= 4) memcpy(&result, buf, 4);
    set_downloading(j->id, false);
    uint8_t cb[24] = {0};
    uint32_t app = app_id();
    memcpy(cb, &app, 4);
    memcpy(cb + 8, &j->id, 8);
    memcpy(cb + 16, &result, 4);
    if (result == 1) {
        uint8_t inst[16] = {0};
        memcpy(inst, &app, 4);
        memcpy(inst + 8, &j->id, 8);
        shim_post_callback(CB_ITEM_INSTALLED, inst, sizeof inst);
    }
    shim_post_callback(CB_DOWNLOAD_RESULT, cb, sizeof cb);
    free(j);
    return 0;
}

static uint64_t SHIM_TC download_item(void *self, uint64_t id, uint8_t high_priority) {
    (void)self;
    (void)high_priority;
    if (!id || !set_downloading(id, true)) return 0;
    UgcJob *j = calloc(1, sizeof *j);
    if (!j) { set_downloading(id, false); return 0; }
    j->id = id;
    HANDLE t = CreateThread(NULL, 0, download_thread, j, 0, NULL);
    if (!t) { download_thread(j); return 1; }
    CloseHandle(t);
    return 1;
}

static DWORD WINAPI subscribe_thread(LPVOID arg) {
    UgcJob *j = arg;
    int32_t result = 2;
    uint8_t buf[8];
    size_t n = 0;
    if (ugc_call((uint8_t)j->op, j->id, 20000, buf, sizeof buf, &n) && n >= 4) memcpy(&result, buf, 4);
    if (result == 1) refresh_list(true);
    uint8_t cb[16] = {0};
    memcpy(cb, &result, 4);
    memcpy(cb + 8, &j->id, 8);
    shim_callresult_complete(j->call, j->op == OP_SUBSCRIBE ? CB_SUBSCRIBED : CB_UNSUBSCRIBED, cb, sizeof cb);
    free(j);
    return 0;
}

static uint64_t start_subscription(uint64_t id, int op) {
    uint64_t call = shim_callresult_reserve();
    UgcJob *j = calloc(1, sizeof *j);
    if (!j) { shim_callresult_fail_handle(call); return call; }
    j->id = id;
    j->call = call;
    j->op = op;
    HANDLE t = CreateThread(NULL, 0, subscribe_thread, j, 0, NULL);
    if (!t) { subscribe_thread(j); return call; }
    CloseHandle(t);
    return call;
}

static uint64_t SHIM_TC subscribe_item(void *self, uint64_t id) { (void)self; return start_subscription(id, OP_SUBSCRIBE); }
static uint64_t SHIM_TC unsubscribe_item(void *self, uint64_t id) { (void)self; return start_subscription(id, OP_UNSUBSCRIBE); }

enum { CB_ENUM_SUBSCRIBED = 1314, ENUM_PAGE = 50 };
typedef struct { uint64_t call; uint32_t start; } EnumJob;

static DWORD WINAPI enum_subscribed_thread(LPVOID arg) {
    EnumJob *j = arg;
    refresh_list(true);
    uint8_t cb[16 + 8 * ENUM_PAGE + 4 * ENUM_PAGE] = {0};
    AcquireSRWLockExclusive(&g_lock);
    uint32_t total = g_n_ids, n = 0;
    for (uint32_t i = j->start; i < total && n < ENUM_PAGE; i++, n++) memcpy(cb + 16 + 8 * n, &g_ids[i], 8);
    ReleaseSRWLockExclusive(&g_lock);
    int32_t result = 1, ret = (int32_t)n, tot = (int32_t)total;
    memcpy(cb, &result, 4);
    memcpy(cb + 4, &ret, 4);
    memcpy(cb + 8, &tot, 4);
    shim_callresult_complete(j->call, CB_ENUM_SUBSCRIBED, cb, sizeof cb);
    free(j);
    return 0;
}

static uint64_t SHIM_TC enumerate_subscribed_files(void *self, uint32_t start) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    EnumJob *j = calloc(1, sizeof *j);
    if (!j) { shim_callresult_fail_handle(call); return call; }
    j->call = call; j->start = start;
    HANDLE t = CreateThread(NULL, 0, enum_subscribed_thread, j, 0, NULL);
    if (!t) { enum_subscribed_thread(j); return call; }
    CloseHandle(t);
    return call;
}

void register_ugc(void) {
    shim_register_family("STEAMREMOTESTORAGE_INTERFACE_VERSION", "EnumerateUserSubscribedFiles", (void *)enumerate_subscribed_files);
    shim_register_family("STEAMREMOTESTORAGE_INTERFACE_VERSION", "SubscribePublishedFile", (void *)subscribe_item);
    shim_register_family("STEAMREMOTESTORAGE_INTERFACE_VERSION", "UnsubscribePublishedFile", (void *)unsubscribe_item);
    const char *p = "STEAMUGC_INTERFACE_VERSION";
    shim_register_family_ab(p, "GetNumSubscribedItems", 0, (void *)num_subscribed);
    shim_register_family_ab(p, "GetNumSubscribedItems", 4, (void *)num_subscribed_021);
    shim_register_family_ab(p, "GetSubscribedItems", 8, (void *)subscribed_items);
    shim_register_family_ab(p, "GetSubscribedItems", 12, (void *)subscribed_items_021);
    shim_register_family(p, "GetItemState", (void *)item_state);
    shim_register_family_ab(p, "GetItemInstallInfo", 24, (void *)item_install_info);
    shim_register_family_ab(p, "GetItemInstallInfo", 20, (void *)item_install_info_002);
    shim_register_family(p, "GetItemDownloadInfo", (void *)item_download_info);
    shim_register_family(p, "DownloadItem", (void *)download_item);
    shim_register_family(p, "SubscribeItem", (void *)subscribe_item);
    shim_register_family(p, "UnsubscribeItem", (void *)unsubscribe_item);
}
