#include <stdio.h>
#include <time.h>
#include <windows.h>

#include "../shim.h"

static uint64_t SHIM_TC ret0(void *self) { (void)self; return 0; }
static uint64_t SHIM_TC big_picture(void *self) { (void)self; return shim_overlay_capable() ? 1 : 0; }
static uint64_t SHIM_TC universe(void *self) { (void)self; return 1; }
static uint32_t SHIM_TC app_id(void *self) { (void)self; shim_trace("GetAppID"); return shim_process_appid(); }
static const char *SHIM_TC country(void *self) { (void)self; shim_trace("GetIPCountry"); const Snapshot *s = shim_snapshot(); return s ? s->country : ""; }
static uint32_t SHIM_TC since_app_active(void *self) {
    (void)self;
    static DWORD last_active;
    DWORD now = GetTickCount();
    if (!last_active) last_active = now;
    DWORD pid = 0;
    HWND fg = GetForegroundWindow();
    if (fg) GetWindowThreadProcessId(fg, &pid);
    if (pid == GetCurrentProcessId()) last_active = now;
    return (now - last_active) / 1000;
}
static uint32_t SHIM_TC since_computer_active(void *self) {
    (void)self;
    LASTINPUTINFO li = { sizeof li, 0 };
    return GetLastInputInfo(&li) ? (GetTickCount() - li.dwTime) / 1000 : 0;
}
static uint64_t SHIM_TC running_under_proton(void *self) {
    (void)self;
    HMODULE nt = GetModuleHandleA("ntdll.dll");
    return nt && GetProcAddress(nt, "wine_get_version") != NULL;
}
static uint32_t SHIM_TC ipc_call_count(void *self) { (void)self; return shim_ipc_call_count(); }
static uint64_t SHIM_TC image_size(void *self, int handle, uint32_t *w, uint32_t *h) { (void)self; (void)handle; if (w) *w = 0; if (h) *h = 0; return 0; }
static uint32_t SHIM_TC server_time(void *self) { (void)self; return (uint32_t)time(NULL); }
const char *shim_game_language(void);
static const char *SHIM_TC ui_language(void *self) { (void)self; return shim_game_language(); }

static uint64_t SHIM_TC api_call_completed(void *self, uint64_t handle, uint8_t *failed) {
    (void)self;
    if (handle == 0) { if (failed) *failed = 1; return 1; }
    bool io_failed = false;
    bool done = shim_callresult_completed(handle, &io_failed);
    if (failed) *failed = io_failed;
    return done;
}
static int SHIM_TC api_call_failure_reason(void *self, uint64_t handle) { (void)self; return handle == 0 ? 2 : -1; }
static uint64_t SHIM_TC api_call_result(void *self, uint64_t handle, void *buf, int cap, int expected, uint8_t *failed) {
    (void)self;
    bool io_failed = false;
    bool ok = shim_callresult_take(handle, buf, cap, expected, &io_failed);
    if (failed) *failed = io_failed;
    return ok;
}

static int copy_text(char *out, uint32_t cap, const char *in) {
    if (!out || cap == 0) return 0;
    snprintf(out, cap, "%s", in ? in : "");
    return (int)strlen(out);
}
static int SHIM_TC filter_text_v9(void *self, char *out, uint32_t cap, const char *in, uint8_t legal) { (void)self; (void)legal; return copy_text(out, cap, in); }
static int SHIM_TC filter_text_v10(void *self, int ctx, uint64_t source, const char *in, char *out, uint32_t cap) { (void)self; (void)ctx; (void)source; copy_text(out, cap, in); return 0; }
static uint64_t SHIM_TC battery_ac(void *self) { (void)self; return 255; }

static uint64_t SHIM_TC check_file_signature(void *self, const char *name) {
    (void)self; (void)name;
    uint64_t call = shim_callresult_reserve();
    int32_t result = 3;
    shim_callresult_complete(call, 705, &result, sizeof result);
    return call;
}

void register_utils(void) {
    shim_register_family("SteamUtils", "CheckFileSignature", (void *)check_file_signature);
    shim_register_family_ab("SteamUtils", "FilterText", 16, (void *)filter_text_v9);
    shim_register_family_ab("SteamUtils", "FilterText", 24, (void *)filter_text_v10);
    shim_register_family("SteamUtils", "GetCurrentBatteryPower", (void *)battery_ac);
    shim_register_family("SteamUtils", "IsAPICallCompleted", (void *)api_call_completed);
    shim_register_family("SteamUtils", "GetAPICallFailureReason", (void *)api_call_failure_reason);
    shim_register_family("SteamUtils", "GetAPICallResult", (void *)api_call_result);
    shim_register_family("SteamUtils", "GetAppID", (void *)app_id);
    shim_register_family("SteamUtils", "GetImageSize", (void *)image_size);
    shim_register_family("SteamUtils", "GetIPCountry", (void *)country);
    shim_register_family("SteamUtils", "GetConnectedUniverse", (void *)universe);
    shim_register_family("SteamUtils", "GetSecondsSinceAppActive", (void *)since_app_active);
    shim_register_family("SteamUtils", "GetSecondsSinceComputerActive", (void *)since_computer_active);
    shim_register_family("SteamUtils", "GetIPCCallCount", (void *)ipc_call_count);
    shim_register_family("SteamUtils", "IsRunningUnderProton", (void *)running_under_proton);
    shim_register_family("SteamUtils", "GetServerRealTime", (void *)server_time);
    shim_register_family("SteamUtils", "GetSteamUILanguage", (void *)ui_language);
    static const char *const zero[] = { "BOverlayNeedsPresent", "IsSteamRunningInVR",
        "IsSteamRunningOnSteamDeck", "IsSteamChinaLauncher", "RunFrame", "SetOverlayNotificationPosition",
        "SetWarningMessageHook", "IsVRHeadsetStreamingEnabled" };
    for (size_t i = 0; i < sizeof zero / sizeof *zero; i++) shim_register_family("SteamUtils", zero[i], (void *)ret0);
    shim_register_family("SteamUtils", "IsSteamInBigPictureMode", (void *)big_picture);
}
