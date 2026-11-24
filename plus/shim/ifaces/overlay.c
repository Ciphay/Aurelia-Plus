#include <ctype.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <windows.h>

#include "../shim.h"

enum {
    CB_GAMEPAD_TEXT_DISMISSED = 714, CB_FLOATING_DISMISSED = 738, MAX_ENTERED_TEXT = 2048,
    SLOT_OVERLAY = 13, OP_REQUEST = 1, OP_POLL = 2, OP_STATE = 3, OP_DLC = 5, DLC_EVERY = 8,
    CB_GAME_OVERLAY_ACTIVATED = 331, MAX_TEXT = 2048, POLL_MS = 250, STATE_TTL_MS = 1000,
};

typedef struct Req { struct Req *next; char kind[40]; char *json; uint64_t order; } Req;

static SRWLOCK g_lock = SRWLOCK_INIT;
static Req *g_head, *g_tail;
static HANDLE g_wake;
static volatile LONG g_started, g_requests_seen;
static bool g_capable, g_state_known;
static DWORD g_state_at;

static char g_entered[MAX_ENTERED_TEXT + 1];
static bool g_entered_ok, g_text_open, g_floating_open;

static uint32_t app_id(void) { const Snapshot *s = shim_snapshot(); return s && s->has_ticket ? s->ticket.appid : 0; }

typedef struct { char *p; size_t len, cap; } Buf;

static void put(Buf *b, const char *s, size_t n) {
    if (b->len + n + 1 > b->cap) {
        size_t cap = b->cap ? b->cap : 256;
        while (b->len + n + 1 > cap) cap *= 2;
        char *g = realloc(b->p, cap);
        if (!g) return;
        b->p = g;
        b->cap = cap;
    }
    memcpy(b->p + b->len, s, n);
    b->len += n;
    b->p[b->len] = 0;
}

static void put_str(Buf *b, const char *s) { put(b, s, strlen(s)); }

static void put_text(Buf *b, const char *s) {
    size_t n = s ? strlen(s) : 0;
    if (n > MAX_TEXT) {
        n = MAX_TEXT;
        while (n && ((unsigned char)s[n] & 0xC0) == 0x80) n--;
    }
    put_str(b, "\"");
    for (size_t i = 0; i < n; i++) {
        unsigned char c = (unsigned char)s[i];
        char tmp[8];
        if (c == '"' || c == '\\') { tmp[0] = '\\'; tmp[1] = (char)c; put(b, tmp, 2); }
        else if (c < 0x20) { snprintf(tmp, sizeof tmp, "\\u%04x", c); put(b, tmp, 6); }
        else put(b, (const char *)&c, 1);
    }
    put_str(b, "\"");
}

static void put_u64(Buf *b, uint64_t v) {
    char tmp[24];
    snprintf(tmp, sizeof tmp, "%llu", (unsigned long long)v);
    put_str(b, tmp);
}

static void send_request(const Req *r) {
    size_t kl = strlen(r->kind), jl = strlen(r->json);
    if (jl > 0xFFFF) return;
    uint8_t *req = malloc(1 + 4 + 2 + kl + 2 + jl);
    if (!req) return;
    uint32_t app = app_id();
    size_t o = 0;
    req[o++] = OP_REQUEST;
    memcpy(req + o, &app, 4); o += 4;
    uint16_t k16 = (uint16_t)kl, j16 = (uint16_t)jl;
    memcpy(req + o, &k16, 2); o += 2;
    memcpy(req + o, r->kind, kl); o += kl;
    memcpy(req + o, &j16, 2); o += 2;
    memcpy(req + o, r->json, jl); o += jl;
    uint8_t reply[64];
    size_t n = 0;
    int32_t status = -1;
    bool ok = bridge_call_fresh(0xFFFF, SLOT_OVERLAY, req, o, &status, reply, sizeof reply, &n, 5000) && status == 0;
    shim_log("overlay: request %s %s", r->kind, ok ? "sent" : "failed");
    if (ok && r->order && n >= 2) {
        uint16_t il;
        memcpy(&il, reply, 2);
        if (il < 24 && (size_t)il + 2 <= n) {
            char id[24];
            memcpy(id, reply + 2, il);
            id[il] = 0;
            shim_microtxn_track(id, r->order);
        }
    }
    free(req);
}

