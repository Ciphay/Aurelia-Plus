#include <stdlib.h>
#include <string.h>

#include <stdio.h>

#include "shim.h"

typedef struct { const char *p, *end; } Cur;
typedef bool (*KeyFn)(Cur *c, const char *key, void *ctx);

static void ws(Cur *c) { while (c->p < c->end && (*c->p == ' ' || *c->p == '\n' || *c->p == '\t' || *c->p == '\r')) c->p++; }
static bool eat(Cur *c, char ch) { ws(c); if (c->p < c->end && *c->p == ch) { c->p++; return true; } return false; }
static bool word(Cur *c, const char *w) {
    size_t n = strlen(w);
    ws(c);
    if ((size_t)(c->end - c->p) >= n && memcmp(c->p, w, n) == 0) { c->p += n; return true; }
    return false;
}

static int hex4(Cur *c) {
    if (c->end - c->p < 4) return -1;
    int v = 0;
    for (int i = 0; i < 4; i++) {
        char ch = c->p[i];
        v <<= 4;
        if (ch >= '0' && ch <= '9') v |= ch - '0';
        else if (ch >= 'a' && ch <= 'f') v |= ch - 'a' + 10;
        else if (ch >= 'A' && ch <= 'F') v |= ch - 'A' + 10;
        else return -1;
    }
    c->p += 4;
    return v;
}

static size_t utf8(unsigned cp, char *o) {
    if (cp < 0x80) { o[0] = (char)cp; return 1; }
    if (cp < 0x800) { o[0] = (char)(0xC0 | (cp >> 6)); o[1] = (char)(0x80 | (cp & 0x3F)); return 2; }
    if (cp < 0x10000) { o[0] = (char)(0xE0 | (cp >> 12)); o[1] = (char)(0x80 | ((cp >> 6) & 0x3F)); o[2] = (char)(0x80 | (cp & 0x3F)); return 3; }
    o[0] = (char)(0xF0 | (cp >> 18)); o[1] = (char)(0x80 | ((cp >> 12) & 0x3F)); o[2] = (char)(0x80 | ((cp >> 6) & 0x3F)); o[3] = (char)(0x80 | (cp & 0x3F));
    return 4;
}

static char *str(Cur *c) {
    if (!eat(c, '"')) return NULL;
    size_t cap = (size_t)(c->end - c->p) + 1, n = 0;
    char *out = malloc(cap);
    if (!out) return NULL;
    while (c->p < c->end && *c->p != '"') {
        char ch = *c->p++;
        if (ch != '\\') { out[n++] = ch; continue; }
        if (c->p >= c->end) goto bad;
        char e = *c->p++;
        switch (e) {
        case 'n': out[n++] = '\n'; break;
        case 't': out[n++] = '\t'; break;
        case 'r': out[n++] = '\r'; break;
        case 'b': out[n++] = '\b'; break;
        case 'f': out[n++] = '\f'; break;
        case 'u': {
            int cp = hex4(c);
            if (cp < 0) goto bad;
            if (cp >= 0xD800 && cp < 0xDC00 && c->end - c->p >= 6 && c->p[0] == '\\' && c->p[1] == 'u') {
                c->p += 2;
                int lo = hex4(c);
                if (lo < 0) goto bad;
                cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
            }
            n += utf8((unsigned)cp, out + n);
            break;
        }
        default: out[n++] = e;
        }
    }
    if (c->p >= c->end) goto bad;
    c->p++;
    out[n] = 0;
    return out;
bad:
    free(out);
    return NULL;
}

static bool num(Cur *c, uint64_t *out) {
    ws(c);
    if (c->p >= c->end || *c->p < '0' || *c->p > '9') return false;
    uint64_t v = 0;
    while (c->p < c->end && *c->p >= '0' && *c->p <= '9') v = v * 10 + (uint64_t)(*c->p++ - '0');
    *out = v;
    return true;
}

static bool skip(Cur *c);
static bool obj(Cur *c, KeyFn fn, void *ctx) {
    if (!eat(c, '{')) return false;
    if (eat(c, '}')) return true;
    for (;;) {
        char *key = str(c);
        if (!key) return false;
        bool ok = eat(c, ':') && fn(c, key, ctx);
        free(key);
        if (!ok) return false;
        if (eat(c, ',')) continue;
        return eat(c, '}');
    }
}

