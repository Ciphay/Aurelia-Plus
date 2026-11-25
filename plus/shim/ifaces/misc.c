#include <string.h>

#include "../shim.h"

#define RP_MAX 20
static char g_rp_key[RP_MAX][65], g_rp_val[RP_MAX][257];
static int g_rp_n;

static void rp_push(void) {
    uint8_t payload[2 + RP_MAX * (4 + 64 + 256)];
    size_t o = 0;
    payload[o++] = (uint8_t)g_rp_n;
    payload[o++] = 0;
    for (int i = 0; i < g_rp_n; i++) {
        size_t kl = strlen(g_rp_key[i]), vl = strlen(g_rp_val[i]);
        payload[o++] = (uint8_t)kl; payload[o++] = (uint8_t)(kl >> 8);
        memcpy(payload + o, g_rp_key[i], kl); o += kl;
        payload[o++] = (uint8_t)vl; payload[o++] = (uint8_t)(vl >> 8);
        memcpy(payload + o, g_rp_val[i], vl); o += vl;
    }
    int32_t status = -99;
    if (!bridge_call(0xFFFF, 6, payload, o, &status, NULL, 0, NULL, 4000) || status != 0)
        shim_log("rich presence did not reach Steam (status=%d)", status);
}

static uint64_t SHIM_TC set_rich_presence(void *self, const char *key, const char *value) {
    (void)self;
    shim_trace("SetRichPresence %s=%s", key ? key : "", value ? value : "");
    if (!key || !key[0] || strlen(key) > 64 || (value && strlen(value) > 256)) return 0;
    int i = 0;
    while (i < g_rp_n && strcmp(g_rp_key[i], key)) i++;
    if (!value || !value[0]) {
        if (i == g_rp_n) return 1;
        memmove(g_rp_key[i], g_rp_key[i + 1], (size_t)(g_rp_n - i - 1) * sizeof g_rp_key[0]);
        memmove(g_rp_val[i], g_rp_val[i + 1], (size_t)(g_rp_n - i - 1) * sizeof g_rp_val[0]);
        g_rp_n--;
    } else {
        if (i == g_rp_n) { if (g_rp_n == RP_MAX) return 0; g_rp_n++; strcpy(g_rp_key[i], key); }
        strcpy(g_rp_val[i], value);
    }
    rp_push();
    return 1;
}
static void SHIM_TC clear_rich_presence(void *self) { (void)self; g_rp_n = 0; rp_push(); }
static bool is_self_id(uint64_t id) { const Snapshot *s = shim_snapshot(); return s && s->steamid == id; }
static const char *SHIM_TC friend_rich_presence(void *self, uint64_t id, const char *key) {
    (void)self;
    for (int i = 0; is_self_id(id) && key && i < g_rp_n; i++) if (!strcmp(g_rp_key[i], key)) return g_rp_val[i];
    return "";
}
static int SHIM_TC friend_rich_presence_key_count(void *self, uint64_t id) { (void)self; return is_self_id(id) ? g_rp_n : 0; }
static const char *SHIM_TC friend_rich_presence_key(void *self, uint64_t id, int i) { (void)self; return is_self_id(id) && i >= 0 && i < g_rp_n ? g_rp_key[i] : ""; }

static const ShimFriend *find_friend(uint64_t id) {
    const Snapshot *s = shim_snapshot();
    for (size_t i = 0; s && i < s->n_friends; i++) if (s->friends[i].steamid == id) return &s->friends[i];
    return NULL;
}
static bool is_self(uint64_t id) { const Snapshot *s = shim_snapshot(); return s && s->steamid == id; }

static const char *SHIM_TC persona_name(void *self) { (void)self; const Snapshot *s = shim_snapshot(); return s ? s->persona : ""; }
static int SHIM_TC persona_state(void *self) { (void)self; return 1; }
static int SHIM_TC get_friend_count(void *self, int flags) { (void)self; const Snapshot *s = shim_snapshot(); return s && (flags & 4) ? (int)s->n_friends : 0; }
static uint64_t *SHIM_TC get_friend_by_index(void *self, uint64_t *ret, int i, int flags) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    *ret = s && (flags & 4) && i >= 0 && (size_t)i < s->n_friends ? s->friends[i].steamid : 0;
    return ret;
}
static int SHIM_TC friend_relationship(void *self, uint64_t id) { (void)self; return find_friend(id) ? 3 : 0; }
static int SHIM_TC friend_persona_state(void *self, uint64_t id) { (void)self; const ShimFriend *f = find_friend(id); return f ? (int)f->state : is_self(id) ? 1 : 0; }
static const char *SHIM_TC friend_persona_name(void *self, uint64_t id) {
    (void)self;
    const ShimFriend *f = find_friend(id);
    if (f) return f->persona ? f->persona : "";
    if (!id) return "";
    return is_self(id) ? persona_name(NULL) : "[unknown]";
}
static uint64_t SHIM_TC friend_game_played(void *self, uint64_t id, uint8_t *info) {
    (void)self;
    const ShimFriend *f = find_friend(id);
    if (!f || !f->game_appid) return 0;
    if (info) { memset(info, 0, 24); uint64_t g = f->game_appid; memcpy(info, &g, 8); }
    return 1;
}
static uint64_t friend_game_old(const ShimFriend *f, uint64_t *gameid, uint32_t *ip, uint16_t *port, uint16_t *query_port) {
    if (!f || !f->game_appid) return 0;
    if (gameid) *gameid = f->game_appid;
    if (ip) *ip = 0;
    if (port) *port = 0;
    if (query_port) *query_port = 0;
    return 1;
}
static uint64_t SHIM_TC friend_game_played_deprecated(void *self, uint64_t id, uint64_t *gameid, uint32_t *ip, uint16_t *port) {
    (void)self;
    return friend_game_old(find_friend(id), gameid, ip, port, NULL);
}
static uint64_t SHIM_TC friend_game_played2(void *self, uint64_t id, uint64_t *gameid, uint32_t *ip, uint16_t *port, uint16_t *query_port) {
    (void)self;
    return friend_game_old(find_friend(id), gameid, ip, port, query_port);
}

