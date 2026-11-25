#define _CRT_RAND_S
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <winsock2.h>
#include <ws2tcpip.h>
#include <iphlpapi.h>
#include <windows.h>

#include "../shim.h"

static uint64_t SHIM_TC ret0(void *self) { (void)self; return 0; }
static uint64_t SHIM_TC get_hsteamuser(void *self) { (void)self; shim_trace("GetHSteamUser"); return 1; }
static uint64_t SHIM_TC logged_on(void *self) { (void)self; shim_trace("BLoggedOn"); return shim_session_online(); }

static uint64_t steamid(void) { const Snapshot *s = shim_snapshot(); return s ? s->steamid : 0; }

static uint64_t *SHIM_TC get_steamid(void *self, uint64_t *ret) { (void)self; shim_trace("GetSteamID"); *ret = steamid(); return ret; }

static void put32(uint8_t *o, uint32_t v) { memcpy(o, &v, 4); }

static void register_with_steam(uint32_t handle, const uint8_t *ticket, size_t n) {
    const Snapshot *s = shim_snapshot();
    if (!s || !s->has_ticket) return;
    uint8_t *payload = malloc(8 + n);
    if (!payload) return;
    put32(payload, shim_process_appid());
    put32(payload + 4, handle);
    memcpy(payload + 8, ticket, n);
    int32_t status = -99;
    bool ok = bridge_call(0xFFFF, 1, payload, 8 + n, &status, NULL, 0, NULL, 8000);
    free(payload);
    shim_log("ticket %u registration with Steam: call=%d status=%d (0 = confirmed by Steam)", handle, ok, status);
}

static DWORD WINAPI cancel_thread(LPVOID arg) {
    uint32_t handle = (uint32_t)(uintptr_t)arg;
    uint8_t payload[4];
    int32_t status = -99;
    put32(payload, handle);
    bridge_call(0xFFFF, 2, payload, sizeof payload, &status, NULL, 0, NULL, 4000);
    shim_trace("CancelAuthTicket handle=%u status=%d", handle, status);
    return 0;
}

static uint64_t SHIM_TC cancel_auth_ticket(void *self, uint32_t handle) {
    (void)self;
    HANDLE t = CreateThread(NULL, 0, cancel_thread, (LPVOID)(uintptr_t)handle, 0, NULL);
    if (t) CloseHandle(t); else cancel_thread((LPVOID)(uintptr_t)handle);
    return 0;
}

enum { BRIDGE_UNREACHABLE, BRIDGE_REFUSED, BRIDGE_OK };

static int ticket_from_bridge(uint32_t handle, void *buf, int cb_max, uint32_t *pcb) {
    const Snapshot *s = shim_snapshot();
    if (!s || !s->has_ticket || !buf || cb_max <= 0) return BRIDGE_UNREACHABLE;
    uint8_t payload[8];
    put32(payload, shim_process_appid());
    put32(payload + 4, handle);
    int32_t status = -99;
    size_t n = 0;
    if (!bridge_call(0xFFFF, 3, payload, sizeof payload, &status, buf, (size_t)cb_max, &n, 10000)) return BRIDGE_UNREACHABLE;
    if (status < 0 || n == 0) { shim_log("bridge refused to create the ticket: status=%d", status); return BRIDGE_REFUSED; }
    if (pcb) *pcb = (uint32_t)n;
    shim_log("GetAuthSessionTicket ok handle=%u bytes=%lu (created by Aurelia, status=%d; 0 = confirmed by Steam)", handle, (unsigned long)n, status);
    return BRIDGE_OK;
}

static void post_ticket_response(uint32_t handle) {
    uint8_t resp[8];
    put32(resp, handle);
    put32(resp + 4, 1);
    shim_post_callback(163, resp, sizeof resp);
}

static uint32_t random32(void) { unsigned int v = 0; rand_s(&v); return v; }

