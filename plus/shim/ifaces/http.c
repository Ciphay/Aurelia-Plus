#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>
#include <winhttp.h>

#include "../shim.h"

#define MAX_REQ 64
#define MAX_HEADERS 24
#define MAX_PARAMS 48

typedef struct { char name[128]; char value[1024]; } Pair;
typedef struct {
    bool used, sent, done, ok, timed_out, verify;
    int method;
    char url[2048];
    Pair headers[MAX_HEADERS]; int n_headers;
    Pair params[MAX_PARAMS]; int n_params;
    uint8_t *body; uint32_t body_len;
    char content_type[128];
    uint64_t context, call;
    uint32_t timeout_ms;
    int status;
    uint8_t *resp; uint32_t resp_len;
    char *resp_headers;
} Req;

static Req g_req[MAX_REQ];
static SRWLOCK g_lock = SRWLOCK_INIT;
static uint32_t g_next_cookie = 1;

static Req *get(uint32_t h) { return h >= 1 && h <= MAX_REQ && g_req[h - 1].used ? &g_req[h - 1] : NULL; }

static const char *verb(int m) {
    static const char *const v[] = { "GET", "GET", "HEAD", "POST", "PUT", "DELETE", "OPTIONS", "PATCH" };
    return m >= 0 && m < 8 ? v[m] : "GET";
}

static wchar_t *wide(const char *s) {
    int n = MultiByteToWideChar(CP_UTF8, 0, s, -1, NULL, 0);
    wchar_t *w = malloc((size_t)(n > 0 ? n : 1) * sizeof *w);
    if (w) { if (n > 0) MultiByteToWideChar(CP_UTF8, 0, s, -1, w, n); else w[0] = 0; }
    return w;
}

