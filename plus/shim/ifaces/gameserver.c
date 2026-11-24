#include <string.h>

#include "../shim.h"

#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <windows.h>

enum { SLOT_GAMESERVER = 12, OP_LOGON = 1, OP_LOGOFF = 2, MAX_RULES = 64, MAX_PLAYERS = 128, MAX_SESSIONS = 64, MAX_PACKETS = 16, PACKET_MAX = 1400 };
typedef struct { char key[64], value[256]; } Rule;
typedef struct { uint64_t id; char name[64]; int32_t score; DWORD since; } Player;
typedef struct { uint64_t id; uint32_t appid; } Session;
typedef struct { uint8_t data[PACKET_MAX]; int len; uint32_t ip; uint16_t port; } Packet;

static struct {
    bool inited, logged_on, dedicated, password;
    uint32_t ip, flags, appid, public_ip;
    uint16_t game_port, query_port, spec_port;
    char version[64], product[32], desc[64], mod_dir[32], name[64], map[32], region[8], tags[128], gamedata[128], spec_name[64];
    int max_players, bots;
    uint64_t steamid;
    Rule rules[MAX_RULES];
    int n_rules;
    Player players[MAX_PLAYERS];
    int n_players;
    Session sessions[MAX_SESSIONS];
    Packet out[MAX_PACKETS];
    int out_head, out_count;
    uint32_t unauth_counter;
} g;
static SRWLOCK g_lock = SRWLOCK_INIT;

static uint64_t SHIM_TC gs_true(void *self) { (void)self; return 1; }
static uint64_t SHIM_TC gs_false(void *self) { (void)self; return 0; }
static void SHIM_TC gs_void(void *self) { (void)self; }
static void push_server_state(void);
static void SHIM_TC gs_force_heartbeat(void *self) { (void)self; if (g.logged_on) push_server_state(); }

static void set_text(char *dst, size_t cap, const char *src) {
    AcquireSRWLockExclusive(&g_lock);
    snprintf(dst, cap, "%s", src ? src : "");
    ReleaseSRWLockExclusive(&g_lock);
}
static void SHIM_TC gs_set_product(void *self, const char *v) { (void)self; set_text(g.product, sizeof g.product, v); }
static void SHIM_TC gs_set_desc(void *self, const char *v) { (void)self; set_text(g.desc, sizeof g.desc, v); }
static void SHIM_TC gs_set_moddir(void *self, const char *v) { (void)self; set_text(g.mod_dir, sizeof g.mod_dir, v); }
static void SHIM_TC gs_set_name(void *self, const char *v) { (void)self; set_text(g.name, sizeof g.name, v); }
static void SHIM_TC gs_set_map(void *self, const char *v) { (void)self; set_text(g.map, sizeof g.map, v); }
static void SHIM_TC gs_set_region(void *self, const char *v) { (void)self; set_text(g.region, sizeof g.region, v); }
static void SHIM_TC gs_set_tags(void *self, const char *v) { (void)self; set_text(g.tags, sizeof g.tags, v); }
static void SHIM_TC gs_set_gamedata(void *self, const char *v) { (void)self; set_text(g.gamedata, sizeof g.gamedata, v); }
static void SHIM_TC gs_set_spec_name(void *self, const char *v) { (void)self; set_text(g.spec_name, sizeof g.spec_name, v); }
static void SHIM_TC gs_set_max(void *self, int v) { (void)self; g.max_players = v; }
static void SHIM_TC gs_set_bots(void *self, int v) { (void)self; g.bots = v; }
static void SHIM_TC gs_set_password(void *self, uint8_t v) { (void)self; g.password = v; }
static void SHIM_TC gs_set_dedicated(void *self, uint8_t v) { (void)self; g.dedicated = v; }
static void SHIM_TC gs_set_spec_port(void *self, uint16_t v) { (void)self; g.spec_port = v; }