static DWORD WINAPI worker_thread(LPVOID unused) {
    (void)unused;
    for (;;) {
        AcquireSRWLockExclusive(&g_lock);
        Req *r = g_head;
        if (r) { g_head = r->next; if (!g_head) g_tail = NULL; }
        ReleaseSRWLockExclusive(&g_lock);
        if (!r) { WaitForSingleObject(g_wake, INFINITE); continue; }
        send_request(r);
        free(r->json);
        free(r);
    }
    return 0;
}

static void post_activated(void *a, void *b, int active) {
    (void)a; (void)b;
    uint8_t cb[12] = {0};
    uint32_t app = app_id();
    cb[0] = (uint8_t)active;
    cb[1] = 1;
    memcpy(cb + 4, &app, 4);
    if (active) { uint8_t one = 1; shim_post_callback(1020048, &one, 1); Sleep(40); }
    shim_post_callback(CB_GAME_OVERLAY_ACTIVATED, cb, sizeof cb);
}

void shim_overlay_activated(int active) { shim_defer(post_activated, NULL, NULL, active); }
void shim_overlay_activated_now(int active) { post_activated(NULL, NULL, active); }

static void type_into_game(const char *utf8) {
    WCHAR wide[MAX_ENTERED_TEXT + 1];
    int n = MultiByteToWideChar(CP_UTF8, 0, utf8, -1, wide, MAX_ENTERED_TEXT + 1) - 1;
    for (int i = 0; i < n; i++) {
        INPUT in[2];
        memset(in, 0, sizeof in);
        in[0].type = in[1].type = INPUT_KEYBOARD;
        if (wide[i] == L'\b' || wide[i] == L'\n' || wide[i] == L'\r') {
            in[0].ki.wVk = in[1].ki.wVk = wide[i] == L'\b' ? VK_BACK : VK_RETURN;
        } else {
            in[0].ki.wScan = in[1].ki.wScan = wide[i];
            in[0].ki.dwFlags = KEYEVENTF_UNICODE;
            in[1].ki.dwFlags = KEYEVENTF_UNICODE | KEYEVENTF_KEYUP;
        }
        in[1].ki.dwFlags |= KEYEVENTF_KEYUP;
        SendInput(2, in, sizeof(INPUT));
    }
}

static void post_floating_dismissed(void *cb, void *unused, int c) {
    (void)unused; (void)c;
    shim_post_callback(CB_FLOATING_DISMISSED, cb, 1);
    free(cb);
}

static void post_text_dismissed(void *cb, void *unused, int c) {
    (void)unused; (void)c;
    shim_post_callback(CB_GAMEPAD_TEXT_DISMISSED, cb, 12);
    free(cb);
}

static void post_join_requested(void *join, void *unused, int c) {
    (void)unused; (void)c;
    shim_post_callback(333, join, 16);
    free(join);
}