static void url_encode(const char *in, char *out, size_t cap) {
    size_t o = 0;
    for (; *in && o + 4 < cap; in++) {
        unsigned char c = (unsigned char)*in;
        if ((c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c == '-' || c == '_' || c == '.' || c == '~') out[o++] = (char)c;
        else o += (size_t)snprintf(out + o, cap - o, "%%%02X", c);
    }
    out[o] = 0;
}

static void complete(Req *r, uint32_t handle) {
    uint8_t cb[32] = {0};
    int32_t status = r->status;
    memcpy(cb, &handle, 4);
    memcpy(cb + 8, &r->context, 8);
    cb[16] = r->ok;
    memcpy(cb + 20, &status, 4);
    memcpy(cb + 24, &r->resp_len, 4);
    r->done = true;
    shim_callresult_complete(r->call, 2101, cb, sizeof cb);
}

static DWORD WINAPI worker(LPVOID arg) {
    uint32_t handle = (uint32_t)(uintptr_t)arg;
    AcquireSRWLockShared(&g_lock);
    Req *r = get(handle);
    ReleaseSRWLockShared(&g_lock);
    if (!r) return 0;

    char url[4096];
    snprintf(url, sizeof url, "%s", r->url);
    char form[4096] = "";
    for (int i = 0; i < r->n_params; i++) {
        char k[512], v[1536];
        url_encode(r->params[i].name, k, sizeof k);
        url_encode(r->params[i].value, v, sizeof v);
        size_t len = strlen(form);
        snprintf(form + len, sizeof form - len, "%s%s=%s", len ? "&" : "", k, v);
    }
    const uint8_t *body = r->body;
    uint32_t body_len = r->body_len;
    bool is_get = r->method <= 1 || r->method == 2;
    if (form[0] && is_get) { size_t len = strlen(url); snprintf(url + len, sizeof url - len, "%s%s", strchr(url, '?') ? "&" : "?", form); }
    else if (form[0] && !body) { body = (const uint8_t *)form; body_len = (uint32_t)strlen(form); }

    wchar_t *wurl = wide(url);
    URL_COMPONENTS uc = {0};
    wchar_t host[256] = L"", path[3072] = L"";
    uc.dwStructSize = sizeof uc;
    uc.lpszHostName = host; uc.dwHostNameLength = 256;
    uc.lpszUrlPath = path; uc.dwUrlPathLength = 3072;
    uc.dwSchemeLength = (DWORD)-1;
    bool parsed = wurl && WinHttpCrackUrl(wurl, 0, 0, &uc);
    HINTERNET session = NULL, conn = NULL, req = NULL;
    bool ok = false;
    int status = 0;
    uint8_t *buf = NULL;
    uint32_t len = 0, cap = 0;
    char *raw_headers = NULL;
    if (parsed) {
        session = WinHttpOpen(L"Valve/Steam HTTP Client 1.0", WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, NULL, NULL, 0);
        conn = session ? WinHttpConnect(session, host, uc.nPort, 0) : NULL;
        wchar_t *wverb = wide(verb(r->method));
        req = conn ? WinHttpOpenRequest(conn, wverb, path, NULL, NULL, NULL, uc.nScheme == INTERNET_SCHEME_HTTPS ? WINHTTP_FLAG_SECURE : 0) : NULL;
        free(wverb);
    }
    if (req) {
        DWORD ms = r->timeout_ms ? r->timeout_ms : 60000;
        WinHttpSetTimeouts(req, 10000, 10000, (int)ms, (int)ms);
        if (!r->verify) {
            DWORD flags = SECURITY_FLAG_IGNORE_UNKNOWN_CA | SECURITY_FLAG_IGNORE_CERT_DATE_INVALID | SECURITY_FLAG_IGNORE_CERT_CN_INVALID;
            WinHttpSetOption(req, WINHTTP_OPTION_SECURITY_FLAGS, &flags, sizeof flags);
        }
        char hdrs[8192] = "";
        for (int i = 0; i < r->n_headers; i++) { size_t l = strlen(hdrs); snprintf(hdrs + l, sizeof hdrs - l, "%s: %s\r\n", r->headers[i].name, r->headers[i].value); }
        if (body && r->content_type[0]) { size_t l = strlen(hdrs); snprintf(hdrs + l, sizeof hdrs - l, "Content-Type: %s\r\n", r->content_type); }
        else if (body && form[0]) { size_t l = strlen(hdrs); snprintf(hdrs + l, sizeof hdrs - l, "Content-Type: application/x-www-form-urlencoded\r\n"); }
        wchar_t *whdrs = wide(hdrs);
        BOOL sent = WinHttpSendRequest(req, hdrs[0] ? whdrs : WINHTTP_NO_ADDITIONAL_HEADERS, hdrs[0] ? (DWORD)-1 : 0, (LPVOID)body, body_len, body_len, 0);
        free(whdrs);
        if (sent && WinHttpReceiveResponse(req, NULL)) {
            DWORD code = 0, sz = sizeof code;
            if (WinHttpQueryHeaders(req, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, NULL, &code, &sz, NULL)) status = (int)code;
            DWORD hsz = 0;
            WinHttpQueryHeaders(req, WINHTTP_QUERY_RAW_HEADERS_CRLF, NULL, NULL, &hsz, NULL);
            if (hsz) {
                wchar_t *wh = malloc(hsz + 2);
                if (wh && WinHttpQueryHeaders(req, WINHTTP_QUERY_RAW_HEADERS_CRLF, NULL, wh, &hsz, NULL)) {
                    int n = WideCharToMultiByte(CP_UTF8, 0, wh, -1, NULL, 0, NULL, NULL);
                    raw_headers = malloc((size_t)(n > 0 ? n : 1));
                    if (raw_headers) { if (n > 0) WideCharToMultiByte(CP_UTF8, 0, wh, -1, raw_headers, n, NULL, NULL); else raw_headers[0] = 0; }
                }
                free(wh);
            }
            ok = true;
            for (;;) {
                DWORD avail = 0, got = 0;
                if (!WinHttpQueryDataAvailable(req, &avail)) { ok = false; break; }
                if (!avail) break;
                if (len + avail > cap) { cap = (len + avail) * 2; uint8_t *nb = realloc(buf, cap); if (!nb) { ok = false; break; } buf = nb; }
                if (!WinHttpReadData(req, buf + len, avail, &got)) { ok = false; break; }
                len += got;
            }
        }
    }
    if (req) WinHttpCloseHandle(req);
    if (conn) WinHttpCloseHandle(conn);
    if (session) WinHttpCloseHandle(session);
    free(wurl);

    AcquireSRWLockExclusive(&g_lock);
    r = get(handle);
    if (r) {
        r->ok = ok; r->status = ok ? status : 0; r->resp = buf; r->resp_len = len; r->resp_headers = raw_headers;
        r->timed_out = !ok && GetLastError() == ERROR_WINHTTP_TIMEOUT;
        complete(r, handle);
    } else { free(buf); free(raw_headers); }
    ReleaseSRWLockExclusive(&g_lock);
    return 0;
}

static uint32_t SHIM_TC create_request(void *self, int method, const char *url) {
    (void)self;
    if (!url || method <= 0 || method > 7) return 0;
    uint32_t h = 0;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < MAX_REQ; i++) {
        if (g_req[i].used) continue;
        memset(&g_req[i], 0, sizeof g_req[i]);
        g_req[i].used = true; g_req[i].method = method; g_req[i].verify = true;
        snprintf(g_req[i].url, sizeof g_req[i].url, "%s", url);
        h = (uint32_t)(i + 1);
        break;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return h;
}

#define WITH_REQ(h, expr) do { AcquireSRWLockExclusive(&g_lock); Req *r = get(h); bool ok = r && !r->sent; if (ok) { expr; } ReleaseSRWLockExclusive(&g_lock); return ok; } while (0)

static uint64_t SHIM_TC set_context(void *self, uint32_t h, uint64_t v) { (void)self; WITH_REQ(h, r->context = v); }
static uint64_t SHIM_TC set_net_timeout(void *self, uint32_t h, uint32_t sec) { (void)self; WITH_REQ(h, r->timeout_ms = sec * 1000); }
static uint64_t SHIM_TC set_abs_timeout(void *self, uint32_t h, uint32_t ms) { (void)self; WITH_REQ(h, r->timeout_ms = ms); }
static uint64_t SHIM_TC set_header(void *self, uint32_t h, const char *name, const char *value) {
    (void)self;
    WITH_REQ(h, if (name && r->n_headers < MAX_HEADERS) { snprintf(r->headers[r->n_headers].name, 128, "%s", name); snprintf(r->headers[r->n_headers].value, 1024, "%s", value ? value : ""); r->n_headers++; } else ok = false);
}
static uint64_t SHIM_TC set_param(void *self, uint32_t h, const char *name, const char *value) {
    (void)self;
    WITH_REQ(h, if (name && r->n_params < MAX_PARAMS) { snprintf(r->params[r->n_params].name, 128, "%s", name); snprintf(r->params[r->n_params].value, 1024, "%s", value ? value : ""); r->n_params++; } else ok = false);
}
static uint64_t SHIM_TC set_raw_body(void *self, uint32_t h, const char *content_type, const uint8_t *body, uint32_t len) {
    (void)self;
    WITH_REQ(h, free(r->body); r->body = malloc(len ? len : 1); if (r->body && body) { memcpy(r->body, body, len); r->body_len = len; snprintf(r->content_type, sizeof r->content_type, "%s", content_type ? content_type : ""); } else ok = false);
}
static uint64_t SHIM_TC set_verify(void *self, uint32_t h, uint8_t v) { (void)self; WITH_REQ(h, r->verify = v); }
static uint64_t SHIM_TC set_user_agent(void *self, uint32_t h, const char *info) { (void)self; (void)info; WITH_REQ(h, (void)r); }
static uint64_t SHIM_TC set_cookie_container(void *self, uint32_t h, uint32_t cookies) { (void)self; (void)cookies; WITH_REQ(h, (void)r); }

static uint64_t SHIM_TC send_request(void *self, uint32_t h, uint64_t *call) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Req *r = get(h);
    bool ok = r && !r->sent;
    if (ok) {
        r->sent = true;
        r->call = shim_callresult_reserve();
        if (call) *call = r->call;
        HANDLE t = CreateThread(NULL, 0, worker, (LPVOID)(uintptr_t)h, 0, NULL);
        if (t) CloseHandle(t); else { r->ok = false; complete(r, h); }
    }
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}
static uint64_t SHIM_TC true_for_valid(void *self, uint32_t h) { (void)self; AcquireSRWLockShared(&g_lock); bool ok = get(h) != NULL; ReleaseSRWLockShared(&g_lock); return ok; }