static void SHIM_TC gs_clear_rules(void *self) { (void)self; AcquireSRWLockExclusive(&g_lock); g.n_rules = 0; ReleaseSRWLockExclusive(&g_lock); }
static void SHIM_TC gs_set_rule(void *self, const char *k, const char *v) {
    (void)self;
    if (!k || !*k) return;
    AcquireSRWLockExclusive(&g_lock);
    int at = -1;
    for (int i = 0; i < g.n_rules; i++) if (!strcmp(g.rules[i].key, k)) at = i;
    if (at < 0 && g.n_rules < MAX_RULES) at = g.n_rules++;
    if (at >= 0) { snprintf(g.rules[at].key, sizeof g.rules[at].key, "%s", k); snprintf(g.rules[at].value, sizeof g.rules[at].value, "%s", v ? v : ""); }
    ReleaseSRWLockExclusive(&g_lock);
}

static uint64_t SHIM_TC gs_init(void *self, uint32_t ip, uint16_t game_port, uint16_t query_port, uint32_t flags, uint32_t appid, const char *version) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    g.inited = true; g.ip = ip; g.game_port = game_port; g.query_port = query_port; g.flags = flags;
    g.appid = appid ? appid : (shim_snapshot() && shim_snapshot()->has_ticket ? shim_snapshot()->ticket.appid : 0);
    snprintf(g.version, sizeof g.version, "%s", version ? version : "");
    ReleaseSRWLockExclusive(&g_lock);
    return 1;
}

static void post_connected(bool ok, int32_t result) {
    if (ok) {
        uint8_t empty = 0, policy[1] = { 0 };
        shim_post_callback(101, &empty, 1);
        shim_post_callback(115, policy, sizeof policy);
    } else {
        uint8_t cb[8] = {0};
        memcpy(cb, &result, 4);
        shim_post_callback(102, cb, sizeof cb);
    }
}

static void push_server_state(void) {
    PbW t = {0}, d = {0};
    AcquireSRWLockExclusive(&g_lock);
    pbw_varint(&t, 1, g.appid);
    pbw_varint(&t, 2, (g.dedicated ? 4 : 0) | 1);
    pbw_varint(&t, 3, g.public_ip);
    pbw_varint(&t, 4, g.game_port);
    pbw_str(&t, 5, g.mod_dir);
    pbw_str(&t, 6, g.version);
    pbw_varint(&t, 7, g.query_port);
    pbw_varint(&d, 3, g.query_port);
    pbw_varint(&d, 4, g.game_port);
    if (g.spec_port) pbw_varint(&d, 5, g.spec_port);
    pbw_varint(&d, 6, g.appid);
    pbw_str(&d, 7, g.mod_dir);
    pbw_str(&d, 8, g.version);
    pbw_str(&d, 9, g.product[0] ? g.product : g.mod_dir);
    if (g.region[0]) pbw_str(&d, 10, g.region);
    for (int i = 0; i < g.n_players; i++) {
        PbW pl = {0};
        pbw_fixed64(&pl, 1, g.players[i].id);
        pbw_msg(&d, 11, &pl);
        pbw_free(&pl);
    }
    pbw_varint(&d, 12, (uint64_t)g.max_players);
    pbw_varint(&d, 13, (uint64_t)g.bots);
    pbw_varint(&d, 14, g.password);
    pbw_varint(&d, 15, 0);
    pbw_varint(&d, 16, g.dedicated);
    pbw_str(&d, 17, "w");
    if (g.gamedata[0]) pbw_str(&d, 18, g.gamedata);
    if (g.tags[0]) pbw_str(&d, 20, g.tags);
    if (g.map[0]) pbw_str(&d, 21, g.map);
    if (g.name[0]) pbw_str(&d, 22, g.name);
    if (g.spec_name[0]) pbw_str(&d, 27, g.spec_name);
    if (g.desc[0]) pbw_str(&d, 29, g.desc);
    static uint32_t revision;
    pbw_varint(&d, 24, ++revision);
    ReleaseSRWLockExclusive(&g_lock);
    shim_cm_gs_send(908, t.p, t.len);
    shim_cm_gs_send(4331, d.p, d.len);
    pbw_free(&t);
    pbw_free(&d);
}

