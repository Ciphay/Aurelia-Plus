#include <string.h>

#include "../shim.h"

static void *find(const char *version) {
    void *native = shim_gns_native(version);
    if (native) { p2p_interface_requested(version); return native; }
    for (int i = 0; version && i < g_n_ifaces; i++)
        if (!strcmp(g_ifaces[i].name, version)) { p2p_interface_requested(version); return g_ifaces[i].object; }
    shim_log("GetISteam*: unknown version %s", version ? version : "(null)");
    return NULL;
}

static uint64_t SHIM_TC ret0(void *self) { (void)self; return 0; }
static uint64_t SHIM_TC ret1(void *self) { (void)self; return 1; }
static int SHIM_TC create_pipe(void *self) { (void)self; shim_trace("CreateSteamPipe"); return 1; }
static int SHIM_TC connect_user(void *self, int pipe) { (void)self; (void)pipe; shim_trace("ConnectToGlobalUser"); return 1; }
static int SHIM_TC create_local_user(void *self, int *pipe, int type) { (void)self; (void)type; if (pipe) *pipe = 1; return 1; }
#define GET3(name) static void *SHIM_TC get3_##name(void *self, int user, int pipe, const char *version) { \
    (void)self; (void)user; (void)pipe; shim_trace(#name " version=%s", version ? version : "(null)"); return find(version); }
GET3(GetISteamUser)
GET3(GetISteamGameServer)
GET3(GetISteamFriends)
GET3(GetISteamMatchmaking)
GET3(GetISteamMatchmakingServers)
GET3(GetISteamGenericInterface)
GET3(GetISteamUserStats)
GET3(GetISteamGameServerStats)
GET3(GetISteamApps)
GET3(GetISteamNetworking)
GET3(GetISteamRemoteStorage)
GET3(GetISteamScreenshots)
GET3(GetISteamHTTP)
GET3(GetISteamController)
GET3(GetISteamUGC)
GET3(GetISteamMusic)
GET3(GetISteamHTMLSurface)
GET3(GetISteamInventory)
GET3(GetISteamVideo)
GET3(GetISteamParentalSettings)
GET3(GetISteamInput)
GET3(GetISteamParties)
GET3(GetISteamRemotePlay)
GET3(GetISteamMusicRemote)
GET3(GetISteamAppList)
GET3(GetISteamAppTicket)
GET3(GetISteamGameSearch)
GET3(GetISteamBilling)
GET3(GetISteamContentServer)
GET3(GetISteamMasterServerUpdater)
GET3(GetISteamUnifiedMessages)
GET3(DEPRECATED_GetISteamUnifiedMessages)
static void *SHIM_TC get_utils(void *self, int pipe, const char *version) { (void)self; (void)pipe; shim_trace("GetISteamUtils version=%s", version ? version : "(null)"); return find(version); }

static int SHIM_TC create_global_user(void *self, int *pipe) { (void)self; if (pipe) *pipe = 1; return 1; }
static const char *SHIM_TC universe_name(void *self, int universe) { (void)self; return universe == 1 ? "Public" : universe == 2 ? "Beta" : universe == 3 ? "Internal" : universe == 4 ? "Dev" : "Invalid"; }

void register_client(void) {
    shim_register_family("SteamClient", "CreateGlobalUser", (void *)create_global_user);
    shim_register_family("SteamClient", "GetUniverseName", (void *)universe_name);
    static const struct { const char *name; void *fn; } three[] = {
        { "GetISteamUser", (void *)get3_GetISteamUser },
        { "GetISteamGameServer", (void *)get3_GetISteamGameServer },
        { "GetISteamFriends", (void *)get3_GetISteamFriends },
        { "GetISteamMatchmaking", (void *)get3_GetISteamMatchmaking },
        { "GetISteamMatchmakingServers", (void *)get3_GetISteamMatchmakingServers },
        { "GetISteamGenericInterface", (void *)get3_GetISteamGenericInterface },
        { "GetISteamUserStats", (void *)get3_GetISteamUserStats },
        { "GetISteamGameServerStats", (void *)get3_GetISteamGameServerStats },
        { "GetISteamApps", (void *)get3_GetISteamApps },
        { "GetISteamNetworking", (void *)get3_GetISteamNetworking },
        { "GetISteamRemoteStorage", (void *)get3_GetISteamRemoteStorage },
        { "GetISteamScreenshots", (void *)get3_GetISteamScreenshots },
        { "GetISteamHTTP", (void *)get3_GetISteamHTTP },
        { "GetISteamController", (void *)get3_GetISteamController },
        { "GetISteamUGC", (void *)get3_GetISteamUGC },
        { "GetISteamMusic", (void *)get3_GetISteamMusic },
        { "GetISteamHTMLSurface", (void *)get3_GetISteamHTMLSurface },
        { "GetISteamInventory", (void *)get3_GetISteamInventory },
        { "GetISteamVideo", (void *)get3_GetISteamVideo },
        { "GetISteamParentalSettings", (void *)get3_GetISteamParentalSettings },
        { "GetISteamInput", (void *)get3_GetISteamInput },
        { "GetISteamParties", (void *)get3_GetISteamParties },
        { "GetISteamRemotePlay", (void *)get3_GetISteamRemotePlay },
        { "GetISteamMusicRemote", (void *)get3_GetISteamMusicRemote },
        { "GetISteamAppList", (void *)get3_GetISteamAppList },
        { "GetISteamAppTicket", (void *)get3_GetISteamAppTicket },
        { "GetISteamGameSearch", (void *)get3_GetISteamGameSearch },
        { "GetISteamBilling", (void *)get3_GetISteamBilling },
        { "GetISteamContentServer", (void *)get3_GetISteamContentServer },
        { "GetISteamMasterServerUpdater", (void *)get3_GetISteamMasterServerUpdater },
        { "GetISteamUnifiedMessages", (void *)get3_GetISteamUnifiedMessages },
        { "DEPRECATED_GetISteamUnifiedMessages", (void *)get3_DEPRECATED_GetISteamUnifiedMessages },
    };
    for (size_t i = 0; i < sizeof three / sizeof *three; i++) shim_register_family("SteamClient", three[i].name, three[i].fn);
    shim_register_family("SteamClient", "GetISteamUtils", (void *)get_utils);
    shim_register_family("SteamClient", "CreateSteamPipe", (void *)create_pipe);
    shim_register_family("SteamClient", "ConnectToGlobalUser", (void *)connect_user);
    shim_register_family("SteamClient", "CreateLocalUser", (void *)create_local_user);
    shim_register_family("SteamClient", "BReleaseSteamPipe", (void *)ret1);
    shim_register_family("SteamClient", "BShutdownIfAllPipesClosed", (void *)ret1);
    static const char *const nothing[] = { "ReleaseUser", "SetLocalIPBinding", "RunFrame", "SetWarningMessageHook", "GetIPCCallCount",
        "Set_SteamAPI_CCheckCallbackRegisteredInProcess", "DEPRECATED_Set_SteamAPI_CPostAPIResultInProcess",
        "DEPRECATED_Remove_SteamAPI_CPostAPIResultInProcess", "DestroyAllInterfaces", "Set_SteamAPI_CPostAPIResultInProcess",
        "Remove_SteamAPI_CPostAPIResultInProcess", "GetIVAC" };
    for (size_t i = 0; i < sizeof nothing / sizeof *nothing; i++) shim_register_family("SteamClient", nothing[i], (void *)ret0);
    shim_register_family("SteamInput", "RunFrame", (void *)ret0);
    shim_register_family("SteamController", "RunFrame", (void *)ret0);
}