static void handle_updates(const uint8_t *buf, size_t n) {
    if (n < 4) return;
    uint32_t count;
    memcpy(&count, buf, 4);
    size_t o = 4;
    for (uint32_t i = 0; i < count && o + 3 <= n; i++) {
        uint8_t type = buf[o];
        uint16_t il;
        memcpy(&il, buf + o + 1, 2);
        o += 3;
        if (o + il > n || il >= 32) return;
        char id[32];
        memcpy(id, buf + o, il);
        id[il] = 0;
        o += il;
        if (type == 8) {
            if (o + 3 > n) return;
            bool done = buf[o];
            uint16_t tl;
            memcpy(&tl, buf + o + 1, 2);
            o += 3;
            if (o + tl > n || tl > MAX_ENTERED_TEXT) return;
            char text[MAX_ENTERED_TEXT + 1];
            memcpy(text, buf + o, tl);
            text[tl] = 0;
            o += tl;
            type_into_game(text);
            if (done) {
                AcquireSRWLockExclusive(&g_lock);
                g_floating_open = false;
                ReleaseSRWLockExclusive(&g_lock);
                uint8_t *cb = calloc(1, 4);
                if (cb) shim_defer(post_floating_dismissed, cb, NULL, 0);
            }
            continue;
        }
        if (type == 7) {
            if (o + 3 > n) return;
            bool submitted = buf[o];
            uint16_t tl;
            memcpy(&tl, buf + o + 1, 2);
            o += 3;
            if (o + tl > n || tl > MAX_ENTERED_TEXT) return;
            AcquireSRWLockExclusive(&g_lock);
            memcpy(g_entered, buf + o, tl);
            g_entered[tl] = 0;
            g_entered_ok = submitted;
            g_text_open = false;
            ReleaseSRWLockExclusive(&g_lock);
            o += tl;
            uint8_t *cb = calloc(1, 12);
            if (cb) {
                uint32_t len = submitted ? (uint32_t)tl + 1 : 0, app = app_id();
                cb[0] = submitted;
                memcpy(cb + 4, &len, 4);
                memcpy(cb + 8, &app, 4);
                shim_defer(post_text_dismissed, cb, NULL, 0);
            }
            continue;
        }
        if (type == 6) {
            if (o + 16 > n) return;
            uint8_t *join = malloc(16);
            if (join) { memcpy(join, buf + o, 16); shim_defer(post_join_requested, join, NULL, 0); }
            o += 16;
            continue;
        }
        if ((type == 4 || type == 5) && shim_microtxn_owns(id)) continue;
        if (type == 4 || type == 5) shim_defer(post_activated, NULL, NULL, type == 4);
        else if (type == 1) shim_microtxn_answer(id, 1, 0);
        else if (type == 2) shim_microtxn_answer(id, 0, 0);
        else if (type == 3) shim_microtxn_answer(id, 0, 1);
    }
}

static DWORD WINAPI poll_thread(LPVOID unused) {
    (void)unused;
    static uint8_t reply[4096];
    for (unsigned tick = 0;; tick++) {
        Sleep(POLL_MS);
        size_t n = 0;
        int32_t status = -1;
        if (g_requests_seen) {
            uint8_t req = OP_POLL;
            if (bridge_call_fresh(0xFFFF, SLOT_OVERLAY, &req, 1, &status, reply, sizeof reply, &n, 3000) && status == 0) handle_updates(reply, n);
        }
        if (tick % DLC_EVERY == 0) {
            uint8_t req = OP_DLC;
            n = 0;
            status = -1;
            if (bridge_call_fresh(0xFFFF, SLOT_OVERLAY, &req, 1, &status, reply, sizeof reply, &n, 3000) && status == 0) shim_dlc_apply(reply, n);
        }
    }
    return 0;
}

void shim_overlay_start(void) {
    if (InterlockedCompareExchange(&g_started, 1, 0)) return;
    g_wake = CreateEventA(NULL, FALSE, FALSE, NULL);
    HANDLE t = CreateThread(NULL, 0, worker_thread, NULL, 0, NULL);
    if (t) CloseHandle(t);
    t = CreateThread(NULL, 0, poll_thread, NULL, 0, NULL);
    if (t) CloseHandle(t);
}

static void enqueue_with(const char *kind, Buf *json, uint64_t order) {
    Req *r = calloc(1, sizeof *r);
    if (!r || !json->p) { free(r); free(json->p); return; }
    snprintf(r->kind, sizeof r->kind, "%s", kind);
    shim_trace("overlay: request '%s' %s", kind, json->p);
    r->order = order;
    r->json = json->p;
    InterlockedExchange(&g_requests_seen, 1);
    shim_overlay_start();
    AcquireSRWLockExclusive(&g_lock);
    if (g_tail) g_tail->next = r; else g_head = r;
    g_tail = r;
    ReleaseSRWLockExclusive(&g_lock);
    while (!g_wake) Sleep(1);
    SetEvent(g_wake);
}