static DWORD WINAPI heartbeat_thread(LPVOID unused) {
    (void)unused;
    for (;;) {
        for (int i = 0; i < 300; i++) {
            Sleep(100);
            if (!g.logged_on) return 0;
        }
        push_server_state();
    }
}

static DWORD WINAPI logon_thread(LPVOID arg) {
    char *token = arg;
    uint8_t req[1 + 128] = { OP_LOGON }, reply[16];
    size_t req_len = 1;
    if (token) { size_t n = strlen(token); if (n > 127) n = 127; memcpy(req + 1, token, n); req_len += n; }
    free(token);
    size_t n = 0;
    int32_t status = -1;
    bool ok = bridge_call_fresh(0xFFFF, SLOT_GAMESERVER, req, req_len, &status, reply, sizeof reply, &n, 90000) && status == 0 && n >= 12;
    if (ok) {
        AcquireSRWLockExclusive(&g_lock);
        memcpy(&g.steamid, reply, 8);
        memcpy(&g.public_ip, reply + 8, 4);
        g.logged_on = true;
        ReleaseSRWLockExclusive(&g_lock);
        shim_log("game server logged in as %llu", (unsigned long long)g.steamid);
        push_server_state();
        HANDLE hb = CreateThread(NULL, 0, heartbeat_thread, NULL, 0, NULL);
        if (hb) CloseHandle(hb);
    }
    post_connected(ok, 3);
    return 0;
}

static void start_logon(const char *token) {
    bool already;
    AcquireSRWLockExclusive(&g_lock);
    already = g.logged_on;
    ReleaseSRWLockExclusive(&g_lock);
    if (already) { post_connected(true, 1); return; }
    char *copy = token && *token ? _strdup(token) : NULL;
    HANDLE t = CreateThread(NULL, 0, logon_thread, copy, 0, NULL);
    if (t) CloseHandle(t); else { free(copy); post_connected(false, 3); }
}
static void SHIM_TC gs_logon(void *self) { (void)self; start_logon(NULL); }
static void SHIM_TC gs_logon_token(void *self, const char *token) { (void)self; start_logon(token); }
static void SHIM_TC gs_logon_account(void *self, const char *account, const char *password) { (void)self; (void)account; (void)password; start_logon(NULL); }
static void SHIM_TC gs_logoff(void *self) {
    (void)self;
    bool was;
    AcquireSRWLockExclusive(&g_lock);
    was = g.logged_on;
    g.logged_on = false;
    ReleaseSRWLockExclusive(&g_lock);
    if (!was) return;
    uint8_t req[1] = { OP_LOGOFF };
    int32_t status;
    bridge_call_fresh(0xFFFF, SLOT_GAMESERVER, req, sizeof req, &status, NULL, 0, NULL, 5000);
    int32_t ok = 1;
    shim_post_callback(103, &ok, 4);
}
static uint64_t SHIM_TC gs_logged_on(void *self) { (void)self; return g.logged_on; }
static uint64_t *SHIM_TC gs_steamid(void *self, uint64_t *ret) { (void)self; *ret = g.logged_on ? g.steamid : 0; return ret; }
static uint32_t SHIM_TC gs_public_ip_v4(void *self) { (void)self; return g.public_ip; }
static void *SHIM_TC gs_public_ip(void *self, uint8_t *ret) { (void)self; memset(ret, 0, 20); memcpy(ret, &g.public_ip, 4); return ret; }
static uint64_t *SHIM_TC gs_create_unauth(void *self, uint64_t *ret, uint32_t ip) {
    (void)self; (void)ip;
    *ret = ((uint64_t)1 << 56) | ((uint64_t)1 << 52) | ((uint64_t)1 << 32) | (0x7F000000u + ++g.unauth_counter);
    return ret;
}

static uint64_t SHIM_TC gs_update_user(void *self, uint64_t id, const char *name, uint32_t score) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    int at = -1;
    for (int i = 0; i < g.n_players; i++) if (g.players[i].id == id) at = i;
    if (at < 0 && g.n_players < MAX_PLAYERS) { at = g.n_players++; g.players[at].since = GetTickCount(); }
    if (at >= 0) { g.players[at].id = id; snprintf(g.players[at].name, sizeof g.players[at].name, "%s", name ? name : ""); g.players[at].score = (int32_t)score; }
    ReleaseSRWLockExclusive(&g_lock);
    return at >= 0;
}

