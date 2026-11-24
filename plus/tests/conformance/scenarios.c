#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "probe_table.h"

typedef void *(*CreateInterfaceFn)(const char *, int *);
typedef uint64_t (*Fn8)(void *, uint64_t, uint64_t, uint64_t, uint64_t, uint64_t, uint64_t, uint64_t);

static FILE *g_out;
static void *g_client;
static uint32_t g_pipe, g_user;

static const ProbeIface *iface_of(const char *name) {
    for (int i = 0; i < PROBE_COUNT; i++) if (!strcmp(g_probe[i].iface, name)) return &g_probe[i];
    return NULL;
}

static void *slot_fn(void *obj, const char *iface, const char *method, int *hidden) {
    const ProbeIface *pi = iface_of(iface);
    if (!pi) return NULL;
    for (int k = 0; k < pi->n; k++) if (!strcmp(pi->methods[k], method)) { *hidden = pi->hidden[k]; return (*(void ***)obj)[k]; }
    return NULL;
}

typedef struct { void *obj; const char *iface; } Obj;

static Obj get(const char *iface) {
    Obj o = { NULL, iface };
    int hidden;
    void *fn = slot_fn(g_client, "SteamClient023", "GetISteamGenericInterface", &hidden);
    o.obj = ((void *(*)(void *, uint32_t, uint32_t, const char *))fn)(g_client, g_user, g_pipe, iface);
    return o;
}

static uint64_t call(Obj o, const char *method, uint64_t a, uint64_t b, uint64_t c, uint64_t d, uint64_t e) {
    int hidden = 0;
    if (!o.obj) return ~0ULL;
    void *fn = slot_fn(o.obj, o.iface, method, &hidden);
    if (!fn) return ~0ULL - 1;
    uint64_t scratch[8] = {0};
    if (hidden) {
        ((Fn8)fn)(o.obj, (uint64_t)(uintptr_t)scratch, a, b, c, d, e, 0);
        return scratch[0];
    }
    return ((Fn8)fn)(o.obj, a, b, c, d, e, 0, 0);
}

static void emit(const char *label, uint64_t v) { fprintf(g_out, "S %s %llx\n", label, (unsigned long long)v); fflush(g_out); }
static void emit_str(const char *label, const char *s) { fprintf(g_out, "S %s %s\n", label, s ? s : "(null)"); fflush(g_out); }
static void emit_bytes(const char *label, const void *p, size_t n) {
    fprintf(g_out, "S %s ", label);
    for (size_t i = 0; i < n; i++) fprintf(g_out, "%02x", ((const uint8_t *)p)[i]);
    fprintf(g_out, "\n");
    fflush(g_out);
}
#define B(v) ((v) & 0xff)
#define P(x) ((uint64_t)(uintptr_t)(x))

