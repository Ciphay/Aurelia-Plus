#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "../shim.h"

enum {
    EMSG_AUTH_REQUEST = 5504, EMSG_AUTHORIZE = 5505, EMSG_AUTHORIZE_RESPONSE = 5506, EMSG_GET_INFO = 5508, EMSG_INFO_RESPONSE = 5509, CB_AUTH_RESPONSE = 152,
    KV_NONE = 0, KV_STRING = 1, KV_INT32 = 2, KV_FLOAT = 3, KV_PTR = 4, KV_WSTRING = 5, KV_COLOR = 6, KV_UINT64 = 7, KV_END = 8, KV_INT64 = 10,
    MAX_ITEMS = 16, MAX_PENDING = 16, MAX_LOGGED = 2048,
};

typedef struct { char name[96]; int64_t amount; int32_t qty; uint64_t item_id; } Item;
typedef struct {
    uint64_t trans_id, order_id;
    int32_t appid, currency, sandbox, refundable, realm, cached_pmt;
    int64_t total;
    Item items[MAX_ITEMS];
    int n_items;
} Order;

typedef struct { const uint8_t *p; size_t len, off; } Rd;

static bool rd_cstr(Rd *r, const char **s) {
    const uint8_t *z = memchr(r->p + r->off, 0, r->len - r->off);
    if (!z) return false;
    *s = (const char *)(r->p + r->off);
    r->off = (size_t)(z - r->p) + 1;
    return true;
}

static bool rd_num(Rd *r, void *out, size_t n) {
    if (r->len - r->off < n) return false;
    memcpy(out, r->p + r->off, n);
    r->off += n;
    return true;
}

static bool parse_order(const uint8_t *body, size_t len, Order *o) {
    Rd r = { body, len, 0 };
    int depth = 0, item = -1;
    char path[2][32] = { "", "" };
    memset(o, 0, sizeof *o);
    while (r.off < r.len) {
        uint8_t type = r.p[r.off++];
        if (type == KV_END) {
            if (depth > 0) { depth--; if (depth == 1) item = -1; }
            else if (o->order_id) return true;
            continue;
        }
        const char *name;
        if (!rd_cstr(&r, &name)) return false;
        if (type == KV_NONE) {
            if (depth < 2) snprintf(path[depth], sizeof path[0], "%s", name);
            if (depth == 1 && !strcmp(path[0], "lineitems") && o->n_items < MAX_ITEMS) item = o->n_items++;
            depth++;
            continue;
        }
        const char *sval = NULL;
        int64_t ival = 0;
        uint64_t uval = 0;
        float fval;
        uint8_t skip[4];
        switch (type) {
            case KV_STRING: if (!rd_cstr(&r, &sval)) return false; break;
            case KV_INT32: { int32_t v; if (!rd_num(&r, &v, 4)) return false; ival = v; break; }
            case KV_FLOAT: if (!rd_num(&r, &fval, 4)) return false; break;
            case KV_PTR: case KV_COLOR: if (!rd_num(&r, skip, 4)) return false; break;
            case KV_UINT64: if (!rd_num(&r, &uval, 8)) return false; ival = (int64_t)uval; break;
            case KV_INT64: if (!rd_num(&r, &ival, 8)) return false; uval = (uint64_t)ival; break;
            default: return false;
        }
        if (depth == 2 && item >= 0) {
            Item *it = &o->items[item];
            if (!strcmp(name, "description") && sval) snprintf(it->name, sizeof it->name, "%s", sval);
            else if (!strcmp(name, "gameitemid")) it->item_id = uval;
            else if (!strcmp(name, "amount")) it->amount = ival;
            else if (!strcmp(name, "quantity")) it->qty = (int32_t)ival;
        } else if (depth == 0) {
            if (!strcmp(name, "transid")) o->trans_id = uval;
            else if (!strcmp(name, "OrderID")) o->order_id = uval;
            else if (!strcmp(name, "appid")) o->appid = (int32_t)ival;
            else if (!strcmp(name, "currency")) o->currency = (int32_t)ival;
            else if (!strcmp(name, "total")) o->total = ival;
            else if (!strcmp(name, "sandbox")) o->sandbox = (int32_t)ival;
            else if (!strcmp(name, "Refundable")) o->refundable = (int32_t)ival;
            else if (!strcmp(name, "SteamRealm")) o->realm = (int32_t)ival;
            else if (!strcmp(name, "RequiresCachedPmtMethod")) o->cached_pmt = (int32_t)ival;
        }
    }
    return false;
}

