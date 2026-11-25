#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "../shim.h"

static uint64_t SHIM_TC ret0(void *self) { (void)self; return 0; }
static void post_no_proof_key(uint32_t appid) {
    uint8_t resp[12 + 240];
    memset(resp, 0, sizeof resp);
    int32_t fail = 2;
    memcpy(resp, &fail, 4);
    memcpy(resp + 4, &appid, 4);
    shim_post_callback(1021, resp, sizeof resp);
}
static const ShimDlc *find_dlc(uint32_t appid);

static void SHIM_TC install_dlc(void *self, uint32_t appid) {
    (void)self;
    const ShimDlc *d = find_dlc(appid);
    if (d && d->owned && d->installed) shim_post_callback(1005, &appid, sizeof appid);
}
static void SHIM_TC request_all_proof_keys(void *self) { (void)self; post_no_proof_key(shim_process_appid()); }
static void SHIM_TC request_proof_key(void *self, uint32_t appid) { (void)self; post_no_proof_key(appid); }

static const ShimApp *find_app(uint32_t appid) {
    const Snapshot *s = shim_snapshot();
    for (size_t i = 0; s && i < s->n_apps; i++) if (s->apps[i].appid == appid) return &s->apps[i];
    return NULL;
}

static const ShimDlc *find_dlc(uint32_t appid) {
    const Snapshot *s = shim_snapshot();
    for (size_t i = 0; s && s->has_app && i < s->app.n_dlc; i++) if (s->app.dlc[i].appid == appid) return &s->app.dlc[i];
    return NULL;
}

static void post_dlc_installed(void *a, void *b, int appid) {
    (void)a; (void)b;
    uint32_t id = (uint32_t)appid;
    shim_post_callback(1005, &id, sizeof id);
}

void shim_dlc_apply(const uint8_t *buf, size_t n) {
    const Snapshot *s = shim_snapshot();
    if (!s || !s->has_app || n < 4) return;
    uint32_t count;
    memcpy(&count, buf, 4);
    for (uint32_t i = 0; i < count && 4 + 6 * (size_t)(i + 1) <= n; i++) {
        uint32_t id;
        memcpy(&id, buf + 4 + 6 * (size_t)i, 4);
        bool owned = buf[4 + 6 * (size_t)i + 4] != 0, installed = buf[4 + 6 * (size_t)i + 5] != 0;
        for (size_t k = 0; k < s->app.n_dlc; k++) {
            ShimDlc *d = &s->app.dlc[k];
            if (d->appid != id) continue;
            bool became_installed = installed && !d->installed;
            d->owned = owned;
            d->installed = installed;
            if (became_installed) shim_defer(post_dlc_installed, NULL, NULL, (int)id);
        }
    }
}

bool shim_app_owned(uint32_t appid) {
    static struct { uint32_t app; int8_t owned; } cache[64];
    static unsigned next;
    static SRWLOCK lock = SRWLOCK_INIT;
    if (!appid) return false;
    const ShimDlc *d = find_dlc(appid);
    if (find_app(appid) || (d && d->owned)) return true;
    AcquireSRWLockExclusive(&lock);
    for (unsigned i = 0; i < 64; i++) if (cache[i].app == appid) { bool r = cache[i].owned > 0; ReleaseSRWLockExclusive(&lock); return r; }
    ReleaseSRWLockExclusive(&lock);
    uint8_t req[13] = { 12 };
    uint32_t cur = shim_snapshot() && shim_snapshot()->has_ticket ? shim_snapshot()->ticket.appid : 0;
    uint64_t id = appid;
    memcpy(req + 1, &cur, 4);
    memcpy(req + 5, &id, 8);
    int32_t status = -1, answer = 0;
    uint8_t reply[8];
    size_t n = 0;
    if (!bridge_call_fresh(0xFFFF, 10, req, sizeof req, &status, reply, sizeof reply, &n, 8000) || status != 0 || n < 4) return false;
    memcpy(&answer, reply, 4);
    AcquireSRWLockExclusive(&lock);
    cache[next++ % 64].app = appid;
    cache[(next - 1) % 64].owned = answer ? 1 : -1;
    ReleaseSRWLockExclusive(&lock);
    return answer != 0;
}