static bool ticket_from_file(void *buf, int cb_max, uint32_t *pcb) {
    const char *path = getenv("AURELIA_SHIM_TICKET_FILE");
    if (!path || !*path) return false;
    FILE *f = fopen(path, "rb");
    if (!f) { shim_log("AURELIA_SHIM_TICKET_FILE unreadable: %s", path); return false; }
    size_t n = fread(buf, 1, (size_t)(cb_max > 0 ? cb_max : 0), f);
    fclose(f);
    if (pcb) *pcb = (uint32_t)n;
    shim_log("GetAuthSessionTicket: ticket from file (%lu bytes)", (unsigned long)n);
    return n > 0;
}

static void log_identity(const uint8_t *id) {
    if (!id) { shim_log("GetAuthSessionTicket: identity=NULL"); return; }
    int32_t type, size;
    memcpy(&type, id, 4);
    memcpy(&size, id + 4, 4);
    if (size < 0 || size > 128) size = 128;
    char hex[2 * 128 + 1], text[129];
    for (int i = 0; i < size; i++) {
        sprintf(hex + 2 * i, "%02x", id[8 + i]);
        text[i] = id[8 + i] >= 32 && id[8 + i] < 127 ? (char)id[8 + i] : '.';
    }
    hex[2 * size] = 0;
    text[size] = 0;
    shim_log("GetAuthSessionTicket: identity type=%d size=%d text=%s hex=%s", type, size, text, hex);
}

static volatile LONG g_next_ticket_handle = 2;
#define next_handle ((uint32_t)g_next_ticket_handle)
#define take_handle() ((uint32_t)InterlockedIncrement(&g_next_ticket_handle) - 1)

static uint32_t auth_ticket_impl(void *buf, int cb_max, uint32_t *pcb, bool post_response) {
    shim_trace("GetAuthSessionTicket cb_max=%d", cb_max);
    const Snapshot *s = shim_snapshot();
    if (pcb) *pcb = 0;
    if (ticket_from_file(buf, cb_max, pcb)) {
        uint32_t handle = take_handle();
        uint8_t resp[8];
        put32(resp, handle);
        put32(resp + 4, 1);
        if (post_response) shim_post_callback(163, resp, sizeof resp);
        return handle;
    }
    int via = ticket_from_bridge(next_handle, buf, cb_max, pcb);
    if (via == BRIDGE_OK) {
        uint32_t handle = take_handle();
        if (post_response) post_ticket_response(handle);
        return handle;
    }
    if (via == BRIDGE_REFUSED && shim_connection_online()) return 0;
    if (via == BRIDGE_REFUSED) shim_log("GetAuthSessionTicket: Steam is down, ticket built from the snapshot fallback");

    if (!s || !s->has_ticket) { shim_log("GetAuthSessionTicket: snapshot has no ticket"); return 0; }
    const ShimTicket *t = &s->ticket;
    size_t need = 4 + t->token_len + 4 + 24 + 4 + t->ownership_len;
    if (!buf || cb_max < 0 || (size_t)cb_max < need) { shim_log("GetAuthSessionTicket: buffer too small (%d < %lu)", cb_max, (unsigned long)need); return 0; }
    uint8_t *o = buf;
    put32(o, (uint32_t)t->token_len); o += 4;
    memcpy(o, t->token, t->token_len); o += t->token_len;
    static uint32_t issued;
    put32(o, 24); o += 4;
    put32(o, 1); put32(o + 4, 2); put32(o + 8, random32()); put32(o + 12, random32()); put32(o + 16, GetTickCount()); put32(o + 20, ++issued); o += 24;
    put32(o, (uint32_t)t->ownership_len); o += 4;
    memcpy(o, t->ownership, t->ownership_len);
    if (pcb) *pcb = (uint32_t)need;
    uint32_t handle = take_handle();
    register_with_steam(handle, buf, need);
    uint8_t resp[8];
    put32(resp, handle);
    put32(resp + 4, 1);
    if (post_response) shim_post_callback(163, resp, sizeof resp);
    shim_log("GetAuthSessionTicket ok handle=%u bytes=%lu", handle, (unsigned long)need);
    return handle;
}

typedef struct { uint32_t handle; char identity[40]; } WebTicketJob;