static const char *const CURRENCY_CODES[] = { "?", "USD", "GBP", "EUR", "CHF", "RUB", "PLN", "BRL", "JPY", "NOK", "IDR", "MYR", "PHP", "SGD", "THB",
    "VND", "KRW", "TRY", "UAH", "MXN", "CAD", "AUD", "NZD", "CNY", "INR", "CLP", "PEN", "COP", "ZAR", "HKD", "TWD", "SAR", "AED" };

static const char *currency_name(int32_t code) { return code > 0 && (size_t)code < sizeof CURRENCY_CODES / sizeof *CURRENCY_CODES ? CURRENCY_CODES[code] : "?"; }

static void money(char *out, size_t cap, int64_t minor, int32_t code) {
    if (code == 8 || code == 15 || code == 16 || code == 25) snprintf(out, cap, "%lld", (long long)minor);
    else snprintf(out, cap, "%lld.%02lld", (long long)(minor / 100), (long long)(minor < 0 ? -minor : minor) % 100);
}

static void put_json_str(char *out, size_t cap, size_t *n, const char *s) {
    if (*n + 2 >= cap) return;
    out[(*n)++] = '"';
    for (; *s && *n + 8 < cap; s++) {
        unsigned char c = (unsigned char)*s;
        if (c == '"' || c == '\\') { out[(*n)++] = '\\'; out[(*n)++] = (char)c; }
        else if (c < 0x20) *n += (size_t)snprintf(out + *n, cap - *n, "\\u%04x", c);
        else out[(*n)++] = (char)c;
    }
    out[(*n)++] = '"';
    out[*n] = 0;
}

static void build_json(const Order *o, char *out, size_t cap) {
    size_t n = 0;
    char m1[40], m2[40];
    money(m1, sizeof m1, o->total, o->currency);
    n += (size_t)snprintf(out + n, cap - n, "{\"order_id\":\"%llu\",\"trans_id\":\"%llu\",\"url\":\"https://checkout.steampowered.com/checkout/approvetxn/%llu/?returnurl=steam\",\"window\":{\"width\":1020,\"height\":440},\"sandbox\":%s,\"refundable\":%s,\"requires_cached_payment_method\":%s,"
                          "\"realm\":%d,\"currency\":\"%s\",\"currency_code\":%d,\"total\":\"%s\",\"total_minor\":%lld,\"items\":[",
                          (unsigned long long)o->order_id, (unsigned long long)o->trans_id, (unsigned long long)o->trans_id, o->sandbox ? "true" : "false", o->refundable ? "true" : "false",
                          o->cached_pmt ? "true" : "false", o->realm, currency_name(o->currency), o->currency, m1, (long long)o->total);
    for (int i = 0; i < o->n_items; i++) {
        const Item *it = &o->items[i];
        money(m2, sizeof m2, it->amount, o->currency);
        n += (size_t)snprintf(out + n, cap - n, "%s{\"name\":", i ? "," : "");
        put_json_str(out, cap, &n, it->name);
        n += (size_t)snprintf(out + n, cap - n, ",\"qty\":%d,\"price\":\"%s\",\"amount_minor\":%lld,\"item_id\":\"%llu\"}", it->qty, m2, (long long)it->amount,
                              (unsigned long long)it->item_id);
    }
    snprintf(out + n, cap - n, "]}");
}

typedef struct { bool used; char id[24]; uint64_t order, trans; uint32_t appid; } Pending;
static Pending g_pending[MAX_PENDING];
static uint64_t g_dec_order[MAX_PENDING];
static int g_dec_state[MAX_PENDING], g_dec_next;
static char g_owned[MAX_PENDING][24];
static int g_owned_next;
static SRWLOCK g_lock = SRWLOCK_INIT;

static uint32_t g_last_appid[MAX_PENDING];
static uint64_t g_last_order[MAX_PENDING], g_last_trans[MAX_PENDING];
static int g_last_n;