static void apps(void) {
    Obj o = get("STEAMAPPS_INTERFACE_VERSION008");
    emit("apps.iface", o.obj != NULL);
    emit("apps.BIsSubscribed", B(call(o, "BIsSubscribed", 0, 0, 0, 0, 0)));
    emit("apps.BIsSubscribedApp.480", B(call(o, "BIsSubscribedApp", 480, 0, 0, 0, 0)));
    emit("apps.BIsSubscribedApp.440", B(call(o, "BIsSubscribedApp", 440, 0, 0, 0, 0)));
    emit("apps.BIsSubscribedApp.0", B(call(o, "BIsSubscribedApp", 0, 0, 0, 0, 0)));
    emit("apps.BIsLowViolence", B(call(o, "BIsLowViolence", 0, 0, 0, 0, 0)));
    emit("apps.BIsCybercafe", B(call(o, "BIsCybercafe", 0, 0, 0, 0, 0)));
    emit("apps.BIsVACBanned", B(call(o, "BIsVACBanned", 0, 0, 0, 0, 0)));
    emit_str("apps.GetCurrentGameLanguage", (const char *)call(o, "GetCurrentGameLanguage", 0, 0, 0, 0, 0));
    emit_str("apps.GetAvailableGameLanguages", (const char *)call(o, "GetAvailableGameLanguages", 0, 0, 0, 0, 0));
    emit("apps.BIsDlcInstalled.480", B(call(o, "BIsDlcInstalled", 480, 0, 0, 0, 0)));
    emit("apps.GetDLCCount", (uint32_t)call(o, "GetDLCCount", 0, 0, 0, 0, 0));
    for (int i = 0; i < 128; i++) {
        uint32_t id = 0xdead; uint8_t avail = 0xee; char name[128] = "zz"; char lbl[64];
        uint64_t r = B(call(o, "BGetDLCDataByIndex", (uint64_t)i, P(&id), P(&avail), P(name), sizeof name));
        snprintf(lbl, sizeof lbl, "apps.BGetDLCDataByIndex.%d", i);
        fprintf(g_out, "S %s ret=%llx id=%x avail=%x name=%s\n", lbl, (unsigned long long)r, id, avail, name);
    }
    emit("apps.GetEarliestPurchaseUnixTime.480.nonzero", (uint32_t)call(o, "GetEarliestPurchaseUnixTime", 480, 0, 0, 0, 0) != 0);
    emit("apps.GetEarliestPurchaseUnixTime.440.nonzero", (uint32_t)call(o, "GetEarliestPurchaseUnixTime", 440, 0, 0, 0, 0) != 0);
    {   char beta[64] = "zz"; uint64_t r = B(call(o, "GetCurrentBetaName", P(beta), sizeof beta, 0, 0, 0)); fprintf(g_out, "S apps.GetCurrentBetaName ret=%llx name=%s\n", (unsigned long long)r, beta); }
    {   char dir[260] = "zz"; uint64_t r = (uint32_t)call(o, "GetAppInstallDir", 480, P(dir), sizeof dir, 0, 0); fprintf(g_out, "S apps.GetAppInstallDir.480 ret=%llx dir=%s\n", (unsigned long long)r, dir); }
    {   char dir[260] = "zz"; uint64_t r = (uint32_t)call(o, "GetAppInstallDir", 440, P(dir), sizeof dir, 0, 0); fprintf(g_out, "S apps.GetAppInstallDir.440 ret=%llx dir=%s\n", (unsigned long long)r, dir); }
    emit("apps.BIsAppInstalled.480", B(call(o, "BIsAppInstalled", 480, 0, 0, 0, 0)));
    emit("apps.BIsAppInstalled.550", B(call(o, "BIsAppInstalled", 550, 0, 0, 0, 0)));
    emit("apps.GetAppOwner", call(o, "GetAppOwner", 0, 0, 0, 0, 0));
    emit_str("apps.GetLaunchQueryParam.x", (const char *)call(o, "GetLaunchQueryParam", P("x"), 0, 0, 0, 0));
    emit("apps.GetAppBuildId.nonzero", (uint32_t)call(o, "GetAppBuildId", 0, 0, 0, 0, 0) != 0);
    {   char cmd[256] = "zz"; uint64_t r = (uint32_t)call(o, "GetLaunchCommandLine", P(cmd), sizeof cmd, 0, 0, 0); fprintf(g_out, "S apps.GetLaunchCommandLine ret=%llx cmd=%s\n", (unsigned long long)r, cmd); }
    {   uint32_t a = 0xdead, b = 0xdead; uint64_t r = B(call(o, "BIsTimedTrial", P(&a), P(&b), 0, 0, 0)); fprintf(g_out, "S apps.BIsTimedTrial ret=%llx a=%x b=%x\n", (unsigned long long)r, a, b); }
    {   uint32_t depots[16] = {0}; uint64_t n = (uint32_t)call(o, "GetInstalledDepots", 480, P(depots), 16, 0, 0); fprintf(g_out, "S apps.GetInstalledDepots.480 n=%llx first=%x\n", (unsigned long long)n, depots[0]); }
}