static DWORD WINAPI web_ticket_thread(LPVOID arg) {
    WebTicketJob *j = arg;
    const Snapshot *s = shim_snapshot();
    static uint8_t resp[12 + 2560];
    uint8_t payload[14 + 40];
    uint8_t ticket[2560];
    size_t idn = strlen(j->identity), n = 0;
    int32_t status = -99, result = 2;
    memset(resp, 0, sizeof resp);
    if (s && s->has_ticket) {
        put32(payload, shim_process_appid());
        put32(payload + 4, j->handle);
        put32(payload + 8, 5);
        payload[12] = (uint8_t)idn; payload[13] = 0;
        memcpy(payload + 14, j->identity, idn);
        if (bridge_call_fresh(0xFFFF, 3, payload, 14 + idn, &status, ticket, sizeof ticket, &n, 15000) && status >= 0 && n > 0) {
            result = 1;
            memcpy(resp + 12, ticket, n);
            put32(resp + 8, (uint32_t)n);
        }
    }
    put32(resp, j->handle);
    put32(resp + 4, (uint32_t)result);
    shim_post_callback(168, resp, sizeof resp);
    shim_log("GetAuthTicketForWebApi handle=%u identity=%s result=%d bytes=%lu", j->handle, j->identity, result, (unsigned long)n);
    free(j);
    return 0;
}

static uint32_t SHIM_TC get_auth_ticket_for_web_api(void *self, const char *identity) {
    (void)self;
    if (!identity) identity = "";
    if (strlen(identity) + 4 >= 40) return 0;
    WebTicketJob *j = calloc(1, sizeof *j);
    if (!j) return 0;
    j->handle = take_handle();
    snprintf(j->identity, sizeof j->identity, "str:%s", identity);
    uint32_t h = j->handle;
    HANDLE t = CreateThread(NULL, 0, web_ticket_thread, j, 0, NULL);
    if (!t) { free(j); return 0; }
    CloseHandle(t);
    return h;
}

static bool parse_store_url(const CmReply *r, void *ctx, void *out, int *size) {
    (void)ctx;
    PbR in;
    PbField f;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field == 1 && f.wt == 2 && f.len > 0 && f.len < 512 && *size >= 512) {
            memset(out, 0, 512);
            memcpy(out, f.data, f.len);
            *size = 512;
            return true;
        }
    }
    return false;
}

static uint64_t SHIM_TC request_store_auth_url(void *self, const char *redirect) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    if (!redirect || !*redirect) { shim_callresult_fail_handle(call); return call; }
    PbW w = {0};
    pbw_bytes(&w, 1, redirect, strlen(redirect));
    shim_cm_async(call, 165, 151, true, "Econ.ClientGetItemShopOverlayAuthURL#1", &w, 15000, parse_store_url, NULL);
    return call;
}

typedef struct { bool known, phone, two_factor; DWORD at; } AccountFacts;
static AccountFacts g_account;
static SRWLOCK g_account_lock = SRWLOCK_INIT;

static volatile LONG g_account_busy;

static AccountFacts account_facts(void) {
    AcquireSRWLockExclusive(&g_account_lock);
    AccountFacts cur = g_account;
    ReleaseSRWLockExclusive(&g_account_lock);
    if (cur.known && GetTickCount() - cur.at < 600000) return cur;
    if (InterlockedCompareExchange(&g_account_busy, 1, 0) != 0) {
        for (int waited = 0; waited < 2500; waited += 25) {
            Sleep(25);
            AcquireSRWLockExclusive(&g_account_lock);
            cur = g_account;
            ReleaseSRWLockExclusive(&g_account_lock);
            if (cur.known) return cur;
            if (!g_account_busy) break;
        }
        return cur;
    }
    AccountFacts f = {0};
    uint8_t reply[16];
    size_t n = 0;
    int32_t status = -1;
    if (bridge_call_fresh(0xFFFF, 14, NULL, 0, &status, reply, sizeof reply, &n, 5000) && status == 0 && n >= 12 && reply[0]) {
        f.known = true;
        f.phone = reply[1] != 0;
        uint32_t state;
        memcpy(&state, reply + 4, 4);
        f.two_factor = state == 1;
    }
    const Snapshot *snap = shim_snapshot();
    if (snap && snap->steamid && f.known && !f.phone) {
        PbW w = {0};
        CmReply r = {0};
        if (shim_cm_service("Phone.AccountPhoneStatus#1", w.p, w.len, 5000, &r)) {
            PbR in;
            PbField fld;
            pbr_init(&in, r.body, r.len);
            while (pbr_next(&in, &fld)) if (fld.field == 1 && fld.v) f.phone = true;
        }
        shim_cm_reply_free(&r);
        pbw_free(&w);
    }
    if (f.known) {
        f.at = GetTickCount();
        AcquireSRWLockExclusive(&g_account_lock);
        g_account = f;
        ReleaseSRWLockExclusive(&g_account_lock);
    }
    InterlockedExchange(&g_account_busy, 0);
    return f;
}