static void SHIM_TC gs_send_user_disconnect(void *self, uint64_t id) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < g.n_players; i++) if (g.players[i].id == id) { g.players[i] = g.players[--g.n_players]; break; }
    ReleaseSRWLockExclusive(&g_lock);
}

static void remember_session(uint64_t id, uint32_t appid) {
    AcquireSRWLockExclusive(&g_lock);
    int free_slot = -1, at = -1;
    for (int i = 0; i < MAX_SESSIONS; i++) { if (g.sessions[i].id == id) at = i; if (!g.sessions[i].id && free_slot < 0) free_slot = i; }
    if (at < 0) at = free_slot;
    if (at >= 0) g.sessions[at] = (Session){ id, appid };
    ReleaseSRWLockExclusive(&g_lock);
}

static void SHIM_TC gs_end_auth_session(void *self, uint64_t id) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < MAX_SESSIONS; i++) if (g.sessions[i].id == id) g.sessions[i].id = 0;
    ReleaseSRWLockExclusive(&g_lock);
}

static uint32_t challenge_for(uint32_t ip, uint16_t port) {
    static uint32_t secret;
    if (!secret) secret = (uint32_t)GetTickCount() * 2654435761u ^ 0x5a17c0deu;
    return (ip * 2246822519u) ^ ((uint32_t)port * 3266489917u) ^ secret;
}

static void queue_packet(uint32_t ip, uint16_t port, const uint8_t *data, int len) {
    if (len <= 0 || len > PACKET_MAX) return;
    if (g.out_count == MAX_PACKETS) { g.out_head = (g.out_head + 1) % MAX_PACKETS; g.out_count--; }
    Packet *p = &g.out[(g.out_head + g.out_count++) % MAX_PACKETS];
    memcpy(p->data, data, (size_t)len);
    p->len = len; p->ip = ip; p->port = port;
}

static int put_str(uint8_t *o, const char *s) { size_t n = strlen(s) + 1; memcpy(o, s, n); return (int)n; }

static int build_info(uint8_t *o) {
    int n = 0;
    memset(o, 0xFF, 4); n = 4;
    o[n++] = 'I';
    o[n++] = 17;
    n += put_str(o + n, g.name);
    n += put_str(o + n, g.map);
    n += put_str(o + n, g.mod_dir);
    n += put_str(o + n, g.desc);
    uint16_t app16 = g.appid < 0x8000 ? (uint16_t)g.appid : 0;
    memcpy(o + n, &app16, 2); n += 2;
    o[n++] = (uint8_t)g.n_players;
    o[n++] = (uint8_t)g.max_players;
    o[n++] = (uint8_t)g.bots;
    o[n++] = g.dedicated ? 'd' : 'l';
    o[n++] = 'w';
    o[n++] = g.password ? 1 : 0;
    o[n++] = 0;
    n += put_str(o + n, g.version);
    o[n++] = 0x80 | 0x10 | 0x20 | 0x01;
    memcpy(o + n, &g.game_port, 2); n += 2;
    uint64_t sid = g.logged_on ? g.steamid : 0;
    memcpy(o + n, &sid, 8); n += 8;
    n += put_str(o + n, g.tags);
    uint64_t gameid = g.appid;
    memcpy(o + n, &gameid, 8); n += 8;
    return n;
}