static void enqueue(const char *kind, Buf *json) { enqueue_with(kind, json, 0); }

void shim_overlay_submit(const char *kind, const char *json, uint64_t order) {
    Buf b = {0};
    put_str(&b, json);
    enqueue_with(kind, &b, order);
}

void shim_overlay_achievement(const char *name) {
    if (!name || !*name) return;
    Buf j = {0};
    put_str(&j, "{\"api_name\":");
    put_text(&j, name);
    put_str(&j, "}");
    enqueue("achievement", &j);
}

enum { COPLAY_MAX = 64, OP_RECENT = 6, COPLAY_TTL_MS = 2000 };
static struct { uint64_t id; uint32_t when, app; } g_coplay[COPLAY_MAX];
static int g_ncoplay;
static DWORD g_coplay_at;
static bool g_coplay_valid;

static void coplay_record(uint64_t id) {
    AcquireSRWLockExclusive(&g_lock);
    int at = g_ncoplay;
    for (int i = 0; i < g_ncoplay; i++) if (g_coplay[i].id == id) { at = i; break; }
    if (at == g_ncoplay) { if (g_ncoplay < COPLAY_MAX) g_ncoplay++; else at = COPLAY_MAX - 1; }
    for (int i = at; i > 0; i--) g_coplay[i] = g_coplay[i - 1];
    g_coplay[0].id = id;
    g_coplay[0].when = (uint32_t)time(NULL);
    g_coplay[0].app = app_id();
    ReleaseSRWLockExclusive(&g_lock);
}

static void SHIM_TC set_played_with(void *self, uint64_t steam_id) {
    (void)self;
    if (!steam_id) return;
    coplay_record(steam_id);
    char tmp[48];
    snprintf(tmp, sizeof tmp, "{\"steam_id\":%llu}", (unsigned long long)steam_id);
    Buf j = {0};
    put_str(&j, tmp);
    enqueue("played_with", &j);
}

static void refresh_coplay(void) {
    AcquireSRWLockShared(&g_lock);
    bool fresh = g_coplay_valid && GetTickCount() - g_coplay_at < COPLAY_TTL_MS;
    ReleaseSRWLockShared(&g_lock);
    if (fresh) return;
    static uint8_t reply[4 + 16 * 256];
    uint8_t req = OP_RECENT;
    size_t n = 0;
    int32_t status = -1;
    if (!bridge_call_fresh(0xFFFF, SLOT_OVERLAY, &req, 1, &status, reply, sizeof reply, &n, 3000) || status != 0 || n < 4) return;
    uint32_t count;
    memcpy(&count, reply, 4);
    if (count > 256 || n < 4 + 16 * (size_t)count) return;
    AcquireSRWLockExclusive(&g_lock);
    typeof(g_coplay) merged = {0};
    int m = 0;
    for (uint32_t i = 0; i < count && m < COPLAY_MAX; i++) {
        memcpy(&merged[m].id, reply + 4 + 16 * i, 8);
        memcpy(&merged[m].app, reply + 12 + 16 * i, 4);
        memcpy(&merged[m].when, reply + 16 + 16 * i, 4);
        m++;
    }
    for (int i = 0; i < g_ncoplay && m < COPLAY_MAX; i++) {
        bool seen = false;
        for (int k = 0; k < m; k++) seen = seen || merged[k].id == g_coplay[i].id;
        if (!seen) merged[m++] = g_coplay[i];
    }
    memcpy(g_coplay, merged, sizeof g_coplay);
    g_ncoplay = m;
    g_coplay_valid = true;
    g_coplay_at = GetTickCount();
    ReleaseSRWLockExclusive(&g_lock);
}