static uint64_t SHIM_TC is_subscribed_app(void *self, uint32_t appid) {
    (void)self;
    bool owned = shim_app_owned(appid);
    shim_trace("BIsSubscribedApp app=%u -> %d", appid, owned);
    return owned;
}
static uint32_t SHIM_TC installed_depots(void *self, uint32_t appid, uint32_t *out, uint32_t max) {
    const Snapshot *s = shim_snapshot();
    if (!s || !s->has_app || (appid && appid != s->app.appid)) return 0;
    if (!out) {
        const char *name = shim_iface_name(self);
        return name && strstr(name, "VERSION005") ? (uint32_t)s->app.n_depots : 0;
    }
    uint32_t n = (uint32_t)s->app.n_depots < max ? (uint32_t)s->app.n_depots : max;
    if (n) memcpy(out, s->app.depots, 4u * n);
    return n;
}
static uint64_t SHIM_TC mark_content_corrupt(void *self, uint8_t missing_only) {
    (void)self;
    shim_log("MarkContentCorrupt(missing only=%d): Aurelia verifies the installation with the `aurelia verify` command", missing_only);
    return 1;
}
static const ShimDlc *dlc_at(const Snapshot *s, int i) {
    if (!s || !s->has_app || i < 0 || (size_t)i >= s->app.n_dlc) return NULL;
    return &s->app.dlc[i];
}
static uint32_t SHIM_TC dlc_count(void *self) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    return s && s->has_app ? (uint32_t)s->app.n_dlc : 0;
}
static uint64_t SHIM_TC dlc_by_index(void *self, int i, uint32_t *appid, uint8_t *available, char *name, int cap) {
    (void)self;
    const ShimDlc *d = dlc_at(shim_snapshot(), i);
    if (!d && appid) { *appid = 0; if (available) *available = 0; if (name && cap > 0) name[0] = 0; }
    if (!d || !appid) return 0;
    *appid = d->appid;
    if (available) *available = d->owned;
    if (name && cap > 0) snprintf(name, (size_t)cap, "%s", d->name);
    return 1;
}
static uint64_t SHIM_TC is_dlc_installed(void *self, uint32_t appid) { (void)self; const ShimDlc *d = find_dlc(appid); return d && d->owned && d->installed; }

static uint32_t SHIM_TC app_install_dir(void *self, uint32_t appid, char *buf, uint32_t cap) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    if (!s || !s->has_app || s->app.appid != appid || !s->app.install_dir || !s->app.install_dir[0]) return 0;
    char tmp[1024];
    size_t n = shim_win_path(s->app.install_dir, tmp, sizeof tmp);
    if (n == 0) return 0;
    if (!buf || cap == 0) return (uint32_t)(n + 1);
    strncpy(buf, tmp, cap - 1);
    buf[cap - 1] = 0;
    return (uint32_t)(strlen(buf) + 1);
}
static uint64_t SHIM_TC current_beta_name(void *self, char *buf, int cap) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    const char *beta = s && s->has_app && s->app.beta_name ? s->app.beta_name : "";
    bool is_beta = beta[0] && strcmp(beta, "public");
    if (buf && cap > 0) snprintf(buf, (size_t)cap, "%s", is_beta ? beta : "");
    return is_beta;
}
static uint32_t SHIM_TC app_build_id(void *self) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    return s && s->has_app ? (uint32_t)s->app.build_id : 0;
}
static uint64_t SHIM_TC is_subscribed(void *self) {
    (void)self;
    shim_trace("BIsSubscribed");
    const Snapshot *s = shim_snapshot();
    return s && s->has_ticket && shim_app_owned(s->ticket.appid);
}
static uint64_t SHIM_TC is_app_installed(void *self, uint32_t appid) { (void)self; const ShimApp *a = find_app(appid); return a && a->installed; }
const char *shim_game_language(void) { const char *l = getenv("AURELIA_SHIM_LANGUAGE"); return l && *l ? l : "english"; }
static const char *SHIM_TC language(void *self) { (void)self; shim_trace("GetCurrentGameLanguage"); return shim_game_language(); }
static const char *SHIM_TC empty(void *self) { (void)self; return ""; }
static int SHIM_TC launch_command_line(void *self, char *buf, int cb) { (void)self; if (buf && cb > 0) buf[0] = 0; return 0; }
static uint64_t *SHIM_TC get_app_owner(void *self, uint64_t *ret) { (void)self; shim_trace("GetAppOwner"); const Snapshot *s = shim_snapshot(); *ret = s ? s->steamid : 0; return ret; }