static uint64_t SHIM_TC release_request(void *self, uint32_t h) {
    (void)self;
    AcquireSRWLockExclusive(&g_lock);
    Req *r = get(h);
    bool ok = r != NULL;
    if (ok && (!r->sent || r->done)) { free(r->body); free(r->resp); free(r->resp_headers); memset(r, 0, sizeof *r); }
    else if (ok) r->used = false;
    ReleaseSRWLockExclusive(&g_lock);
    return ok;
}

static const char *header_value(const char *raw, const char *name, size_t *len) {
    size_t n = strlen(name);
    for (const char *line = raw; line && *line; ) {
        const char *eol = strstr(line, "\r\n");
        size_t ll = eol ? (size_t)(eol - line) : strlen(line);
        if (ll > n && !_strnicmp(line, name, n) && line[n] == ':') {
            const char *v = line + n + 1;
            while (v < line + ll && *v == ' ') v++;
            *len = (size_t)(line + ll - v);
            return v;
        }
        line = eol ? eol + 2 : NULL;
    }
    return NULL;
}
static uint64_t SHIM_TC response_header_size(void *self, uint32_t h, const char *name, uint32_t *size) {
    (void)self;
    AcquireSRWLockShared(&g_lock);
    Req *r = get(h);
    size_t l = 0;
    const char *v = r && r->done && r->resp_headers && name ? header_value(r->resp_headers, name, &l) : NULL;
    if (v && size) *size = (uint32_t)l + 1;
    ReleaseSRWLockShared(&g_lock);
    return v != NULL;
}
static uint64_t SHIM_TC response_header_value(void *self, uint32_t h, const char *name, uint8_t *out, uint32_t cap) {
    (void)self;
    AcquireSRWLockShared(&g_lock);
    Req *r = get(h);
    size_t l = 0;
    const char *v = r && r->done && r->resp_headers && name ? header_value(r->resp_headers, name, &l) : NULL;
    bool ok = v && out && cap >= l + 1;
    if (ok) { memcpy(out, v, l); out[l] = 0; }
    ReleaseSRWLockShared(&g_lock);
    return ok;
}
static uint64_t SHIM_TC body_size(void *self, uint32_t h, uint32_t *size) {
    (void)self;
    AcquireSRWLockShared(&g_lock);
    Req *r = get(h);
    bool ok = r && r->done && r->ok;
    if (ok && size) *size = r->resp_len;
    ReleaseSRWLockShared(&g_lock);
    return ok;
}
static uint64_t SHIM_TC body_data(void *self, uint32_t h, uint8_t *out, uint32_t cap) {
    (void)self;
    AcquireSRWLockShared(&g_lock);
    Req *r = get(h);
    bool ok = r && r->done && r->ok && out && cap >= r->resp_len;
    if (ok && r->resp_len) memcpy(out, r->resp, r->resp_len);
    ReleaseSRWLockShared(&g_lock);
    return ok;
}
static uint64_t SHIM_TC streaming_body_data(void *self, uint32_t h, uint32_t offset, uint8_t *out, uint32_t cap) {
    (void)self;
    AcquireSRWLockShared(&g_lock);
    Req *r = get(h);
    bool ok = r && r->done && r->ok && out && offset <= r->resp_len && cap >= r->resp_len - offset;
    if (ok && r->resp_len > offset) memcpy(out, r->resp + offset, r->resp_len - offset);
    ReleaseSRWLockShared(&g_lock);
    return ok;
}
static uint64_t SHIM_TC download_progress(void *self, uint32_t h, float *pct) {
    (void)self;
    AcquireSRWLockShared(&g_lock);
    Req *r = get(h);
    bool ok = r && r->sent;
    if (ok && pct) *pct = r->done ? 100.0f : 0.0f;
    ReleaseSRWLockShared(&g_lock);
    return ok;
}
static uint64_t SHIM_TC was_timed_out(void *self, uint32_t h, uint8_t *timed_out) {
    (void)self;
    AcquireSRWLockShared(&g_lock);
    Req *r = get(h);
    bool ok = r && r->done;
    if (ok && timed_out) *timed_out = r->timed_out;
    ReleaseSRWLockShared(&g_lock);
    return ok;
}