static void utils(void) {
    Obj o = get("SteamUtils010");
    emit("utils.iface", o.obj != NULL);
    emit("utils.GetAppID", (uint32_t)call(o, "GetAppID", 0, 0, 0, 0, 0));
    emit_str("utils.GetIPCountry", (const char *)call(o, "GetIPCountry", 0, 0, 0, 0, 0));
    emit("utils.GetConnectedUniverse", (uint32_t)call(o, "GetConnectedUniverse", 0, 0, 0, 0, 0));
    emit("~utils.GetServerRealTime", (uint32_t)call(o, "GetServerRealTime", 0, 0, 0, 0, 0) != 0);
    emit("~utils.GetSecondsSinceAppActive", (uint32_t)call(o, "GetSecondsSinceAppActive", 0, 0, 0, 0, 0) < 600);
    emit("utils.GetCurrentBatteryPower", B(call(o, "GetCurrentBatteryPower", 0, 0, 0, 0, 0)));
    emit("utils.GetSteamUILanguage", (uint64_t)(uintptr_t)0 + (call(o, "GetSteamUILanguage", 0, 0, 0, 0, 0) != 0));
    emit_str("utils.GetSteamUILanguage.text", (const char *)call(o, "GetSteamUILanguage", 0, 0, 0, 0, 0));
    emit("utils.IsSteamRunningInVR", B(call(o, "IsSteamRunningInVR", 0, 0, 0, 0, 0)));
    emit("utils.IsSteamInBigPictureMode", B(call(o, "IsSteamInBigPictureMode", 0, 0, 0, 0, 0)));
    emit("utils.IsSteamChinaLauncher", B(call(o, "IsSteamChinaLauncher", 0, 0, 0, 0, 0)));
    emit("utils.IsSteamRunningOnSteamDeck", B(call(o, "IsSteamRunningOnSteamDeck", 0, 0, 0, 0, 0)));
    emit("utils.GetEnteredGamepadTextLength", (uint32_t)call(o, "GetEnteredGamepadTextLength", 0, 0, 0, 0, 0));
    emit("utils.IsAPICallCompleted.0", B(call(o, "IsAPICallCompleted", 0, P(&(uint8_t){0}), 0, 0, 0)));
    emit("utils.GetAPICallFailureReason.0", (uint32_t)call(o, "GetAPICallFailureReason", 0, 0, 0, 0, 0));
    {   int32_t w = 0x7e, h = 0x7e; uint64_t r = B(call(o, "GetImageSize", 0, P(&w), P(&h), 0, 0)); fprintf(g_out, "S utils.GetImageSize.0 ret=%llx w=%x h=%x\n", (unsigned long long)r, w, h); }
    emit("utils.GetIPv6ConnectivityState.0", (uint32_t)call(o, "GetIPv6ConnectivityState", 0, 0, 0, 0, 0));
    {   char out[64] = "zz"; uint64_t r = (uint32_t)call(o, "FilterText", 1, 0, P("hello world"), P(out), sizeof out); fprintf(g_out, "S utils.FilterText ret=%llx out=%s\n", (unsigned long long)r, out); }
}

