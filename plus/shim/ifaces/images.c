
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "../shim.h"

enum { SLOT_IMAGE = 20, CB_AVATAR_LOADED = 334, CB_ICON_FETCHED = 1109, MAX_PIXELS = 256 * 256 * 4 };

typedef struct { uint32_t w, h; uint8_t *rgba; } Img;
static Img *g_imgs;
static int g_n_imgs, g_cap_imgs;

typedef struct { char key[176]; int state, handle; } Req;
static Req g_reqs[1024];
static int g_n_reqs;
static SRWLOCK g_lock = SRWLOCK_INIT;

static int add_image(uint32_t w, uint32_t h, const uint8_t *px) {
    uint8_t *copy = malloc((size_t)w * h * 4);
    if (!copy) return 0;
    memcpy(copy, px, (size_t)w * h * 4);
    if (g_n_imgs == g_cap_imgs) {
        int cap = g_cap_imgs ? g_cap_imgs * 2 : 32;
        Img *grown = realloc(g_imgs, (size_t)cap * sizeof *grown);
        if (!grown) { free(copy); return 0; }
        g_imgs = grown;
        g_cap_imgs = cap;
    }
    g_imgs[g_n_imgs] = (Img){ w, h, copy };
    return ++g_n_imgs;
}

static Req *find_req(const char *key) {
    for (int i = 0; i < g_n_reqs; i++) if (!strcmp(g_reqs[i].key, key)) return &g_reqs[i];
    return NULL;
}

typedef struct { char key[176]; uint8_t payload[160]; size_t len; int kind; uint64_t steamid; char name[128]; uint8_t achieved; } Job;

static DWORD WINAPI fetch_thread(LPVOID arg) {
    Job *j = arg;
    uint8_t *reply = malloc(8 + MAX_PIXELS);
    int32_t status = -1;
    size_t n = 0;
    int handle = 0;
    uint32_t w = 0, h = 0;
    if (reply && bridge_call_fresh(0xFFFF, SLOT_IMAGE, j->payload, j->len, &status, reply, 8 + MAX_PIXELS, &n, 20000) && status >= 0 && n >= 8) {
        memcpy(&w, reply, 4);
        memcpy(&h, reply + 4, 4);
        if (w && h && (size_t)w * h * 4 == n - 8) {
            AcquireSRWLockExclusive(&g_lock);
            handle = add_image(w, h, reply + 8);
            ReleaseSRWLockExclusive(&g_lock);
        }
    }
    free(reply);
    AcquireSRWLockExclusive(&g_lock);
    Req *r = find_req(j->key);
    if (r) { r->state = handle ? 2 : 3; r->handle = handle; }
    ReleaseSRWLockExclusive(&g_lock);
    shim_log("image %s: %s (%ux%u)", j->key, handle ? "ready" : "unavailable", w, h);
    if (handle && j->kind == 1) {
        uint8_t cb[20];
        int32_t iw = (int32_t)w, ih = (int32_t)h;
        memcpy(cb, &j->steamid, 8);
        memcpy(cb + 8, &handle, 4);
        memcpy(cb + 12, &iw, 4);
        memcpy(cb + 16, &ih, 4);
        shim_post_callback(CB_AVATAR_LOADED, cb, sizeof cb);
    } else if (j->kind == 2) {
        uint8_t cb[144];
        uint64_t game = shim_process_appid();
        memset(cb, 0, sizeof cb);
        memcpy(cb, &game, 8);
        memcpy(cb + 8, j->name, strnlen(j->name, 127));
        cb[136] = j->achieved;
        memcpy(cb + 140, &handle, 4);
        shim_post_callback(CB_ICON_FETCHED, cb, sizeof cb);
    }
    free(j);
    return 0;
}

static int request(Job *j) {
    AcquireSRWLockExclusive(&g_lock);
    Req *r = find_req(j->key);
    if (r) {
        int out = r->state == 2 ? r->handle : r->state == 1 ? -1 : 0;
        ReleaseSRWLockExclusive(&g_lock);
        free(j);
        return out;
    }
    if (g_n_reqs >= (int)(sizeof g_reqs / sizeof *g_reqs)) { ReleaseSRWLockExclusive(&g_lock); free(j); return 0; }
    r = &g_reqs[g_n_reqs++];
    snprintf(r->key, sizeof r->key, "%s", j->key);
    r->state = 1;
    ReleaseSRWLockExclusive(&g_lock);
    HANDLE t = CreateThread(NULL, 0, fetch_thread, j, 0, NULL);
    if (!t) {
        AcquireSRWLockExclusive(&g_lock);
        r->state = 3;
        ReleaseSRWLockExclusive(&g_lock);
        free(j);
        return 0;
    }
    CloseHandle(t);
    return -1;
}