static DWORD WINAPI account_preload_thread(LPVOID unused) { (void)unused; (void)account_facts(); return 0; }

void account_preload(void) {
    static volatile LONG once;
    if (InterlockedExchange(&once, 1)) return;
    HANDLE t = CreateThread(NULL, 0, account_preload_thread, NULL, 0, NULL);
    if (t) CloseHandle(t);
}
static uint64_t SHIM_TC is_phone_verified(void *self) { (void)self; return account_facts().phone; }
static uint64_t SHIM_TC is_two_factor_enabled(void *self) { (void)self; return account_facts().two_factor; }

#define MAX_REG 64
static struct { char name[96]; int32_t value; } g_reg[MAX_REG];
static SRWLOCK g_reg_lock = SRWLOCK_INIT;
static uint64_t SHIM_TC set_registry_int(void *self, const char *name, int32_t value) {
    (void)self;
    if (!name || !*name) return 0;
    uint64_t ok = 0;
    AcquireSRWLockExclusive(&g_reg_lock);
    int free_slot = -1;
    for (int i = 0; i < MAX_REG && !ok; i++) {
        if (g_reg[i].name[0] && !strcmp(g_reg[i].name, name)) { g_reg[i].value = value; ok = 1; }
        else if (!g_reg[i].name[0] && free_slot < 0) free_slot = i;
    }
    if (!ok && free_slot >= 0) { snprintf(g_reg[free_slot].name, sizeof g_reg[free_slot].name, "%s", name); g_reg[free_slot].value = value; ok = 1; }
    ReleaseSRWLockExclusive(&g_reg_lock);
    return ok;
}
static uint64_t SHIM_TC get_registry_int(void *self, const char *name, int32_t *value) {
    (void)self;
    if (!name || !*name) return 0;
    uint64_t found = 0;
    AcquireSRWLockExclusive(&g_reg_lock);
    for (int i = 0; i < MAX_REG; i++) if (g_reg[i].name[0] && !strcmp(g_reg[i].name, name)) { if (value) *value = g_reg[i].value; found = 1; }
    ReleaseSRWLockExclusive(&g_reg_lock);
    return found;
}

static uint64_t SHIM_TC behind_nat(void *self) {
    (void)self;
    ULONG len = 0;
    if (GetAdaptersAddresses(AF_INET, GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER, NULL, NULL, &len) != ERROR_BUFFER_OVERFLOW || !len) return 0;
    IP_ADAPTER_ADDRESSES *aa = malloc(len);
    bool priv = false;
    if (aa && GetAdaptersAddresses(AF_INET, GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER, NULL, aa, &len) == NO_ERROR) {
        for (IP_ADAPTER_ADDRESSES *a = aa; a; a = a->Next) {
            if (a->OperStatus != IfOperStatusUp || a->IfType == IF_TYPE_SOFTWARE_LOOPBACK) continue;
            for (IP_ADAPTER_UNICAST_ADDRESS *u = a->FirstUnicastAddress; u; u = u->Next) {
                const struct sockaddr_in *sa = (const struct sockaddr_in *)u->Address.lpSockaddr;
                uint32_t ip = ntohl(sa->sin_addr.s_addr);
                priv = priv || (ip >> 24) == 10 || (ip >> 20) == 0xAC1 || (ip >> 16) == 0xC0A8 || (ip >> 22) == 0x191;
            }
        }
    }
    free(aa);
    return priv;
}