void shim_microtxn_track(const char *id, uint64_t order) {
    AcquireSRWLockExclusive(&g_lock);
    uint32_t appid = 0;
    uint64_t trans = 0;
    for (int i = 0; i < g_last_n; i++) if (g_last_order[i] == order) { appid = g_last_appid[i]; trans = g_last_trans[i]; }
    for (int i = 0; i < MAX_PENDING; i++) {
        if (g_pending[i].used) continue;
        g_pending[i].used = true;
        snprintf(g_pending[i].id, sizeof g_pending[i].id, "%s", id);
        g_pending[i].order = order;
        g_pending[i].trans = trans;
        g_pending[i].appid = appid;
        break;
    }
    snprintf(g_owned[g_owned_next++ % MAX_PENDING], sizeof g_owned[0], "%s", id);
    ReleaseSRWLockExclusive(&g_lock);
}

bool shim_microtxn_owns(const char *id) {
    bool owned = false;
    AcquireSRWLockShared(&g_lock);
    for (int i = 0; i < MAX_PENDING; i++) if (g_owned[i][0] && !strcmp(g_owned[i], id)) owned = true;
    ReleaseSRWLockShared(&g_lock);
    return owned;
}

static void remember_app(uint64_t order, uint64_t trans, uint32_t appid) {
    AcquireSRWLockExclusive(&g_lock);
    if (g_last_n < MAX_PENDING) { g_last_order[g_last_n] = order; g_last_trans[g_last_n] = trans; g_last_appid[g_last_n++] = appid; }
    ReleaseSRWLockExclusive(&g_lock);
}

static void set_decision(uint64_t order, int state) {
    AcquireSRWLockExclusive(&g_lock);
    int slot = -1;
    for (int i = 0; i < MAX_PENDING; i++) if (g_dec_order[i] == order) slot = i;
    if (slot < 0) { slot = g_dec_next++ % MAX_PENDING; g_dec_order[slot] = order; }
    g_dec_state[slot] = state;
    ReleaseSRWLockExclusive(&g_lock);
}

int shim_microtxn_wait(uint64_t order, int timeout_ms) {
    for (int waited = 0;; waited += 100) {
        int state = MTX_PENDING;
        AcquireSRWLockShared(&g_lock);
        for (int i = 0; i < MAX_PENDING; i++) if (g_dec_order[i] == order) state = g_dec_state[i];
        ReleaseSRWLockShared(&g_lock);
        if (state != MTX_PENDING || waited >= timeout_ms) return state;
        Sleep(100);
    }
}

static bool take(const char *id, Pending *out) {
    bool found = false;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < MAX_PENDING; i++) {
        if (g_pending[i].used && !strcmp(g_pending[i].id, id)) { *out = g_pending[i]; g_pending[i].used = false; found = true; break; }
    }
    ReleaseSRWLockExclusive(&g_lock);
    return found;
}

static DWORD WINAPI request_info(LPVOID arg) {
    uint64_t *trans = arg;
    CmReply r = {0};
    bool ok = shim_cm_call(EMSG_GET_INFO, false, NULL, (const uint8_t *)trans, 8, 10000, &r);
    size_t shown = ok && r.len < MAX_LOGGED ? r.len : (ok ? MAX_LOGGED : 0);
    char *hex = malloc(shown * 2 + 1);
    if (hex) {
        for (size_t i = 0; i < shown; i++) snprintf(hex + 2 * i, 3, "%02x", r.body[i]);
        shim_log("microtxn: 5508 of transaction %llu -> call=%d eresult=%d reply %d (%lu bytes): %s", (unsigned long long)*trans, ok, r.eresult, r.emsg, (unsigned long)r.len, hex);
        free(hex);
    }
    shim_cm_reply_free(&r);
    free(trans);
    return 0;
}

static void on_auth_request(const uint8_t *body, size_t len) {
    size_t shown = len < MAX_LOGGED ? len : MAX_LOGGED;
    char *hex = malloc(shown * 2 + 1);
    if (hex) {
        for (size_t i = 0; i < shown; i++) snprintf(hex + 2 * i, 3, "%02x", body[i]);
        shim_log("microtxn: push %d (%lu bytes): %s%s", EMSG_AUTH_REQUEST, (unsigned long)len, hex, shown < len ? "..." : "");
        free(hex);
    }
    Order o;
    if (!parse_order(body, len, &o)) { shim_log("microtxn: order body not recognized; kept in the log only"); return; }
    char *json = malloc(8192);
    if (!json) return;
    build_json(&o, json, 8192);
    remember_app(o.order_id, o.trans_id, (uint32_t)o.appid);
    shim_log("microtxn: order %llu of app %d sent to the launcher", (unsigned long long)o.order_id, o.appid);
    shim_overlay_submit("microtxn", json, o.order_id);
    free(json);
    uint64_t *trans = malloc(8);
    if (trans) { *trans = o.trans_id; HANDLE t = CreateThread(NULL, 0, request_info, trans, 0, NULL); if (t) CloseHandle(t); else free(trans); }
}