static uint64_t SHIM_TC has_friend(void *self, uint64_t id, int flags) { (void)self; return (flags & 4) && find_friend(id) != NULL; }
static uint64_t SHIM_TC request_user_information(void *self, uint64_t id, uint8_t name_only) { (void)self; (void)id; (void)name_only; return 0; }

static int SHIM_TC clan_count(void *self) { (void)self; return 0; }
static uint64_t *SHIM_TC clan_by_index(void *self, uint64_t *ret, int i) { (void)self; (void)i; *ret = 0; return ret; }
static const char *SHIM_TC clan_name(void *self, uint64_t id) { (void)self; (void)id; return ""; }

static float SHIM_TC music_volume(void *self) { (void)self; return 1.0f; }

static void SHIM_TC request_friend_rich_presence(void *self, uint64_t steamid) {
    (void)self;
    uint8_t resp[12];
    uint32_t appid = shim_process_appid();
    memcpy(resp, &steamid, 8);
    memcpy(resp + 8, &appid, 4);
    shim_post_callback(336, resp, sizeof resp);
}

static uint64_t SHIM_TC set_persona_name(void *self, const char *name) {
    (void)self; (void)name;
    uint8_t resp[8] = {0};
    int32_t denied = 15;
    memcpy(resp + 4, &denied, 4);
    uint64_t call = shim_callresult_reserve();
    shim_callresult_complete(call, 347, resp, sizeof resp);
    return call;
}
void register_misc(void) {
    shim_register_family("SteamFriends", "SetPersonaName", (void *)set_persona_name);
    shim_register_family("STEAMMUSIC_INTERFACE", "GetVolume", (void *)music_volume);
    shim_register_family("SteamFriends", "GetClanCount", (void *)clan_count);
    shim_register_family("SteamFriends", "GetClanByIndex", (void *)clan_by_index);
    shim_register_family("SteamFriends", "GetClanName", (void *)clan_name);
    shim_register_family("SteamFriends", "SetRichPresence", (void *)set_rich_presence);
    shim_register_family("SteamFriends", "RequestFriendRichPresence", (void *)request_friend_rich_presence);
    shim_register_family("SteamFriends", "ClearRichPresence", (void *)clear_rich_presence);
    shim_register_family("SteamFriends", "GetFriendRichPresence", (void *)friend_rich_presence);
    shim_register_family("SteamFriends", "GetFriendRichPresenceKeyCount", (void *)friend_rich_presence_key_count);
    shim_register_family("SteamFriends", "GetFriendRichPresenceKeyByIndex", (void *)friend_rich_presence_key);
    shim_register_family("SteamFriends", "GetFriendCount", (void *)get_friend_count);
    shim_register_family("SteamFriends", "GetPersonaName", (void *)persona_name);
    shim_register_family("SteamFriends", "GetPersonaState", (void *)persona_state);
    shim_register_family("SteamFriends", "GetFriendByIndex", (void *)get_friend_by_index);
    shim_register_family("SteamFriends", "GetFriendRelationship", (void *)friend_relationship);
    shim_register_family("SteamFriends", "GetFriendPersonaState", (void *)friend_persona_state);
    shim_register_family("SteamFriends", "GetFriendPersonaName", (void *)friend_persona_name);
    shim_register_family("SteamFriends", "GetFriendGamePlayed", (void *)friend_game_played);
    shim_register_family("SteamFriends", "HasFriend", (void *)has_friend);
    shim_register_family("SteamFriends", "Deprecated_GetFriendGamePlayed", (void *)friend_game_played_deprecated);
    shim_register_family("SteamFriends", "GetFriendGamePlayed2", (void *)friend_game_played2);
    shim_register_family("SteamFriends", "RequestUserInformation", (void *)request_user_information);
}