static int SHIM_TC player_steam_level(void *self) {
    (void)self;
    static SRWLOCK lock = SRWLOCK_INIT;
    static int level = -1;
    static DWORD at;
    AcquireSRWLockExclusive(&lock);
    if (level >= 0 && GetTickCount() - at < 600000) { int v = level; ReleaseSRWLockExclusive(&lock); return v; }
    ReleaseSRWLockExclusive(&lock);
    const Snapshot *snap = shim_snapshot();
    if (!snap || !snap->steamid) return 0;
    PbW w = {0};
    pbw_varint(&w, 1, (uint32_t)(snap->steamid & 0xFFFFFFFFu));
    CmReply r = {0};
    int v = 0;
    bool got = false;
    if (shim_cm_call(7528, true, NULL, w.p, w.len, 8000, &r)) {
        PbR in;
        PbField f;
        pbr_init(&in, r.body, r.len);
        while (pbr_next(&in, &f)) {
            if (f.field != 1 || f.wt != 2) continue;
            PbR fr;
            PbField g;
            pbr_init(&fr, f.data, f.len);
            while (pbr_next(&fr, &g)) if (g.field == 2) { v = (int)(uint32_t)g.v; got = true; }
        }
    }
    if (got) {
        AcquireSRWLockExclusive(&lock);
        level = v;
        at = GetTickCount();
        ReleaseSRWLockExclusive(&lock);
    }
    shim_cm_reply_free(&r);
    pbw_free(&w);
    return v;
}

static uint32_t SHIM_TC get_auth_ticket_v3(void *self, void *buf, int cb_max, uint32_t *pcb) { (void)self; return auth_ticket_impl(buf, cb_max, pcb, true); }
static uint32_t SHIM_TC get_auth_ticket_v4(void *self, void *buf, int cb_max, uint32_t *pcb, const uint8_t *identity) {
    (void)self;
    log_identity(identity);
    return auth_ticket_impl(buf, cb_max, pcb, true);
}

#define MAX_CONNECTIONS 16
static struct { uint32_t ip, port, handle; bool used; } g_conn[MAX_CONNECTIONS];

static int initiate_game_connection(void *blob, int cb_max, uint32_t ip, uint32_t port) {
    uint32_t n = 0;
    uint32_t handle = auth_ticket_impl(blob, cb_max, &n, false);
    if (!handle || n == 0 || (int)n > cb_max) return 0;
    for (int i = 0; i < MAX_CONNECTIONS; i++) {
        if (g_conn[i].used) continue;
        g_conn[i].used = true; g_conn[i].ip = ip; g_conn[i].port = port; g_conn[i].handle = handle;
        break;
    }
    shim_trace("InitiateGameConnection %u.%u.%u.%u:%u -> %u bytes", ip & 255, (ip >> 8) & 255, (ip >> 16) & 255, ip >> 24, port, n);
    return (int)n;
}
static int SHIM_TC initiate_game_connection_v(void *self, void *blob, int cb_max, uint64_t server, uint32_t ip, uint32_t port, uint32_t secure) {
    (void)self; (void)server; (void)secure;
    return initiate_game_connection(blob, cb_max, ip, port);
}
static int SHIM_TC initiate_game_connection_old(void *self, void *blob, int cb_max, uint64_t server, uint64_t game, uint32_t ip, uint32_t port, uint32_t secure) {
    (void)self; (void)server; (void)game; (void)secure;
    return initiate_game_connection(blob, cb_max, ip, port);
}
static void SHIM_TC terminate_game_connection(void *self, uint32_t ip, uint32_t port) {
    (void)self;
    for (int i = 0; i < MAX_CONNECTIONS; i++) {
        if (!g_conn[i].used || g_conn[i].ip != ip || g_conn[i].port != port) continue;
        cancel_auth_ticket(NULL, g_conn[i].handle);
        g_conn[i].used = false;
    }
}

int shim_user_initiate_game_connection(void *blob, int cb_max, uint32_t ip, uint32_t port) { return initiate_game_connection(blob, cb_max, ip, port); }
void shim_user_terminate_game_connection(uint32_t ip, uint32_t port) { terminate_game_connection(NULL, ip, port); }