static bool skip(Cur *c) {
    ws(c);
    if (c->p >= c->end) return false;
    if (*c->p == '"') { char *s = str(c); free(s); return s != NULL; }
    if (*c->p == '{' || *c->p == '[') {
        char open = *c->p, close = open == '{' ? '}' : ']';
        int depth = 0;
        while (c->p < c->end) {
            if (*c->p == '"') { char *s = str(c); if (!s) return false; free(s); continue; }
            if (*c->p == open) depth++;
            else if (*c->p == close && --depth == 0) { c->p++; return true; }
            c->p++;
        }
        return false;
    }
    if (word(c, "true") || word(c, "false") || word(c, "null")) return true;
    uint64_t v;
    if (num(c, &v)) { while (c->p < c->end && (*c->p == '.' || *c->p == 'e' || *c->p == 'E' || *c->p == '+' || *c->p == '-' || (*c->p >= '0' && *c->p <= '9'))) c->p++; return true; }
    if (c->p < c->end && *c->p == '-') { c->p++; while (c->p < c->end && ((*c->p >= '0' && *c->p <= '9') || *c->p == '.')) c->p++; return true; }
    return false;
}

static int b64v(char ch) {
    if (ch >= 'A' && ch <= 'Z') return ch - 'A';
    if (ch >= 'a' && ch <= 'z') return ch - 'a' + 26;
    if (ch >= '0' && ch <= '9') return ch - '0' + 52;
    if (ch == '+') return 62;
    if (ch == '/') return 63;
    return -1;
}

static bool b64(const char *s, uint8_t **out, size_t *len) {
    size_t n = strlen(s);
    uint8_t *buf = malloc(n / 4 * 3 + 3);
    if (!buf) return false;
    size_t o = 0;
    uint32_t acc = 0;
    int bits = 0;
    for (size_t i = 0; i < n && s[i] != '='; i++) {
        int v = b64v(s[i]);
        if (v < 0) { free(buf); return false; }
        acc = (acc << 6) | (uint32_t)v;
        bits += 6;
        if (bits >= 8) { bits -= 8; buf[o++] = (uint8_t)(acc >> bits); }
    }
    *out = buf;
    *len = o;
    return true;
}

static void copy_cstr(char *dst, size_t cap, const char *src) { strncpy(dst, src, cap - 1); dst[cap - 1] = 0; }

static bool account_key(Cur *c, const char *k, void *ctx) {
    Snapshot *s = ctx;
    if (!strcmp(k, "steamid")) return num(c, &s->steamid);
    if (!strcmp(k, "persona") || !strcmp(k, "country")) {
        char *v = str(c);
        if (!v) return false;
        if (k[0] == 'p') copy_cstr(s->persona, sizeof s->persona, v); else copy_cstr(s->country, sizeof s->country, v);
        free(v);
        return true;
    }
    return skip(c);
}

static bool app_key(Cur *c, const char *k, void *ctx) {
    ShimApp *a = ctx;
    uint64_t v;
    if (!strcmp(k, "appid")) { if (!num(c, &v)) return false; a->appid = (uint32_t)v; return true; }
    if (!strcmp(k, "name")) { free(a->name); a->name = str(c); return a->name != NULL; }
    if (!strcmp(k, "installed")) { if (word(c, "true")) a->installed = true; else if (word(c, "false")) a->installed = false; else return false; return true; }
    return skip(c);
}

static bool apps_array(Cur *c, Snapshot *s) {
    if (!eat(c, '[')) return false;
    if (eat(c, ']')) return true;
    size_t cap = 0;
    for (;;) {
        if (s->n_apps == cap) {
            cap = cap ? cap * 2 : 64;
            ShimApp *grown = realloc(s->apps, cap * sizeof *grown);
            if (!grown) return false;
            s->apps = grown;
        }
        ShimApp *a = &s->apps[s->n_apps];
        memset(a, 0, sizeof *a);
        s->n_apps++;
        if (!obj(c, app_key, a)) return false;
        if (eat(c, ',')) continue;
        return eat(c, ']');
    }
}