typedef struct { uint64_t order, trans; uint32_t appid; int response; } Answer;

static void post_auth_response(void *a, void *b, int authorized) {
    (void)b;
    Answer *an = a;
    uint8_t cb[24] = {0};
    memcpy(cb, &an->appid, 4);
    memcpy(cb + 8, &an->order, 8);
    cb[16] = (uint8_t)authorized;
    shim_post_callback(CB_AUTH_RESPONSE, cb, sizeof cb);
    shim_log("microtxn: callback %d delivered to the game (app %u, order %llu, authorized=%d)", CB_AUTH_RESPONSE, an->appid, (unsigned long long)an->order, authorized);
    free(an);
}

static DWORD WINAPI send_answer(LPVOID arg) {
    Answer *an = arg;
    uint8_t body[12];
    uint32_t resp = (uint32_t)an->response;
    memcpy(body, &an->trans, 8);
    memcpy(body + 8, &resp, 4);
    CmReply r = {0};
    bool ok = shim_cm_call(EMSG_AUTHORIZE, false, NULL, body, sizeof body, 15000, &r);
    int32_t eresult = ok && r.len >= 4 ? (int32_t)(r.body[0] | r.body[1] << 8 | r.body[2] << 16 | (uint32_t)r.body[3] << 24) : -1;
    shim_log("microtxn: response %u for transaction %llu (order %llu) sent (call=%d, Steam result=%d)", resp, (unsigned long long)an->trans, (unsigned long long)an->order, ok, eresult);
    shim_cm_reply_free(&r);
    bool delivered = ok && (eresult == 1 || an->response == 2);
    set_decision(an->order, delivered && an->response == 1 ? MTX_APPROVED : MTX_DENIED);
    shim_overlay_activated_now(0);
    if (delivered) {
        Sleep(230);
        post_auth_response(an, NULL, an->response == 1);
    } else {
        free(an);
    }
    return 0;
}

void shim_microtxn_answer(const char *id, int approved, int denied_by_steam) {
    Pending p;
    if (!take(id, &p)) { shim_log("microtxn: response for an unknown order (%s)", id); return; }
    if (denied_by_steam) { shim_log("microtxn: Steam withdrew order %llu", (unsigned long long)p.order); set_decision(p.order, MTX_DENIED); shim_overlay_activated(0); return; }
    Answer *an = calloc(1, sizeof *an);
    if (!an) return;
    an->order = p.order;
    an->trans = p.trans;
    an->appid = p.appid;
    an->response = approved ? 1 : 2;
    HANDLE t = CreateThread(NULL, 0, send_answer, an, 0, NULL);
    if (t) CloseHandle(t); else free(an);
}

static void on_other_push(const uint8_t *body, size_t len, int emsg) {
    size_t shown = len < MAX_LOGGED ? len : MAX_LOGGED;
    char *hex = malloc(shown * 2 + 1);
    if (!hex) return;
    for (size_t i = 0; i < shown; i++) snprintf(hex + 2 * i, 3, "%02x", body[i]);
    shim_log("microtxn: push %d (%lu bytes): %s", emsg, (unsigned long)len, hex);
    free(hex);
}
static void on_authorize_response(const uint8_t *body, size_t len) { on_other_push(body, len, EMSG_AUTHORIZE_RESPONSE); }
static void on_info_response(const uint8_t *body, size_t len) { on_other_push(body, len, EMSG_INFO_RESPONSE); }

void shim_microtxn_capture_start(void) {
    static volatile LONG started;
    if (InterlockedCompareExchange(&started, 1, 0)) return;
    shim_cm_on_push(EMSG_AUTH_REQUEST, on_auth_request);
    shim_cm_on_push(EMSG_AUTHORIZE_RESPONSE, on_authorize_response);
    shim_cm_on_push(EMSG_INFO_RESPONSE, on_info_response);
}