static int avatar(uint64_t steamid, uint8_t size) {
    if (!steamid) return 0;
    Job *j = calloc(1, sizeof *j);
    if (!j) return 0;
    snprintf(j->key, sizeof j->key, "avatar:%llu:%u", (unsigned long long)steamid, size);
    j->kind = 1;
    j->steamid = steamid;
    j->payload[0] = 1;
    memcpy(j->payload + 1, &steamid, 8);
    j->payload[9] = size;
    j->len = 10;
    return request(j);
}

static int SHIM_TC small_avatar(void *self, uint64_t steamid) { (void)self; int h = avatar(steamid, 0); return h < 0 ? 0 : h; }
static int SHIM_TC medium_avatar(void *self, uint64_t steamid) { (void)self; int h = avatar(steamid, 1); return h < 0 ? 0 : h; }
static int SHIM_TC large_avatar(void *self, uint64_t steamid) { (void)self; return avatar(steamid, 2); }

static int avatar_waiting(uint64_t steamid, uint8_t size) {
    int h = avatar(steamid, size);
    for (int i = 0; h < 0 && i < 30; i++) { Sleep(50); h = avatar(steamid, size); }
    return h < 0 ? 0 : h;
}
static int SHIM_TC friend_avatar_v3(void *self, uint64_t steamid) { (void)self; return avatar_waiting(steamid, 0); }
static int SHIM_TC friend_avatar_v4(void *self, uint64_t steamid, int size) { (void)self; return avatar_waiting(steamid, size == 1 ? 1 : 0); }

int shim_achievement_achieved(const char *name);

static int SHIM_TC achievement_icon(void *self, const char *name) {
    (void)self;
    if (!name || !*name || strlen(name) > 120) return 0;
    int state = shim_achievement_achieved(name);
    if (state < 0) return 0;
    Job *j = calloc(1, sizeof *j);
    if (!j) return 0;
    snprintf(j->key, sizeof j->key, "icon:%s:%d", name, state);
    snprintf(j->name, sizeof j->name, "%s", name);
    j->kind = 2;
    j->achieved = (uint8_t)state;
    uint32_t app = shim_process_appid();
    size_t nlen = strlen(name);
    j->payload[0] = 2;
    memcpy(j->payload + 1, &app, 4);
    j->payload[5] = (uint8_t)state;
    memcpy(j->payload + 6, name, nlen);
    j->len = 6 + nlen;
    int h = request(j);
    return h < 0 ? 0 : h;
}

static uint64_t SHIM_TC image_size(void *self, int handle, uint32_t *w, uint32_t *h) {
    (void)self;
    AcquireSRWLockShared(&g_lock);
    bool ok = handle > 0 && handle <= g_n_imgs;
    if (w) *w = ok ? g_imgs[handle - 1].w : 0;
    if (h) *h = ok ? g_imgs[handle - 1].h : 0;
    ReleaseSRWLockShared(&g_lock);
    return ok;
}
static uint64_t SHIM_TC image_rgba(void *self, int handle, uint8_t *dest, int cap) {
    (void)self;
    AcquireSRWLockShared(&g_lock);
    bool ok = handle > 0 && handle <= g_n_imgs && dest && cap >= 0 && (size_t)cap >= (size_t)g_imgs[handle - 1].w * g_imgs[handle - 1].h * 4;
    if (ok) memcpy(dest, g_imgs[handle - 1].rgba, (size_t)g_imgs[handle - 1].w * g_imgs[handle - 1].h * 4);
    ReleaseSRWLockShared(&g_lock);
    return ok;
}

void shim_prefetch_own_avatar(void) {
    const Snapshot *s = shim_snapshot();
    if (!s || !s->steamid) return;
    for (uint8_t size = 0; size < 3; size++) avatar(s->steamid, size);
}

void register_images(void) {
    shim_register_family_ab("SteamFriends", "GetFriendAvatar", 8, (void *)friend_avatar_v3);
    shim_register_family_ab("SteamFriends", "GetFriendAvatar", 12, (void *)friend_avatar_v4);
    shim_register_family("SteamFriends", "GetSmallFriendAvatar", (void *)small_avatar);
    shim_register_family("SteamFriends", "GetMediumFriendAvatar", (void *)medium_avatar);
    shim_register_family("SteamFriends", "GetLargeFriendAvatar", (void *)large_avatar);
    shim_register_family("STEAMUSERSTATS_INTERFACE_VERSION", "GetAchievementIcon", (void *)achievement_icon);
    shim_register_family("SteamUtils", "GetImageSize", (void *)image_size);
    shim_register_family("SteamUtils", "GetImageRGBA", (void *)image_rgba);
}