static bool ticket_key(Cur *c, const char *k, void *ctx) {
    ShimTicket *t = ctx;
    uint64_t v;
    if (!strcmp(k, "appid")) { if (!num(c, &v)) return false; t->appid = (uint32_t)v; return true; }
    if (!strcmp(k, "ownership_ticket_b64") || !strcmp(k, "game_connect_token_b64")) {
        char *s = str(c);
        if (!s) return false;
        bool own = k[0] == 'o';
        bool ok = own ? b64(s, &t->ownership, &t->ownership_len) : b64(s, &t->token, &t->token_len);
        free(s);
        return ok;
    }
    return skip(c);
}

static bool bool_val(Cur *c, bool *out) {
    if (word(c, "true")) { *out = true; return true; }
    if (word(c, "false")) { *out = false; return true; }
    return false;
}

static bool dlc_key(Cur *c, const char *k, void *ctx) {
    ShimDlc *d = ctx;
    uint64_t v;
    if (!strcmp(k, "appid")) { if (!num(c, &v)) return false; d->appid = (uint32_t)v; return true; }
    if (!strcmp(k, "owned")) return bool_val(c, &d->owned);
    if (!strcmp(k, "installed")) return bool_val(c, &d->installed);
    if (!strcmp(k, "name")) { char *n = str(c); if (!n) return false; snprintf(d->name, sizeof d->name, "%s", n); free(n); return true; }
    return skip(c);
}

static bool dlc_array(Cur *c, ShimAppInfo *a) {
    if (!eat(c, '[')) return false;
    if (eat(c, ']')) return true;
    size_t cap = 0;
    for (;;) {
        if (a->n_dlc == cap) {
            cap = cap ? cap * 2 : 16;
            ShimDlc *grown = realloc(a->dlc, cap * sizeof *grown);
            if (!grown) return false;
            a->dlc = grown;
        }
        ShimDlc *d = &a->dlc[a->n_dlc];
        memset(d, 0, sizeof *d);
        a->n_dlc++;
        if (!obj(c, dlc_key, d)) return false;
        if (eat(c, ',')) continue;
        return eat(c, ']');
    }
}

static bool beta_key(Cur *c, const char *k, void *ctx) {
    ShimBeta *b = ctx;
    uint64_t v;
    if (!strcmp(k, "name")) { free(b->name); b->name = str(c); return b->name != NULL; }
    if (!strcmp(k, "description")) { free(b->description); b->description = str(c); return b->description != NULL; }
    if (!strcmp(k, "build_id")) { if (!num(c, &v)) return false; b->build_id = (uint32_t)v; return true; }
    if (!strcmp(k, "time_updated")) { if (!num(c, &v)) return false; b->time_updated = (uint32_t)v; return true; }
    if (!strcmp(k, "private")) return bool_val(c, &b->priv);
    return skip(c);
}

static bool beta_array(Cur *c, ShimAppInfo *a) {
    if (!eat(c, '[')) return false;
    if (eat(c, ']')) return true;
    size_t cap = 0;
    for (;;) {
        if (a->n_betas == cap) {
            cap = cap ? cap * 2 : 8;
            ShimBeta *grown = realloc(a->betas, cap * sizeof *grown);
            if (!grown) return false;
            a->betas = grown;
        }
        ShimBeta *b = &a->betas[a->n_betas];
        memset(b, 0, sizeof *b);
        a->n_betas++;
        if (!obj(c, beta_key, b)) return false;
        if (eat(c, ',')) continue;
        return eat(c, ']');
    }
}

static bool depot_array(Cur *c, ShimAppInfo *a) {
    if (!eat(c, '[')) return false;
    if (eat(c, ']')) return true;
    size_t cap = 0;
    for (;;) {
        uint64_t v;
        if (!num(c, &v)) return false;
        if (a->n_depots == cap) {
            cap = cap ? cap * 2 : 16;
            uint32_t *grown = realloc(a->depots, cap * sizeof *grown);
            if (!grown) return false;
            a->depots = grown;
        }
        a->depots[a->n_depots++] = (uint32_t)v;
        if (eat(c, ',')) continue;
        return eat(c, ']');
    }
}