static bool SHIM_TC gs_handle_packet(void *self, const uint8_t *data, int cb, uint32_t ip, uint16_t port) {
    (void)self;
    if (!data || cb < 5 || memcmp(data, "\xff\xff\xff\xff", 4)) return false;
    uint8_t out[PACKET_MAX];
    int n = 0;
    uint8_t kind = data[4];
    AcquireSRWLockExclusive(&g_lock);
    bool handled = true;
    uint32_t want = challenge_for(ip, port), got = cb >= 9 ? (uint32_t)(data[5] | data[6] << 8 | data[7] << 16 | (uint32_t)data[8] << 24) : 0xFFFFFFFFu;
    if (kind == 'T' && cb >= 5 + 20 && !memcmp(data + 5, "Source Engine Query", 19)) {
        n = build_info(out);
    } else if (kind == 'W' || ((kind == 'U' || kind == 'V') && got != want)) {
        memset(out, 0xFF, 4); out[4] = 'A'; memcpy(out + 5, &want, 4); n = 9;
    } else if (kind == 'U') {
        memset(out, 0xFF, 4); n = 4; out[n++] = 'D'; out[n++] = (uint8_t)g.n_players;
        DWORD now = GetTickCount();
        for (int i = 0; i < g.n_players && n + 80 < PACKET_MAX; i++) {
            float secs = (float)(now - g.players[i].since) / 1000.0f;
            out[n++] = (uint8_t)i;
            n += put_str(out + n, g.players[i].name);
            memcpy(out + n, &g.players[i].score, 4); n += 4;
            memcpy(out + n, &secs, 4); n += 4;
        }
    } else if (kind == 'V') {
        memset(out, 0xFF, 4); n = 4; out[n++] = 'E';
        uint16_t count = 0;
        int count_at = n;
        n += 2;
        for (int i = 0; i < g.n_rules && n + 330 < PACKET_MAX; i++) { n += put_str(out + n, g.rules[i].key); n += put_str(out + n, g.rules[i].value); count++; }
        memcpy(out + count_at, &count, 2);
    } else {
        handled = false;
    }
    if (handled && n > 0) queue_packet(ip, port, out, n);
    ReleaseSRWLockExclusive(&g_lock);
    return handled;
}

static int SHIM_TC gs_next_packet(void *self, uint8_t *out, int cap, uint32_t *ip, uint16_t *port) {
    (void)self;
    int n = 0;
    AcquireSRWLockExclusive(&g_lock);
    if (g.out_count && out) {
        Packet *p = &g.out[g.out_head];
        if (p->len <= cap) {
            memcpy(out, p->data, (size_t)p->len);
            n = p->len;
            if (ip) *ip = p->ip;
            if (port) *port = p->port;
        }
        g.out_head = (g.out_head + 1) % MAX_PACKETS;
        g.out_count--;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return n;
}

static uint64_t SHIM_TC send_user_connect_and_authenticate(void *self, uint32_t ip, const void *blob, uint32_t cb, uint64_t *out_id) {
    (void)self; (void)ip;
    uint64_t owner = 0;
    const Snapshot *s = shim_snapshot();
    int response = shim_verify_auth_ticket(blob, cb, s && s->has_ticket ? s->ticket.appid : 0, 0, &owner);
    if (response == 0 && owner) {
        if (out_id) *out_id = owner;
        uint8_t ok[16];
        memcpy(ok, &owner, 8);
        memcpy(ok + 8, &owner, 8);
        shim_post_callback(201, ok, sizeof ok);
        return 1;
    }
    if (out_id) *out_id = 0;
    uint8_t deny[144] = {0};
    int32_t reason = response == 2 ? 4 : 2;
    memcpy(deny, &owner, 8);
    memcpy(deny + 8, &reason, 4);
    strcpy((char *)deny + 12, response == 2 ? "no license for this game or ticket expired" : "invalid ticket (Steam signature does not match)");
    shim_post_callback(202, deny, sizeof deny);
    return 0;
}
static int SHIM_TC gs_begin_auth_session(void *self, const void *ticket, int cb, uint64_t id) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    uint64_t owner = 0;
    int response = cb > 0 ? shim_verify_auth_ticket(ticket, (uint32_t)cb, s && s->has_ticket ? s->ticket.appid : 0, id, &owner) : 8;
    uint8_t resp[24] = {0};
    memcpy(resp, &id, 8);
    memcpy(resp + 8, &response, 4);
    memcpy(resp + 16, &owner, 8);
    shim_post_callback(143, resp, sizeof resp);
    if (response == 0) remember_session(id, s && s->has_ticket ? s->ticket.appid : 0);
    return response == 0 ? 0 : 1;
}