static void user(void) {
    Obj o = get("SteamUser023");
    emit("user.iface", o.obj != NULL);
    emit("user.BLoggedOn", B(call(o, "BLoggedOn", 0, 0, 0, 0, 0)));
    emit("user.GetSteamID", call(o, "GetSteamID", 0, 0, 0, 0, 0));
    emit("user.GetPlayerSteamLevel", (uint32_t)call(o, "GetPlayerSteamLevel", 0, 0, 0, 0, 0));
    emit("user.GetGameBadgeLevel.1.0", (uint32_t)call(o, "GetGameBadgeLevel", 1, 0, 0, 0, 0));
    emit("user.GetGameBadgeLevel.480.0", (uint32_t)call(o, "GetGameBadgeLevel", 480, 0, 0, 0, 0));
    emit("user.BIsBehindNAT", B(call(o, "BIsBehindNAT", 0, 0, 0, 0, 0)));
    emit("user.BIsPhoneVerified", B(call(o, "BIsPhoneVerified", 0, 0, 0, 0, 0)));
    emit("user.BIsTwoFactorEnabled", B(call(o, "BIsTwoFactorEnabled", 0, 0, 0, 0, 0)));
    emit("user.BIsPhoneIdentifying", B(call(o, "BIsPhoneIdentifying", 0, 0, 0, 0, 0)));
    emit("user.BIsPhoneRequiringVerification", B(call(o, "BIsPhoneRequiringVerification", 0, 0, 0, 0, 0)));
    emit("user.UserHasLicenseForApp.480", (uint32_t)call(o, "UserHasLicenseForApp", call(o, "GetSteamID", 0, 0, 0, 0, 0), 480, 0, 0, 0));
    emit("user.UserHasLicenseForApp.440", (uint32_t)call(o, "UserHasLicenseForApp", call(o, "GetSteamID", 0, 0, 0, 0, 0), 440, 0, 0, 0));
    emit("user.UserHasLicenseForApp.0", (uint32_t)call(o, "UserHasLicenseForApp", call(o, "GetSteamID", 0, 0, 0, 0, 0), 0, 0, 0, 0));
    emit("user.GetVoiceOptimalSampleRate", (uint32_t)call(o, "GetVoiceOptimalSampleRate", 0, 0, 0, 0, 0));
    {   char tick[1024]; uint32_t n = 0xdead; uint32_t h = (uint32_t)call(o, "GetAuthSessionTicket", P(tick), sizeof tick, P(&n), 0, 0); fprintf(g_out, "S ~user.GetAuthSessionTicket handle_nonzero=%d n_nonzero=%d\n", h != 0, n != 0 && n != 0xdead); call(o, "CancelAuthTicket", h, 0, 0, 0, 0); }
    {   uint32_t avail = 0xdead; uint64_t r = (uint32_t)call(o, "GetAvailableVoice", P(&avail), 0, 0, 0, 0); fprintf(g_out, "S user.GetAvailableVoice.idle ret=%llx avail=%x\n", (unsigned long long)r, avail); }
}