static int SHIM_TC coplay_friend_count(void *self) { (void)self; refresh_coplay(); AcquireSRWLockShared(&g_lock); int n = g_ncoplay; ReleaseSRWLockShared(&g_lock); return n; }
static uint64_t *SHIM_TC coplay_friend(void *self, uint64_t *ret, int i) {
    (void)self;
    refresh_coplay();
    AcquireSRWLockShared(&g_lock);
    *ret = i >= 0 && i < g_ncoplay ? g_coplay[i].id : 0;
    ReleaseSRWLockShared(&g_lock);
    return ret;
}
static uint32_t SHIM_TC friend_coplay_time(void *self, uint64_t id) {
    (void)self;
    refresh_coplay();
    uint32_t when = 0;
    AcquireSRWLockShared(&g_lock);
    for (int i = 0; i < g_ncoplay; i++) if (g_coplay[i].id == id) when = g_coplay[i].when;
    ReleaseSRWLockShared(&g_lock);
    return when;
}
static uint32_t SHIM_TC friend_coplay_game(void *self, uint64_t id) {
    refresh_coplay();
    uint32_t app = 0;
    AcquireSRWLockShared(&g_lock);
    for (int i = 0; i < g_ncoplay; i++) if (g_coplay[i].id == id) app = g_coplay[i].app;
    ReleaseSRWLockShared(&g_lock);
    return app;
}

static void SHIM_TC activate_overlay(void *self, const char *dialog) {
    (void)self;
    char kind[40] = "other";
    if (dialog && *dialog) {
        size_t n = 0;
        for (; dialog[n] && n < sizeof kind - 1; n++) kind[n] = isalnum((unsigned char)dialog[n]) ? (char)tolower((unsigned char)dialog[n]) : '_';
        kind[n] = 0;
    }
    Buf j = {0};
    put_str(&j, "{}");
    enqueue(kind, &j);
}

static void SHIM_TC activate_overlay_to_user(void *self, const char *dialog, uint64_t steam_id) {
    (void)self;
    Buf j = {0};
    put_str(&j, "{\"dialog\":");
    put_text(&j, dialog ? dialog : "");
    put_str(&j, ",\"steam_id\":");
    put_u64(&j, steam_id);
    put_str(&j, "}");
    enqueue("user", &j);
}

static void web_page(const char *url, int mode) {
    Buf j = {0};
    put_str(&j, "{\"url\":");
    put_text(&j, url ? url : "");
    put_str(&j, mode == 1 ? ",\"mode\":\"modal\"}" : ",\"mode\":\"default\"}");
    enqueue("web_page", &j);
}

void shim_overlay_web_page(const char *url) { web_page(url, 0); }

static void SHIM_TC activate_overlay_to_web_page_old(void *self, const char *url) { (void)self; web_page(url, 0); }
static void SHIM_TC activate_overlay_to_web_page(void *self, const char *url, int mode) { (void)self; web_page(url, mode); }

static void SHIM_TC activate_overlay_to_store(void *self, uint32_t app, int flag) {
    (void)self;
    char tmp[64];
    snprintf(tmp, sizeof tmp, "{\"target\":{\"app_id\":%u,\"flag\":%d}}", app, flag);
    Buf j = {0};
    put_str(&j, tmp);
    enqueue("store", &j);
}

static void SHIM_TC invite_dialog(void *self, uint64_t lobby) {
    (void)self;
    Buf j = {0};
    put_str(&j, "{\"lobby\":");
    put_u64(&j, lobby);
    put_str(&j, "}");
    enqueue("invite", &j);
}

static void SHIM_TC invite_dialog_connect_string(void *self, const char *connect) {
    (void)self;
    Buf j = {0};
    put_str(&j, "{\"connect\":");
    put_text(&j, connect ? connect : "");
    put_str(&j, "}");
    enqueue("invite", &j);
}

static void SHIM_TC remote_play_invite_dialog(void *self, uint64_t lobby) {
    (void)self;
    Buf j = {0};
    put_str(&j, "{\"lobby\":");
    put_u64(&j, lobby);
    put_str(&j, "}");
    enqueue("remote_play_invite", &j);
}