static uint32_t SHIM_TC gs_has_license(void *self, uint64_t id, uint32_t appid) {
    (void)self;
    uint32_t result = 2;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < MAX_SESSIONS; i++) if (g.sessions[i].id == id && g.sessions[i].appid == appid) result = 0;
    ReleaseSRWLockExclusive(&g_lock);
    return result;
}

static uint32_t SHIM_TC gs_invalid_state(void *self) { (void)self; return 9; }
static uint32_t SHIM_TC gs_invalid_param(void *self) { (void)self; return 8; }

void register_gameserver(void) {
    const char *p = "SteamGameServer0";
    shim_register_family(p, "BSetServerType", (void *)gs_true);
    shim_register_family_ab(p, "InitGameServer", 24, (void *)gs_init);
    shim_register_family_ab(p, "LogOn", 0, (void *)gs_logon);
    shim_register_family_ab(p, "LogOn", 4, (void *)gs_logon_token);
    shim_register_family_ab(p, "LogOn", 8, (void *)gs_logon_account);
    shim_register_family(p, "LogOnAnonymous", (void *)gs_logon);
    shim_register_family(p, "LogOff", (void *)gs_logoff);
    shim_register_family(p, "BLoggedOn", (void *)gs_logged_on);
    shim_register_family(p, "GetSteamID", (void *)gs_steamid);
    shim_register_family_ab(p, "GetPublicIP", 0, (void *)gs_public_ip_v4);
    shim_register_family_ab(p, "GetPublicIP", 4, (void *)gs_public_ip);
    shim_register_family(p, "CreateUnauthenticatedUserConnection", (void *)gs_create_unauth);
    shim_register_family(p, "BUpdateUserData", (void *)gs_update_user);
    shim_register_family(p, "EndAuthSession", (void *)gs_end_auth_session);
    shim_register_family(p, "HandleIncomingPacket", (void *)gs_handle_packet);
    shim_register_family(p, "GetNextOutgoingPacket", (void *)gs_next_packet);
    shim_register_family(p, "SetProduct", (void *)gs_set_product);
    shim_register_family(p, "SetGameDescription", (void *)gs_set_desc);
    shim_register_family(p, "SetModDir", (void *)gs_set_moddir);
    shim_register_family(p, "SetDedicatedServer", (void *)gs_set_dedicated);
    shim_register_family(p, "SetMaxPlayerCount", (void *)gs_set_max);
    shim_register_family(p, "SetBotPlayerCount", (void *)gs_set_bots);
    shim_register_family(p, "SetServerName", (void *)gs_set_name);
    shim_register_family(p, "SetMapName", (void *)gs_set_map);
    shim_register_family(p, "SetPasswordProtected", (void *)gs_set_password);
    shim_register_family(p, "SetSpectatorPort", (void *)gs_set_spec_port);
    shim_register_family(p, "SetSpectatorServerName", (void *)gs_set_spec_name);
    shim_register_family(p, "ClearAllKeyValues", (void *)gs_clear_rules);
    shim_register_family(p, "SetKeyValue", (void *)gs_set_rule);
    shim_register_family(p, "SetGameTags", (void *)gs_set_tags);
    shim_register_family(p, "SetGameData", (void *)gs_set_gamedata);
    shim_register_family(p, "SetRegion", (void *)gs_set_region);
    shim_register_family(p, "EnableHeartbeats", (void *)gs_void);
    shim_register_family(p, "SetHeartbeatInterval", (void *)gs_void);
    shim_register_family(p, "ForceHeartbeat", (void *)gs_force_heartbeat);
    shim_register_family(p, "BSecure", (void *)gs_false);
    shim_register_family(p, "WasRestartRequested", (void *)gs_false);
    shim_register_family(p, "SendUserConnectAndAuthenticate", (void *)send_user_connect_and_authenticate);
    shim_register_family(p, "SendUserConnectAndAuthenticate_DEPRECATED", (void *)send_user_connect_and_authenticate);
    shim_register_family(p, "BeginAuthSession", (void *)gs_begin_auth_session);
    shim_register_family(p, "GSSetServerType", (void *)gs_true);
    shim_register_family(p, "GSSetServerType2", (void *)gs_true);
    shim_register_family(p, "GSUpdateStatus", (void *)gs_true);
    shim_register_family(p, "GSUpdateStatus2", (void *)gs_true);
    shim_register_family(p, "SendUserDisconnect", (void *)gs_send_user_disconnect);
    shim_register_family(p, "SendUserDisconnect_DEPRECATED", (void *)gs_send_user_disconnect);
    shim_register_family(p, "UserHasLicenseForApp", (void *)gs_has_license);
    static const char *const stats[] = { "CommitOutstandingRows", "CommitRow", "EndSession" };
    for (size_t i = 0; i < sizeof stats / sizeof *stats; i++) shim_register_family("SteamGameStats", stats[i], (void *)gs_true);
    shim_register_family("SteamGameStats", "AddNewRow", (void *)gs_invalid_state);
    static const char *const attrs[] = { "AddRowAtributeString", "AddRowAttributeFloat", "AddRowAttributeInt", "AddRowAttributeInt64",
        "AddSessionAttributeFloat", "AddSessionAttributeInt", "AddSessionAttributeInt64", "AddSessionAttributeString" };
    for (size_t i = 0; i < sizeof attrs / sizeof *attrs; i++) shim_register_family("SteamGameStats", attrs[i], (void *)gs_invalid_param);
}