static bool app_info_key(Cur *c, const char *k, void *ctx) {
    ShimAppInfo *a = ctx;
    uint64_t v;
    if (!strcmp(k, "appid")) { if (!num(c, &v)) return false; a->appid = (uint32_t)v; return true; }
    if (!strcmp(k, "install_dir")) { free(a->install_dir); a->install_dir = str(c); return a->install_dir != NULL; }
    if (!strcmp(k, "cloud_quota")) return num(c, &a->cloud_quota);
    if (!strcmp(k, "build_id")) return num(c, &a->build_id);
    if (!strcmp(k, "purchase_time")) { if (!num(c, &v)) return false; a->purchase_time = (uint32_t)v; return true; }
    if (!strcmp(k, "beta_name")) { free(a->beta_name); a->beta_name = str(c); return a->beta_name != NULL; }
    if (!strcmp(k, "cloud_dir")) { free(a->cloud_dir); a->cloud_dir = str(c); return a->cloud_dir != NULL; }
    if (!strcmp(k, "dlc")) return dlc_array(c, a);
    if (!strcmp(k, "installed_depots")) return depot_array(c, a);
    if (!strcmp(k, "betas")) return beta_array(c, a);
    return skip(c);
}

static bool friend_key(Cur *c, const char *k, void *ctx) {
    ShimFriend *f = ctx;
    uint64_t v;
    if (!strcmp(k, "steamid")) return num(c, &f->steamid);
    if (!strcmp(k, "persona")) { free(f->persona); f->persona = str(c); return f->persona != NULL; }
    if (!strcmp(k, "state")) { if (!num(c, &v)) return false; f->state = (uint32_t)v; return true; }
    if (!strcmp(k, "game_appid")) { if (!num(c, &v)) return false; f->game_appid = (uint32_t)v; return true; }
    return skip(c);
}

static bool friends_array(Cur *c, Snapshot *s) {
    if (!eat(c, '[')) return false;
    if (eat(c, ']')) return true;
    size_t cap = 0;
    for (;;) {
        if (s->n_friends == cap) {
            cap = cap ? cap * 2 : 32;
            ShimFriend *grown = realloc(s->friends, cap * sizeof *grown);
            if (!grown) return false;
            s->friends = grown;
        }
        ShimFriend *f = &s->friends[s->n_friends];
        memset(f, 0, sizeof *f);
        s->n_friends++;
        if (!obj(c, friend_key, f)) return false;
        if (eat(c, ',')) continue;
        return eat(c, ']');
    }
}

static bool top_key(Cur *c, const char *k, void *ctx) {
    Snapshot *s = ctx;
    uint64_t v;
    if (!strcmp(k, "schema")) { if (!num(c, &v)) return false; s->schema = (int)v; return true; }
    if (!strcmp(k, "account")) return obj(c, account_key, s);
    if (!strcmp(k, "apps")) return apps_array(c, s);
    if (!strcmp(k, "ticket")) {
        if (word(c, "null")) return true;
        s->has_ticket = true;
        return obj(c, ticket_key, &s->ticket);
    }
    if (!strcmp(k, "friends")) return friends_array(c, s);
    if (!strcmp(k, "online")) return bool_val(c, &s->online);
    if (!strcmp(k, "app")) {
        if (word(c, "null")) return true;
        s->has_app = true;
        return obj(c, app_info_key, &s->app);
    }
    return skip(c);
}

void snapshot_free(Snapshot *s) {
    for (size_t i = 0; i < s->n_apps; i++) free(s->apps[i].name);
    free(s->apps);
    for (size_t i = 0; i < s->n_friends; i++) free(s->friends[i].persona);
    free(s->friends);
    free(s->app.install_dir);
    free(s->app.cloud_dir);
    free(s->app.beta_name);
    free(s->app.dlc);
    for (size_t i = 0; i < s->app.n_betas; i++) { free(s->app.betas[i].name); free(s->app.betas[i].description); }
    free(s->app.betas);
    free(s->app.depots);
    free(s->ticket.ownership);
    free(s->ticket.token);
    memset(s, 0, sizeof *s);
}

bool snapshot_load(const char *json, size_t len, Snapshot *out) {
    memset(out, 0, sizeof *out);
    out->online = true;
    Cur c = { json, json + len };
    if (!obj(&c, top_key, out) || out->schema != 1) {
        snapshot_free(out);
        return false;
    }
    return true;
}