static bool show_text(int mode, int line_mode, const char *description, uint32_t max_chars, const char *existing) {
    bool capable = shim_overlay_capable();
    shim_log("overlay: the game requested a keyboard (ShowGamepadTextInput, mode %d, max %u): %s", mode, (unsigned)max_chars, capable ? "forwarding" : "refused, launcher did not declare capable");
    if (!capable) return false;
    char nums[96];
    snprintf(nums, sizeof nums, "{\"mode\":%d,\"line_mode\":%d,\"max_chars\":%u,\"description\":", mode, line_mode, (unsigned)max_chars);
    Buf j = {0};
    put_str(&j, nums);
    put_text(&j, description ? description : "");
    put_str(&j, ",\"existing\":");
    put_text(&j, existing ? existing : "");
    put_str(&j, "}");
    AcquireSRWLockExclusive(&g_lock);
    g_text_open = true;
    g_entered_ok = false;
    g_entered[0] = 0;
    ReleaseSRWLockExclusive(&g_lock);
    enqueue("gamepad_text", &j);
    return true;
}
static uint64_t SHIM_TC show_gamepad_text_input(void *self, int mode, int line_mode, const char *description, uint32_t max_chars, const char *existing) { (void)self; return show_text(mode, line_mode, description, max_chars, existing); }
static uint64_t SHIM_TC show_gamepad_text_input_old(void *self, int mode, int line_mode, const char *description, uint32_t max_chars) { (void)self; return show_text(mode, line_mode, description, max_chars, NULL); }
static uint32_t SHIM_TC entered_text_length(void *self) {
    (void)self;
    AcquireSRWLockShared(&g_lock);
    uint32_t n = g_entered_ok ? (uint32_t)strlen(g_entered) + 1 : 1;
    ReleaseSRWLockShared(&g_lock);
    return n;
}
static uint64_t SHIM_TC entered_text_input(void *self, char *buf, uint32_t cap) {
    (void)self;
    AcquireSRWLockShared(&g_lock);
    bool ok = g_entered_ok && buf && cap > 0;
    if (ok) snprintf(buf, cap, "%s", g_entered);
    ReleaseSRWLockShared(&g_lock);
    return ok;
}
static uint64_t SHIM_TC show_floating_text_input(void *self, int mode, int left, int top, int width, int height) {
    (void)self;
    bool capable = shim_overlay_capable();
    shim_log("overlay: the game requested a floating keyboard (ShowFloatingGamepadTextInput, mode %d): %s", mode, capable ? "forwarding" : "refused, launcher did not declare capable");
    if (!capable) return false;
    char json[200];
    snprintf(json, sizeof json, "{\"floating\":true,\"mode\":%d,\"left\":%d,\"top\":%d,\"width\":%d,\"height\":%d}", mode, left, top, width, height);
    Buf j = {0};
    put_str(&j, json);
    AcquireSRWLockExclusive(&g_lock);
    g_floating_open = true;
    ReleaseSRWLockExclusive(&g_lock);
    enqueue("gamepad_text", &j);
    return true;
}
static uint64_t SHIM_TC dismiss_floating_text_input(void *self) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    bool was = g_floating_open;
    g_floating_open = false;
    ReleaseSRWLockExclusive(&g_lock);
    if (!was) return false;
    Buf j = {0};
    put_str(&j, "{}");
    enqueue("gamepad_text_close", &j);
    uint8_t *cb = calloc(1, 4);
    if (cb) shim_defer(post_floating_dismissed, cb, NULL, 0);
    return true;
}

int Aurelia_Keyboard(int show) {
    if (show) {
        AcquireSRWLockExclusive(&g_lock);
        bool open = g_floating_open;
        ReleaseSRWLockExclusive(&g_lock);
        if (open) return 1;
        return show_floating_text_input(NULL, 0, 0, 0, 0, 0) != 0;
    }
    return dismiss_floating_text_input(NULL) != 0;
}

int Aurelia_KeyboardOpen(void) {
    AcquireSRWLockExclusive(&g_lock);
    bool open = g_floating_open;
    ReleaseSRWLockExclusive(&g_lock);
    return open;
}

