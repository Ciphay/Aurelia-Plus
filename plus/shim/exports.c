#include <stdint.h>
#include <string.h>

#include "shim.h"

#define RET0(name) uint64_t name(void) { return 0; }

uint64_t Steam_BGetCallback(int pipe, void *msg, int *call) {
    (void)pipe;
    if (call) *call = 1;
    shim_gns_pump();
    bool got = shim_next_callback((ShimCallbackMsg *)msg);
    shim_trace("Steam_BGetCallback -> %d id=%d", got, got ? ((ShimCallbackMsg *)msg)->id : 0);
    return got;
}
uint64_t Steam_FreeLastCallback(int pipe) { (void)pipe; shim_free_last_callback(); return 1; }
uint64_t Steam_GetAPICallResult(int pipe, uint64_t call, void *out, int cb, int expected, int *failed) {
    (void)pipe;
    bool io_failed = false;
    bool ok = shim_callresult_take(call, out, cb, expected, &io_failed);
    shim_trace("Steam_GetAPICallResult call=%llu cb=%d expected=%d -> %d", (unsigned long long)call, cb, expected, ok);
    if (failed) *(uint8_t *)failed = io_failed;
    return ok;
}
void Steam_ReleaseThreadLocalMemory(int thread_exit) { (void)thread_exit; }
uint64_t Steam_IsKnownInterface(const char *name) {
    for (int i = 0; i < g_n_ifaces; i++) if (!strcmp(g_ifaces[i].name, name)) return 1;
    return 0;
}
void Steam_NotifyMissingInterface(int pipe, const char *name) { (void)pipe; shim_log("NotifyMissingInterface %s", name ? name : "?"); }
uint64_t Steam_BConnected(int user, int pipe) { (void)user; (void)pipe; return shim_session_online(); }
uint64_t Steam_BLoggedOn(int user, int pipe) { (void)user; (void)pipe; return shim_session_online(); }
uint64_t Steam_CreateSteamPipe(void) { return 1; }
uint64_t Steam_BReleaseSteamPipe(int pipe) { (void)pipe; return 1; }
uint64_t Steam_ConnectToGlobalUser(int pipe) { (void)pipe; return 1; }
uint64_t Steam_CreateGlobalUser(int *pipe) { if (pipe) *pipe = 1; return 1; }
uint64_t Steam_CreateLocalUser(int *pipe, int type) { (void)type; if (pipe) *pipe = 1; return 1; }
void Steam_ReleaseUser(int pipe, int user) { (void)pipe; (void)user; }
void Steam_SetLocalIPBinding(uint32_t ip, uint16_t port) { shim_set_local_ip_binding(ip, port); }
void Steam_LogOn(int user, int pipe, uint64_t id) { (void)user; (void)pipe; (void)id; }
void Steam_LogOff(int user, int pipe) { (void)user; (void)pipe; }
uint64_t Steam_GetGSHandle(int user, int pipe) { (void)user; (void)pipe; return (uint64_t)(uintptr_t)shim_gs_handle(); }
uint64_t Steam_GSBLoggedOn(void *h) { return h ? shim_gs_logged_on() : 0; }
uint64_t Steam_GSBSecure(void *h) { return h ? shim_gs_secure() : 0; }
uint64_t Steam_GSGetSteamID(void *h) { return h ? shim_gs_steamid() : 0; }
void Steam_GSLogOn(void *h) { if (h) shim_gs_logon(); }
void Steam_GSLogOff(void *h) { if (h) shim_gs_logoff(); }
uint64_t Steam_GSSetServerType(void *h, int32_t appid, uint32_t flags, uint32_t ip, uint16_t port, const char *dir, const char *version) { return h ? shim_gs_set_server_type(appid, flags, ip, port, dir, version) : 0; }
uint64_t Steam_GSUpdateStatus(void *h, int players, int max_players, int bots, const char *name, const char *map) { return h ? shim_gs_update_status(players, max_players, bots, name, map) : 0; }
void Steam_GSSetSpawnCount(void *h, uint32_t count) { if (h) shim_gs_set_spawn_count(count); }
uint64_t Steam_GSSendSteam3UserConnect(void *h, uint32_t user_id, uint32_t ip, const void *cookie, uint32_t cb) { return h ? shim_gs_user_connect(user_id, ip, cookie, cb) : 0; }
uint64_t Steam_GSRemoveUserConnect(void *h, uint32_t user_id) { return h ? shim_gs_user_remove(user_id) : 0; }
uint64_t Steam_GSSendUserDisconnect(void *h, uint64_t steamid, uint32_t user_id) { (void)steamid; return h ? shim_gs_user_remove(user_id) : 0; }
RET0(Steam_GSGetSteam2GetEncryptionKeyToSendToNewClient) RET0(Steam_GSSendSteam2UserConnect) RET0(Steam_GSSendUserStatusResponse)
uint64_t Steam_InitiateGameConnection(int user, int pipe, void *blob, int cb_max, uint64_t server, uint64_t game, uint32_t ip, uint16_t port, uint8_t secure) {
    (void)user; (void)pipe; (void)server; (void)secure;
    if ((int32_t)(uint32_t)(game & 0xffffffffu) < 0) return 0;
    return (uint64_t)(uint32_t)shim_user_initiate_game_connection(blob, cb_max, ip, port);
}
void Steam_TerminateGameConnection(int user, int pipe, uint32_t ip, uint16_t port) { (void)user; (void)pipe; shim_user_terminate_game_connection(ip, port); }
RET0(Breakpad_SteamMiniDumpInit) RET0(Breakpad_SteamSendMiniDump) RET0(Breakpad_SteamSetAppID)
RET0(Breakpad_SteamSetSteamID) RET0(Breakpad_SteamWriteMiniDumpSetComment)
RET0(Breakpad_SteamWriteMiniDumpUsingExceptionInfoWithBuildId)