static void friends(void) {
    Obj o = get("SteamFriends017");
    emit("friends.iface", o.obj != NULL);
    emit_str("friends.GetPersonaName", (const char *)call(o, "GetPersonaName", 0, 0, 0, 0, 0));
    emit("friends.GetPersonaState", (uint32_t)call(o, "GetPersonaState", 0, 0, 0, 0, 0));
    uint32_t n4 = (uint32_t)call(o, "GetFriendCount", 4, 0, 0, 0, 0);
    emit("friends.GetFriendCount.4", n4);
    emit("friends.GetFriendCount.0", (uint32_t)call(o, "GetFriendCount", 0, 0, 0, 0, 0));
    emit("friends.GetFriendCount.1", (uint32_t)call(o, "GetFriendCount", 1, 0, 0, 0, 0));
    emit("friends.GetFriendCount.0xffff.eq4", (uint32_t)call(o, "GetFriendCount", 0xffff, 0, 0, 0, 0) == n4);
    emit("friends.GetFriendByIndex.out_of_range", call(o, "GetFriendByIndex", n4 + 5, 4, 0, 0, 0));
    uint64_t ids[3] = { 0, 0, 76561197960287930ULL };
    for (uint32_t i = 0; i < 2 && i < n4; i++) ids[i] = call(o, "GetFriendByIndex", i, 4, 0, 0, 0);
    emit("friends.sorted_ids_nonzero", ids[0] != 0 || n4 == 0);
    emit("friends.id.0", ids[0]);
    emit("friends.id.1", ids[1]);
    for (int i = 0; i < 3; i++) {
        if (!ids[i]) continue;
        char l[96];
        uint64_t id = ids[i];
        snprintf(l, sizeof l, "friends.%d.GetFriendRelationship", i); emit(l, (uint32_t)call(o, "GetFriendRelationship", id, 0, 0, 0, 0));
        snprintf(l, sizeof l, "friends.%d.GetFriendPersonaName", i); emit_str(l, (const char *)call(o, "GetFriendPersonaName", id, 0, 0, 0, 0));
        snprintf(l, sizeof l, "~friends.%d.GetFriendPersonaState", i); emit(l, (uint32_t)call(o, "GetFriendPersonaState", id, 0, 0, 0, 0));
        snprintf(l, sizeof l, "friends.%d.HasFriend.4", i); emit(l, B(call(o, "HasFriend", id, 4, 0, 0, 0)));
        snprintf(l, sizeof l, "friends.%d.HasFriend.1", i); emit(l, B(call(o, "HasFriend", id, 1, 0, 0, 0)));
        snprintf(l, sizeof l, "friends.%d.GetFriendSteamLevel", i); emit(l, (uint32_t)call(o, "GetFriendSteamLevel", id, 0, 0, 0, 0));
        snprintf(l, sizeof l, "friends.%d.GetPlayerNickname", i); emit_str(l, (const char *)call(o, "GetPlayerNickname", id, 0, 0, 0, 0));
        snprintf(l, sizeof l, "friends.%d.GetFriendRichPresence.status", i); emit_str(l, (const char *)call(o, "GetFriendRichPresence", id, P("status"), 0, 0, 0));
        snprintf(l, sizeof l, "friends.%d.GetFriendRichPresenceKeyCount.nonneg", i); emit(l, (int32_t)call(o, "GetFriendRichPresenceKeyCount", id, 0, 0, 0, 0) >= 0);
        snprintf(l, sizeof l, "friends.%d.GetFriendGamePlayed.shape", i);
        { uint8_t info[32]; memset(info, 0xee, sizeof info); uint64_t r = B(call(o, "GetFriendGamePlayed", id, P(info), 0, 0, 0)); fprintf(g_out, "S ~%s ret=%llx\n", l, (unsigned long long)r); }
        snprintf(l, sizeof l, "friends.%d.GetFriendCoplayTime", i); emit(l, (uint32_t)call(o, "GetFriendCoplayTime", id, 0, 0, 0, 0));
        snprintf(l, sizeof l, "friends.%d.GetFriendCoplayGame", i); emit(l, (uint32_t)call(o, "GetFriendCoplayGame", id, 0, 0, 0, 0));
    }
    emit("friends.GetClanCount", (uint32_t)call(o, "GetClanCount", 0, 0, 0, 0, 0));
    emit("friends.GetFriendsGroupCount", (uint32_t)call(o, "GetFriendsGroupCount", 0, 0, 0, 0, 0));
    emit("friends.GetUserRestrictions", (uint32_t)call(o, "GetUserRestrictions", 0, 0, 0, 0, 0));
    emit("friends.GetCoplayFriendCount", (uint32_t)call(o, "GetCoplayFriendCount", 0, 0, 0, 0, 0));
    emit("friends.GetFriendRelationship.self", (uint32_t)call(o, "GetFriendRelationship", call(get("SteamUser023"), "GetSteamID", 0, 0, 0, 0, 0), 0, 0, 0, 0));
    emit_str("friends.GetFriendPersonaName.self", (const char *)call(o, "GetFriendPersonaName", call(get("SteamUser023"), "GetSteamID", 0, 0, 0, 0, 0), 0, 0, 0, 0));
    emit("friends.GetFriendRelationship.0", (uint32_t)call(o, "GetFriendRelationship", 0, 0, 0, 0, 0));
    emit_str("friends.GetFriendPersonaName.0", (const char *)call(o, "GetFriendPersonaName", 0, 0, 0, 0, 0));
}

