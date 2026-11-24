#include <stdio.h>
#include <string.h>

#include "../shim.h"

static size_t installed_count(void) {
    const Snapshot *s = shim_snapshot();
    size_t n = 0;
    for (size_t i = 0; s && i < s->n_apps; i++) n += s->apps[i].installed;
    return n;
}

static uint32_t SHIM_TC num_installed_apps(void *self) { (void)self; return (uint32_t)installed_count(); }
static uint32_t SHIM_TC get_installed_apps(void *self, uint32_t *ids, uint32_t max) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    uint32_t n = 0;
    for (size_t i = 0; s && ids && i < s->n_apps && n < max; i++) if (s->apps[i].installed) ids[n++] = s->apps[i].appid;
    return n;
}
static int SHIM_TC app_name(void *self, uint32_t appid, char *buf, int cap) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    for (size_t i = 0; s && i < s->n_apps; i++) {
        if (s->apps[i].appid != appid) continue;
        if (buf && cap > 0) snprintf(buf, (size_t)cap, "%s", s->apps[i].name ? s->apps[i].name : "");
        return (int)strlen(s->apps[i].name ? s->apps[i].name : "") + 1;
    }
    return -1;
}
static int SHIM_TC app_build_id(void *self, uint32_t appid) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    return s && s->has_app && s->app.appid == appid ? (int)s->app.build_id : 0;
}

static uint32_t SHIM_TC app_ownership_ticket(void *self, uint32_t appid, void *buf, uint32_t cap, uint32_t *off_appid, uint32_t *off_steamid,
                                             uint32_t *off_signature, uint32_t *cb_signature) {
    (void)self;
    const Snapshot *s = shim_snapshot();
    if (!s || !s->has_ticket || s->ticket.appid != appid || !s->ticket.ownership_len || cap < s->ticket.ownership_len) return 0;
    memcpy(buf, s->ticket.ownership, s->ticket.ownership_len);
    if (off_appid) *off_appid = 16;
    if (off_steamid) *off_steamid = 8;
    if (off_signature) *off_signature = (uint32_t)(s->ticket.ownership_len > 128 ? s->ticket.ownership_len - 128 : 0);
    if (cb_signature) *cb_signature = s->ticket.ownership_len > 128 ? 128 : 0;
    return (uint32_t)s->ticket.ownership_len;
}

void register_applist(void) {
    const char *p = "STEAMAPPLIST_INTERFACE";
    shim_register_family(p, "GetNumInstalledApps", (void *)num_installed_apps);
    shim_register_family(p, "GetInstalledApps", (void *)get_installed_apps);
    shim_register_family(p, "GetAppName", (void *)app_name);
    shim_register_family(p, "GetAppBuildId", (void *)app_build_id);
    shim_register_family("STEAMAPPTICKET_INTERFACE", "GetAppOwnershipTicketData", (void *)app_ownership_ticket);
}