static uint32_t SHIM_TC earliest_purchase(void *self, uint32_t appid) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    uint32_t cached = s && s->has_app && s->app.appid == appid ? s->app.purchase_time : 0;
    uint8_t req[5] = { 1 };
    memcpy(req + 1, &appid, 4);
    int32_t status = -1;
    uint8_t reply[4];
    size_t n = 0;
    uint32_t t = cached;
    if (bridge_call_fresh(0xFFFF, 16, req, sizeof req, &status, reply, sizeof reply, &n, 5000) && status == 0 && n >= 4) {
        uint32_t got;
        memcpy(&got, reply, 4);
        if (got) t = got;
    }
    shim_trace("GetEarliestPurchaseUnixTime app=%u -> %u", appid, t);
    return t;
}

static int SHIM_TC get_app_data(void *self, uint32_t appid, const char *key, char *buf, int cb) {
    (void)self;
    if (!key || !buf || cb <= 0) return 0;
    const Snapshot *s = shim_snapshot();
    const ShimApp *a = find_app(appid);
    const char *value = NULL;
    char dir[260] = "";
    if (!strcmp(key, "name")) value = a && a->name ? a->name : NULL;
    else if (!strcmp(key, "installdir") && s && s->has_app && s->app.appid == appid && s->app.install_dir) {
        const char *slash = strrchr(s->app.install_dir, '/');
        snprintf(dir, sizeof dir, "%s", slash ? slash + 1 : s->app.install_dir);
        value = dir;
    } else if (!strcmp(key, "type") && a) value = "game";
    if (!value || !*value) { buf[0] = 0; return 0; }
    snprintf(buf, (size_t)cb, "%s", value);
    return (int)strlen(buf) + 1 < cb ? (int)strlen(buf) + 1 : cb;
}

static void reg(const char *method, void *fn) {
    shim_register_family("STEAMAPPS_INTERFACE_VERSION", method, fn);
    shim_register_family("SteamApps", method, fn);
}

enum { SLOT_UGC_APPS = 10, OP_SET_BRANCH = 11, BETA_DEFAULT = 1, BETA_AVAILABLE = 2, BETA_PRIVATE = 4, BETA_SELECTED = 8 };

static uint32_t SHIM_TC num_betas(void *self, int32_t *available, int32_t *priv) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    int32_t a = 0, p = 0;
    for (size_t i = 0; s && s->has_app && i < s->app.n_betas; i++) { if (s->app.betas[i].priv) p++; else a++; }
    if (available) *available = a;
    if (priv) *priv = p;
    return (uint32_t)(a + p);
}

static const ShimBeta *beta_at(const Snapshot *s, int32_t index) {
    for (int pass = 0; pass < 2; pass++) {
        for (size_t i = 0; i < s->app.n_betas; i++) {
            if (s->app.betas[i].priv != (pass == 1)) continue;
            if (index-- == 0) return &s->app.betas[i];
        }
    }
    return NULL;
}

static uint64_t SHIM_TC beta_info(void *self, int32_t index, uint32_t *flags, uint32_t *build_id, char *name, int32_t cch_name, char *desc, int32_t cch_desc) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    if (!s || !s->has_app || index < 0) return 0;
    const ShimBeta *pick = beta_at(s, index);
    if (!pick) return 0;
    const char *current = s->app.beta_name && s->app.beta_name[0] ? s->app.beta_name : "public";
    if (flags) *flags = (pick->priv ? BETA_PRIVATE : BETA_AVAILABLE) | (!strcmp(pick->name, "public") ? BETA_DEFAULT : 0) | (!strcmp(pick->name, current) ? BETA_SELECTED : 0);
    if (build_id) *build_id = pick->build_id;
    if (name && cch_name > 0) snprintf(name, (size_t)cch_name, "%s", pick->name ? pick->name : "");
    if (desc && cch_desc > 0) snprintf(desc, (size_t)cch_desc, "%s", pick->description ? pick->description : "");
    return 1;
}

static uint64_t SHIM_TC beta_info_v9(void *self, int32_t index, uint32_t *flags, uint32_t *build_id, char *name, int32_t cch_name, char *desc, int32_t cch_desc, uint32_t *time_updated) {
    const Snapshot *s = shim_snapshot();
    const ShimBeta *pick = s && s->has_app && index >= 0 ? beta_at(s, index) : NULL;
    if (time_updated) *time_updated = pick ? pick->time_updated : 0;
    return beta_info(self, index, flags, build_id, name, cch_name, desc, cch_desc);
}