#define MAX_SESSIONS 32
static struct { uint64_t id; uint32_t appid; } g_sessions[MAX_SESSIONS];
static SRWLOCK g_sessions_lock = SRWLOCK_INIT;
static void remember_session(uint64_t id, uint32_t appid, bool add) {
    AcquireSRWLockExclusive(&g_sessions_lock);
    for (int i = 0; i < MAX_SESSIONS; i++) if (g_sessions[i].id == id) g_sessions[i].id = 0;
    for (int i = 0; add && i < MAX_SESSIONS; i++) if (!g_sessions[i].id) { g_sessions[i].id = id; g_sessions[i].appid = appid; break; }
    ReleaseSRWLockExclusive(&g_sessions_lock);
}
static bool session_has(uint64_t id, uint32_t appid) {
    bool yes = false;
    if (!id) return false;
    AcquireSRWLockExclusive(&g_sessions_lock);
    for (int i = 0; i < MAX_SESSIONS; i++) yes = yes || (g_sessions[i].id == id && g_sessions[i].appid == appid);
    ReleaseSRWLockExclusive(&g_sessions_lock);
    return yes;
}

static int SHIM_TC begin_auth_session(void *self, const void *ticket, int cb, uint64_t id) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    uint64_t owner = 0;
    int response = cb > 0 ? shim_verify_auth_ticket(ticket, (uint32_t)cb, s && s->has_ticket ? s->ticket.appid : 0, id, &owner) : 8;
    uint8_t resp[24] = {0};
    memcpy(resp, &id, 8);
    memcpy(resp + 8, &response, 4);
    memcpy(resp + 16, &owner, 8);
    shim_post_callback(143, resp, sizeof resp);
    if (response == 0) remember_session(id, s && s->has_ticket ? s->ticket.appid : 0, true);
    return response == 0 ? 0 : 1;
}
static void SHIM_TC end_auth_session(void *self, uint64_t id) { (void)self; remember_session(id, 0, false); }

enum { VOICE_TAIL_MS = 5000 };
static volatile LONG g_voice_recording;
static volatile DWORD g_voice_stopped_at;
static volatile LONG g_voice_stopped;
static uint32_t SHIM_TC voice_not_recording(void *self) { (void)self; return 2; }
static uint32_t SHIM_TC voice_no_data(void *self) {
    (void)self;
    bool tail = g_voice_stopped && GetTickCount() - g_voice_stopped_at < VOICE_TAIL_MS;
    return g_voice_recording || tail ? 3 : 2;
}
static uint32_t SHIM_TC voice_available(void *self, uint32_t *bytes, uint32_t *uncompressed, uint32_t rate) {
    (void)uncompressed; (void)rate;
    if (bytes) *bytes = 0;
    return voice_no_data(self);
}
static void SHIM_TC voice_start(void *self) { (void)self; g_voice_stopped = 0; InterlockedExchange(&g_voice_recording, 1); }
static void SHIM_TC voice_stop(void *self) {
    (void)self;
    if (InterlockedExchange(&g_voice_recording, 0)) { g_voice_stopped_at = GetTickCount(); g_voice_stopped = 1; }
}
static uint32_t SHIM_TC decompress_voice(void *self, const void *src, uint32_t cb, void *dest, uint32_t cap, uint32_t *written, uint32_t rate) {
    (void)self; (void)dest; (void)cap; (void)rate;
    if (written) *written = 0;
    return !src || !cb ? 5 : 7;
}
static uint32_t SHIM_TC decompress_voice20(void *self, const void *src, uint32_t cb, void *dest, uint32_t cap, uint32_t *written) { return decompress_voice(self, src, cb, dest, cap, written, 0); }
static uint32_t SHIM_TC voice_sample_rate(void *self) { (void)self; return 24000; }

static uint64_t SHIM_TC connected_true(void *self) { (void)self; return 1; }

static uint64_t SHIM_TC user_data_folder(void *self, char *buf, int cap) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    char win[1024];
    if (!buf || cap <= 0 || !s || !s->has_app || !s->app.cloud_dir || !shim_win_path(s->app.cloud_dir, win, sizeof win)) return 0;
    char *slash = strrchr(win, '\\');
    if (slash) *slash = 0;
    snprintf(buf, (size_t)cap, "%s", win);
    return 1;
}