static uint32_t SHIM_TC create_cookie_container(void *self, uint8_t allow_modify) { (void)self; (void)allow_modify; return g_next_cookie++; }
static uint64_t SHIM_TC release_cookie_container(void *self, uint32_t h) { (void)self; return h != 0 && h < g_next_cookie; }
static uint64_t SHIM_TC set_cookie(void *self, uint32_t h, const char *host, const char *url, const char *cookie) { (void)self; (void)host; (void)url; (void)cookie; return h != 0 && h < g_next_cookie; }

void register_http(void) {
    const char *p = "STEAMHTTP_INTERFACE";
    shim_register_family(p, "CreateHTTPRequest", (void *)create_request);
    shim_register_family(p, "SetHTTPRequestContextValue", (void *)set_context);
    shim_register_family(p, "SetHTTPRequestNetworkActivityTimeout", (void *)set_net_timeout);
    shim_register_family(p, "SetHTTPRequestAbsoluteTimeoutMS", (void *)set_abs_timeout);
    shim_register_family(p, "SetHTTPRequestHeaderValue", (void *)set_header);
    shim_register_family(p, "SetHTTPRequestGetOrPostParameter", (void *)set_param);
    shim_register_family(p, "SetHTTPRequestRawPostBody", (void *)set_raw_body);
    shim_register_family(p, "SetHTTPRequestRequiresVerifiedCertificate", (void *)set_verify);
    shim_register_family(p, "SetHTTPRequestUserAgentInfo", (void *)set_user_agent);
    shim_register_family(p, "SetHTTPRequestCookieContainer", (void *)set_cookie_container);
    shim_register_family(p, "SendHTTPRequest", (void *)send_request);
    shim_register_family(p, "SendHTTPRequestAndStreamResponse", (void *)send_request);
    shim_register_family(p, "DeferHTTPRequest", (void *)true_for_valid);
    shim_register_family(p, "PrioritizeHTTPRequest", (void *)true_for_valid);
    shim_register_family(p, "ReleaseHTTPRequest", (void *)release_request);
    shim_register_family(p, "GetHTTPResponseHeaderSize", (void *)response_header_size);
    shim_register_family(p, "GetHTTPResponseHeaderValue", (void *)response_header_value);
    shim_register_family(p, "GetHTTPResponseBodySize", (void *)body_size);
    shim_register_family(p, "GetHTTPResponseBodyData", (void *)body_data);
    shim_register_family(p, "GetHTTPStreamingResponseBodyData", (void *)streaming_body_data);
    shim_register_family(p, "GetHTTPDownloadProgressPct", (void *)download_progress);
    shim_register_family(p, "GetHTTPRequestWasTimedOut", (void *)was_timed_out);
    shim_register_family(p, "CreateCookieContainer", (void *)create_cookie_container);
    shim_register_family(p, "ReleaseCookieContainer", (void *)release_cookie_container);
    shim_register_family(p, "SetCookie", (void *)set_cookie);
}