static void stats(void) {
    Obj o = get("STEAMUSERSTATS_INTERFACE_VERSION012");
    emit("stats.iface", o.obj != NULL);
    call(o, "RequestCurrentStats", 0, 0, 0, 0, 0);
    Sleep(4000);
    uint32_t n = (uint32_t)call(o, "GetNumAchievements", 0, 0, 0, 0, 0);
    emit("stats.GetNumAchievements", n);
    for (uint32_t i = 0; i < n && i < 6; i++) {
        const char *name = (const char *)call(o, "GetAchievementName", i, 0, 0, 0, 0);
        char l[160];
        snprintf(l, sizeof l, "stats.ach.%u.name", i); emit_str(l, name);
        if (!name) continue;
        { uint8_t got = 0xee; uint64_t r = B(call(o, "GetAchievement", P(name), P(&got), 0, 0, 0)); fprintf(g_out, "S stats.ach.%u.GetAchievement ret=%llx got=%x\n", i, (unsigned long long)r, got); }
        { uint8_t got = 0xee; uint32_t t = 0xdead; uint64_t r = B(call(o, "GetAchievementAndUnlockTime", P(name), P(&got), P(&t), 0, 0)); fprintf(g_out, "S stats.ach.%u.GetAchievementAndUnlockTime ret=%llx got=%x time=%x\n", i, (unsigned long long)r, got, t); }
        snprintf(l, sizeof l, "stats.ach.%u.display.name", i); emit_str(l, (const char *)call(o, "GetAchievementDisplayAttribute", P(name), P("name"), 0, 0, 0));
        snprintf(l, sizeof l, "stats.ach.%u.display.desc", i); emit_str(l, (const char *)call(o, "GetAchievementDisplayAttribute", P(name), P("desc"), 0, 0, 0));
        snprintf(l, sizeof l, "stats.ach.%u.display.hidden", i); emit_str(l, (const char *)call(o, "GetAchievementDisplayAttribute", P(name), P("hidden"), 0, 0, 0));
        snprintf(l, sizeof l, "stats.ach.%u.display.bogus", i); emit_str(l, (const char *)call(o, "GetAchievementDisplayAttribute", P(name), P("bogus"), 0, 0, 0));
    }
    emit_str("stats.ach.name.out_of_range", (const char *)call(o, "GetAchievementName", n + 3, 0, 0, 0, 0));
    { uint8_t got = 0xee; uint64_t r = B(call(o, "GetAchievement", P("NAO_EXISTE"), P(&got), 0, 0, 0)); fprintf(g_out, "S stats.GetAchievement.unknown ret=%llx got=%x\n", (unsigned long long)r, got); }
    static const char *const names[] = { "NumGames", "NumWins", "NumLosses", "FeetTraveled", "MaxFeetTraveled", "AverageSpeed", "NAO_EXISTE" };
    for (int i = 0; i < 7; i++) {
        int32_t iv = 0x7e7e; float fv = 123.5f; char l[96];
        uint64_t r1 = B(call(o, "GetStat", P(names[i]), P(&iv), 0, 0, 0));
        uint64_t r2 = B(call(o, "GetStat_2", P(names[i]), P(&fv), 0, 0, 0));
        snprintf(l, sizeof l, "stats.stat.%s", names[i]);
        fprintf(g_out, "S %s int_ret=%llx int=%x float_ret=%llx float=%x\n", l, (unsigned long long)r1, iv, (unsigned long long)r2, *(uint32_t *)&fv);
    }
    { float pct = -1; uint64_t r = B(call(o, "GetAchievementAchievedPercent", P("ACH_WIN_ONE_GAME"), P(&pct), 0, 0, 0)); fprintf(g_out, "S ~stats.GetAchievementAchievedPercent ret=%llx\n", (unsigned long long)r); }
    emit("stats.GetAchievementIcon.unknown", (uint32_t)call(o, "GetAchievementIcon", P("NAO_EXISTE"), 0, 0, 0, 0));
    emit("stats.IndicateAchievementProgress.unknown", B(call(o, "IndicateAchievementProgress", P("NAO_EXISTE"), 1, 2, 0, 0)));
}

static void remote(void) {
    Obj o = get("STEAMREMOTESTORAGE_INTERFACE_VERSION016");
    emit("remote.iface", o.obj != NULL);
    emit("remote.IsCloudEnabledForAccount", B(call(o, "IsCloudEnabledForAccount", 0, 0, 0, 0, 0)));
    emit("remote.IsCloudEnabledForApp", B(call(o, "IsCloudEnabledForApp", 0, 0, 0, 0, 0)));
    emit("remote.FileExists.unknown", B(call(o, "FileExists", P("nao_existe.dat"), 0, 0, 0, 0)));
    emit("remote.GetFileSize.unknown", (uint32_t)call(o, "GetFileSize", P("nao_existe.dat"), 0, 0, 0, 0));
    emit("remote.GetFileTimestamp.unknown", call(o, "GetFileTimestamp", P("nao_existe.dat"), 0, 0, 0, 0));
    emit("remote.FilePersisted.unknown", B(call(o, "FilePersisted", P("nao_existe.dat"), 0, 0, 0, 0)));
    { char buf[16] = {0}; emit("remote.FileRead.unknown", (uint32_t)call(o, "FileRead", P("nao_existe.dat"), P(buf), sizeof buf, 0, 0)); }
    emit("remote.GetFileCount.nonneg", (int32_t)call(o, "GetFileCount", 0, 0, 0, 0, 0) >= 0);
    { uint64_t total = 0xdead, avail = 0xdead; uint64_t r = B(call(o, "GetQuota", P(&total), P(&avail), 0, 0, 0)); fprintf(g_out, "S remote.GetQuota ret=%llx total=%llx avail_le_total=%d\n", (unsigned long long)r, (unsigned long long)total, avail <= total); }
    emit("remote.GetSyncPlatforms.unknown", (uint32_t)call(o, "GetSyncPlatforms", P("nao_existe.dat"), 0, 0, 0, 0));
    emit("remote.GetCachedUGCCount", (uint32_t)call(o, "GetCachedUGCCount", 0, 0, 0, 0, 0));
    emit("remote.GetLocalFileChangeCount", (uint32_t)call(o, "GetLocalFileChangeCount", 0, 0, 0, 0, 0));
}