static int g_gs_token;
static struct { uint32_t user_id; uint64_t steamid; } g_legacy_users[MAX_PLAYERS];
void *shim_gs_handle(void) { return &g_gs_token; }
uint64_t shim_gs_logged_on(void) { return g.logged_on; }
uint64_t shim_gs_steamid(void) { return g.logged_on ? g.steamid : 0; }
void shim_gs_logon(void) { start_logon(NULL); }
void shim_gs_logoff(void) { gs_logoff(NULL); }
uint64_t shim_gs_secure(void) { return 0; }
uint64_t shim_gs_set_server_type(int32_t appid, uint32_t flags, uint32_t ip, uint16_t port, const char *game_dir, const char *version) {
    if (appid < 0) return 0;
    gs_init(NULL, ip, port, port, flags, (uint32_t)appid, version);
    if (game_dir) set_text(g.mod_dir, sizeof g.mod_dir, game_dir);
    return 1;
}
void shim_gs_set_spawn_count(uint32_t count) {
    static volatile LONG last = -1;
    if ((LONG)count == InterlockedExchange(&last, (LONG)count)) return;
    if (g.logged_on) push_server_state();
}
uint64_t shim_gs_update_status(int players, int max_players, int bots, const char *name, const char *map) {
    (void)players;
    g.max_players = max_players;
    g.bots = bots;
    if (name) set_text(g.name, sizeof g.name, name);
    if (map) set_text(g.map, sizeof g.map, map);
    return 1;
}
uint64_t shim_gs_user_connect(uint32_t user_id, uint32_t ip, const void *cookie, uint32_t cb) {
    uint64_t id = 0;
    if (!send_user_connect_and_authenticate(NULL, ip, cookie, cb, &id) || !id) return 0;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < MAX_PLAYERS; i++) if (!g_legacy_users[i].steamid || g_legacy_users[i].user_id == user_id) { g_legacy_users[i].user_id = user_id; g_legacy_users[i].steamid = id; break; }
    ReleaseSRWLockExclusive(&g_lock);
    return 1;
}
uint64_t shim_gs_user_remove(uint32_t user_id) {
    uint64_t id = 0;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < MAX_PLAYERS; i++) if (g_legacy_users[i].steamid && g_legacy_users[i].user_id == user_id) { id = g_legacy_users[i].steamid; g_legacy_users[i].steamid = 0; break; }
    ReleaseSRWLockExclusive(&g_lock);
    if (!id) return 0;
    gs_send_user_disconnect(NULL, id);
    gs_end_auth_session(NULL, id);
    return 1;
}