static uint64_t SHIM_TC dismiss_gamepad_text_input(void *self) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    bool was = g_text_open;
    g_text_open = false;
    ReleaseSRWLockExclusive(&g_lock);
    if (was) {
        uint8_t *cb = calloc(1, 12);
        if (cb) { uint32_t app = app_id(); memcpy(cb + 8, &app, 4); shim_defer(post_text_dismissed, cb, NULL, 0); }
    }
    return was;
}

static uint64_t SHIM_TC is_overlay_enabled(void *self) {
    (void)self;
    shim_trace("IsOverlayEnabled");
    DWORD now = GetTickCount();
    AcquireSRWLockExclusive(&g_lock);
    bool fresh = g_state_known && now - g_state_at < STATE_TTL_MS;
    bool cached = g_capable;
    ReleaseSRWLockExclusive(&g_lock);
    if (fresh) return cached && shim_connection_online();
    uint8_t req = OP_STATE, reply[4];
    size_t n = 0;
    int32_t status = -1;
    bool ok = bridge_call_fresh(0xFFFF, SLOT_OVERLAY, &req, 1, &status, reply, sizeof reply, &n, 1500) && status == 0 && n >= 1;
    AcquireSRWLockExclusive(&g_lock);
    g_capable = ok && reply[0] != 0;
    g_state_known = true;
    g_state_at = now;
    cached = g_capable;
    ReleaseSRWLockExclusive(&g_lock);
    return cached && shim_connection_online();
}

bool shim_overlay_capable(void) { return is_overlay_enabled(NULL) != 0; }

void register_overlay(void) {
    const char *f = "SteamFriends";
    shim_register_family(f, "ActivateGameOverlay", (void *)activate_overlay);
    shim_register_family(f, "ActivateGameOverlayToUser", (void *)activate_overlay_to_user);
    shim_register_family_ab(f, "ActivateGameOverlayToWebPage", 4, (void *)activate_overlay_to_web_page_old);
    shim_register_family_ab(f, "ActivateGameOverlayToWebPage", 8, (void *)activate_overlay_to_web_page);
    shim_register_family(f, "ActivateGameOverlayToStore", (void *)activate_overlay_to_store);
    shim_register_family(f, "SetPlayedWith", (void *)set_played_with);
    shim_register_family(f, "GetCoplayFriendCount", (void *)coplay_friend_count);
    shim_register_family(f, "GetCoplayFriend", (void *)coplay_friend);
    shim_register_family(f, "GetFriendCoplayTime", (void *)friend_coplay_time);
    shim_register_family(f, "GetFriendCoplayGame", (void *)friend_coplay_game);
    shim_register_family(f, "ActivateGameOverlayInviteDialog", (void *)invite_dialog);
    shim_register_family(f, "ActivateGameOverlayInviteDialogConnectString", (void *)invite_dialog_connect_string);
    shim_register_family(f, "ActivateGameOverlayRemotePlayTogetherInviteDialog", (void *)remote_play_invite_dialog);
    shim_register_family("SteamUtils", "IsOverlayEnabled", (void *)is_overlay_enabled);
    shim_register_family_ab("SteamUtils", "ShowGamepadTextInput", 16, (void *)show_gamepad_text_input_old);
    shim_register_family_ab("SteamUtils", "ShowGamepadTextInput", 20, (void *)show_gamepad_text_input);
    shim_register_family("SteamUtils", "GetEnteredGamepadTextLength", (void *)entered_text_length);
    shim_register_family("SteamUtils", "GetEnteredGamepadTextInput", (void *)entered_text_input);
    shim_register_family("SteamUtils", "DismissGamepadTextInput", (void *)dismiss_gamepad_text_input);
    shim_register_family("SteamUtils", "ShowFloatingGamepadTextInput", (void *)show_floating_text_input);
    shim_register_family("SteamUtils", "DismissFloatingGamepadTextInput", (void *)dismiss_floating_text_input);
}
