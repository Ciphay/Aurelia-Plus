#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "../shim.h"

enum { EMSG_ENCRYPTED_APP_TICKET = 5526, CB_ENCRYPTED_APP_TICKET = 154, WAIT_MS = 20000 };

static uint8_t *g_ticket;
static size_t g_ticket_len;
static SRWLOCK g_lock = SRWLOCK_INIT;

static uint32_t app_id(void) {
    const Snapshot *s = shim_snapshot();
    return s && s->has_ticket ? s->ticket.appid : 0;
}

static bool parse_ticket(const CmReply *r, void *ctx, void *out, int *size) {
    (void)ctx;
    PbR in;
    PbField f;
    int32_t eresult = 2;
    const uint8_t *ticket = NULL;
    size_t ticket_len = 0;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field == 2) eresult = (int32_t)(uint32_t)f.v;
        else if (f.field == 3 && f.wt == 2) { ticket = f.data; ticket_len = f.len; }
    }
    if (in.bad || *size < 4) return false;
    if (eresult == 1 && ticket && ticket_len) {
        uint8_t *copy = malloc(ticket_len);
        if (!copy) return false;
        memcpy(copy, ticket, ticket_len);
        AcquireSRWLockExclusive(&g_lock);
        free(g_ticket);
        g_ticket = copy;
        g_ticket_len = ticket_len;
        ReleaseSRWLockExclusive(&g_lock);
    } else if (eresult == 1) {
        eresult = 2;
    }
    shim_log("EncryptedAppTicket reply: eresult=%d ticket=%lu bytes", (int)eresult, (unsigned long)ticket_len);
    memcpy(out, &eresult, 4);
    *size = 4;
    return true;
}

static uint64_t SHIM_TC request_encrypted(void *self, const void *data, int cb) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    shim_trace("RequestEncryptedAppTicket cb=%d call=%llu", cb, (unsigned long long)call);
    if (cb < 0 || (cb && !data)) { shim_callresult_fail_handle(call); return call; }
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    if (cb > 0) pbw_bytes(&w, 2, data, (size_t)cb);
    shim_cm_async(call, CB_ENCRYPTED_APP_TICKET, EMSG_ENCRYPTED_APP_TICKET, false, NULL, &w, WAIT_MS, parse_ticket, NULL);
    return call;
}

static uint64_t SHIM_TC get_encrypted(void *self, void *buf, int max, uint32_t *size) {
    (void)self;
    bool ok = false;
    shim_trace("GetEncryptedAppTicket max=%d have=%lu", max, (unsigned long)g_ticket_len);
    AcquireSRWLockExclusive(&g_lock);
    if (g_ticket && size) {
        *size = (uint32_t)g_ticket_len;
        if (buf && max >= 0 && (size_t)max >= g_ticket_len) { memcpy(buf, g_ticket, g_ticket_len); ok = true; }
    }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

void register_appticket(void) {
    shim_register_family("SteamUser", "RequestEncryptedAppTicket", (void *)request_encrypted);
    shim_register_family("SteamUser", "GetEncryptedAppTicket", (void *)get_encrypted);
}