static void matchmaking(void) {
    Obj o = get("SteamMatchMaking009");
    emit("mm.iface", o.obj != NULL);
    emit("mm.GetFavoriteGameCount", (uint32_t)call(o, "GetFavoriteGameCount", 0, 0, 0, 0, 0));
    emit("mm.GetNumLobbyMembers.unknown", (uint32_t)call(o, "GetNumLobbyMembers", 109775240000000001ULL, 0, 0, 0, 0));
    emit("mm.GetLobbyOwner.unknown", call(o, "GetLobbyOwner", 109775240000000001ULL, 0, 0, 0, 0));
    emit_str("mm.GetLobbyData.unknown", (const char *)call(o, "GetLobbyData", 109775240000000001ULL, P("map"), 0, 0, 0));
    emit("mm.GetLobbyDataCount.unknown", (uint32_t)call(o, "GetLobbyDataCount", 109775240000000001ULL, 0, 0, 0, 0));
    emit("mm.GetLobbyMemberLimit.unknown", (uint32_t)call(o, "GetLobbyMemberLimit", 109775240000000001ULL, 0, 0, 0, 0));
    emit("mm.GetLobbyByIndex.0", call(o, "GetLobbyByIndex", 0, 0, 0, 0, 0));
}

int main(int argc, char **argv) {
    const char *dll_path = argc > 1 ? argv[1] : getenv("PROBE_DLL"), *out_path = argc > 2 ? argv[2] : getenv("PROBE_OUT"), *appid = argc > 3 ? argv[3] : getenv("PROBE_APPID");
    if (!dll_path || !out_path) { fprintf(stderr, "usage: scenarios <dll> <output> [appid]\n"); return 2; }
    g_out = fopen(out_path, "w");
    if (!g_out) return 2;
    if (appid) { SetEnvironmentVariableA("SteamAppId", appid); SetEnvironmentVariableA("SteamGameId", appid); }
    HMODULE dll = LoadLibraryA(dll_path);
    if (!dll) { fprintf(g_out, "ERROR load %lu\n", GetLastError()); return 2; }
    CreateInterfaceFn create = (CreateInterfaceFn)GetProcAddress(dll, "CreateInterface");
    g_client = create ? create("SteamClient023", NULL) : NULL;
    if (!g_client) { fprintf(g_out, "ERROR no client\n"); return 2; }
    int hidden;
    g_pipe = (uint32_t)((Fn8)slot_fn(g_client, "SteamClient023", "CreateSteamPipe", &hidden))(g_client, 0, 0, 0, 0, 0, 0, 0);
    g_user = (uint32_t)((Fn8)slot_fn(g_client, "SteamClient023", "ConnectToGlobalUser", &hidden))(g_client, g_pipe, 0, 0, 0, 0, 0, 0);
    fprintf(g_out, "S pipe_user_nonzero %d\n", g_pipe != 0 && g_user != 0);
    if (!g_pipe || !g_user) { fprintf(g_out, "NOPIPE\n"); return 5; }
    apps(); utils(); user(); friends(); stats(); remote(); matchmaking();
    fprintf(g_out, "END\n");
    fclose(g_out);
    return 0;
}