static uint64_t SHIM_TC set_active_beta(void *self, const char *name) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    if (!s || !s->has_app || !name || !*name) return 0;
    bool known = false;
    for (size_t i = 0; i < s->app.n_betas; i++) if (!s->app.betas[i].priv && !strcmp(s->app.betas[i].name, name)) known = true;
    if (!known) return 0;
    uint8_t req[13 + 128];
    size_t len = strlen(name);
    if (len >= 128) return 0;
    uint32_t app = s->app.appid;
    uint64_t zero = 0;
    req[0] = OP_SET_BRANCH;
    memcpy(req + 1, &app, 4);
    memcpy(req + 5, &zero, 8);
    memcpy(req + 13, name, len);
    int32_t status = -1;
    uint8_t reply[8];
    size_t n = 0;
    int32_t result = 2;
    if (bridge_call_fresh(0xFFFF, SLOT_UGC_APPS, req, 13 + len, &status, reply, sizeof reply, &n, 20000) && status == 0 && n >= 4) memcpy(&result, reply, 4);
    return result == 1;
}

static uint64_t SHIM_TC timed_trial(void *self, uint32_t *allowed, uint32_t *played) { (void)self; if (allowed) *allowed = 0; if (played) *played = 0; return 0; }

static uint64_t SHIM_TC set_dlc_context(void *self, uint32_t appid) { (void)self; (void)appid; return 1; }
void register_apps(void) {
    shim_register_family("STEAMAPPS_INTERFACE_VERSION", "SetDlcContext", (void *)set_dlc_context);
    reg("GetEarliestPurchaseUnixTime", (void *)earliest_purchase);
    reg("GetAppData", (void *)get_app_data);
    shim_register_family_ab("STEAMAPPS_INTERFACE_VERSION", "BIsTimedTrial", 8, (void *)timed_trial);
    shim_register_family_ab("STEAMAPPS_INTERFACE_VERSION", "GetNumBetas", 8, (void *)num_betas);
    shim_register_family_ab("STEAMAPPS_INTERFACE_VERSION", "GetBetaInfo", 28, (void *)beta_info);
    shim_register_family_ab("STEAMAPPS_INTERFACE_VERSION", "GetBetaInfo", 32, (void *)beta_info_v9);
    shim_register_family("STEAMAPPS_INTERFACE_VERSION", "SetActiveBeta", (void *)set_active_beta);
    reg("BIsSubscribed", (void *)is_subscribed);
    reg("BIsSubscribedApp", (void *)is_subscribed_app);
    reg("BIsAppInstalled", (void *)is_app_installed);
    reg("GetCurrentGameLanguage", (void *)language);
    reg("GetAvailableGameLanguages", (void *)language);
    reg("GetLaunchQueryParam", (void *)empty);
    reg("GetAppOwner", (void *)get_app_owner);
    reg("GetLaunchCommandLine", (void *)launch_command_line);
    reg("GetDLCCount", (void *)dlc_count);
    reg("GetInstalledDepots", (void *)installed_depots);
    reg("MarkContentCorrupt", (void *)mark_content_corrupt);
    reg("BGetDLCDataByIndex", (void *)dlc_by_index);
    reg("BIsDlcInstalled", (void *)is_dlc_installed);
    reg("GetAppInstallDir", (void *)app_install_dir);
    reg("RequestAllProofOfPurchaseKeys", (void *)request_all_proof_keys);
    reg("InstallDLC", (void *)install_dlc);
    reg("RequestAppProofOfPurchaseKey", (void *)request_proof_key);
    shim_register_family("STEAMAPPLIST_INTERFACE_VERSION", "GetAppInstallDir", (void *)app_install_dir);
    reg("GetCurrentBetaName", (void *)current_beta_name);
    reg("GetAppBuildId", (void *)app_build_id);
    static const char *const zero[] = { "BIsLowViolence", "BIsCybercafe", "BIsVACBanned", "BIsSubscribedFromFreeWeekend",
        "BIsSubscribedFromFamilySharing" };
    for (size_t i = 0; i < sizeof zero / sizeof *zero; i++) reg(zero[i], (void *)ret0);
}