static uint32_t SHIM_TC has_license(void *self, uint64_t id, uint32_t appid) {
    (void)self;
    shim_trace("UserHasLicenseForApp app=%u", appid);
    if (id == steamid()) return shim_app_owned(appid) ? 0 : 1;
    return session_has(id, appid) ? 0 : 2;
}

static uint64_t SHIM_TC set_duration_control_state(void *self, int state) { (void)self; (void)state; return 1; }
void register_user(void) {
    shim_register_family("SteamUser", "BSetDurationControlOnlineState", (void *)set_duration_control_state);
    shim_register_family("SteamUser", "GetHSteamUser", (void *)get_hsteamuser);
    shim_register_family("SteamUser", "BLoggedOn", (void *)logged_on);
    shim_register_family("SteamUser", "GetSteamID", (void *)get_steamid);
    shim_register_family("SteamUser", "GetAuthSessionTicket", (void *)get_auth_ticket_v3);
    shim_register("SteamUser023", "GetAuthSessionTicket", (void *)get_auth_ticket_v4);
    shim_register("SteamUser024", "GetAuthSessionTicket", (void *)get_auth_ticket_v4);
    shim_register_family("SteamUser", "UserHasLicenseForApp", (void *)has_license);
    shim_register_family("SteamUser", "CancelAuthTicket", (void *)cancel_auth_ticket);
    shim_register_family("SteamUser", "BeginAuthSession", (void *)begin_auth_session);
    shim_register_family("SteamUser", "GetAuthTicketForWebApi", (void *)get_auth_ticket_for_web_api);
    shim_register_family("SteamUser", "RequestStoreAuthURL", (void *)request_store_auth_url);
    shim_register_family("SteamUser", "EndAuthSession", (void *)end_auth_session);
    shim_register_family("SteamUser", "InitiateGameConnection", (void *)initiate_game_connection_v);
    shim_register_family("SteamUser", "InitiateGameConnection_DEPRECATED", (void *)initiate_game_connection_old);
    shim_register_family("SteamUser", "TerminateGameConnection", (void *)terminate_game_connection);
    shim_register_family("SteamUser", "TerminateGameConnection_DEPRECATED", (void *)terminate_game_connection);
    shim_register_family("SteamUser", "BConnected", (void *)connected_true);
    shim_register_family("SteamUser", "GetUserDataFolder", (void *)user_data_folder);
    shim_register_family("SteamUser", "GetAvailableVoice", (void *)voice_available);
    shim_register_family("SteamUser", "GetVoice", (void *)voice_no_data);
    shim_register_family("SteamUser", "GetCompressedVoice", (void *)voice_not_recording);
    shim_register_family("SteamUser", "DecompressVoice", (void *)voice_not_recording);
    shim_register_family_ab("SteamUser", "DecompressVoice", 24, (void *)decompress_voice);
    shim_register_family_ab("SteamUser", "DecompressVoice", 20, (void *)decompress_voice20);
    shim_register_family("SteamUser", "BIsBehindNAT", (void *)behind_nat);
    shim_register_family("SteamUser", "BIsPhoneVerified", (void *)is_phone_verified);
    shim_register_family("SteamUser", "BIsTwoFactorEnabled", (void *)is_two_factor_enabled);
    shim_register_family("SteamUser", "SetRegistryInt", (void *)set_registry_int);
    shim_register_family("SteamUser", "GetRegistryInt", (void *)get_registry_int);
    shim_register_family("SteamUser", "IsPrimaryChatDestination", (void *)connected_true);
    shim_register_family("SteamUser", "GetPlayerSteamLevel", (void *)player_steam_level);
    shim_register_family("SteamUser", "GetVoiceOptimalSampleRate", (void *)voice_sample_rate);
    shim_register_family("SteamUser", "StartVoiceRecording", (void *)voice_start);
    shim_register_family("SteamUser", "StopVoiceRecording", (void *)voice_stop);
    static const char *const zero[] = { "TrackAppUsageEvent", "AdvertiseGame",
        "BIsPhoneIdentifying", "BIsPhoneRequiringVerification", "GetGameBadgeLevel" };
    for (size_t i = 0; i < sizeof zero / sizeof *zero; i++) shim_register_family("SteamUser", zero[i], (void *)ret0);
}
