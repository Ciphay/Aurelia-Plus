#include <stdlib.h>
#include <string.h>

#include "shim.h"

static bool reserve(PbW *w, size_t more) {
    if (w->oom) return false;
    if (w->len + more <= w->cap) return true;
    size_t cap = w->cap ? w->cap * 2 : 128;
    while (cap < w->len + more) cap *= 2;
    uint8_t *p = realloc(w->p, cap);
    if (!p) { w->oom = true; return false; }
    w->p = p;
    w->cap = cap;
    return true;
}

static void raw_varint(PbW *w, uint64_t v) {
    if (!reserve(w, 10)) return;
    while (v >= 0x80) { w->p[w->len++] = (uint8_t)(v | 0x80); v >>= 7; }
    w->p[w->len++] = (uint8_t)v;
}

void pbw_free(PbW *w) { free(w->p); memset(w, 0, sizeof *w); }

void pbw_varint(PbW *w, uint32_t field, uint64_t v) { raw_varint(w, (uint64_t)field << 3); raw_varint(w, v); }

void pbw_fixed32(PbW *w, uint32_t field, uint32_t v) {
    raw_varint(w, ((uint64_t)field << 3) | 5);
    if (reserve(w, 4)) { memcpy(w->p + w->len, &v, 4); w->len += 4; }
}

void pbw_fixed64(PbW *w, uint32_t field, uint64_t v) {
    raw_varint(w, ((uint64_t)field << 3) | 1);
    if (reserve(w, 8)) { memcpy(w->p + w->len, &v, 8); w->len += 8; }
}

void pbw_bytes(PbW *w, uint32_t field, const void *data, size_t len) {
    raw_varint(w, ((uint64_t)field << 3) | 2);
    raw_varint(w, len);
    if (len && reserve(w, len)) { memcpy(w->p + w->len, data, len); w->len += len; }
}

void pbw_str(PbW *w, uint32_t field, const char *s) { pbw_bytes(w, field, s, s ? strlen(s) : 0); }

void pbw_append(PbW *w, const void *data, size_t len) {
    if (len && reserve(w, len)) { memcpy(w->p + w->len, data, len); w->len += len; }
}

void pbw_msg(PbW *w, uint32_t field, const PbW *inner) { pbw_bytes(w, field, inner->p, inner->len); }

static bool read_varint(PbR *r, uint64_t *v) {
    uint64_t out = 0;
    for (int shift = 0; shift < 70 && r->p < r->end; shift += 7) {
        uint8_t b = *r->p++;
        out |= (uint64_t)(b & 0x7f) << shift;
        if (!(b & 0x80)) { *v = out; return true; }
    }
    return false;
}

void pbr_init(PbR *r, const uint8_t *p, size_t len) { r->p = p; r->end = p + len; r->bad = false; }

bool pbr_next(PbR *r, PbField *f) {
    if (r->p >= r->end || r->bad) return false;
    uint64_t key;
    if (!read_varint(r, &key)) { r->bad = true; return false; }
    f->field = (uint32_t)(key >> 3);
    f->wt = (int)(key & 7);
    f->v = 0;
    f->data = NULL;
    f->len = 0;
    switch (f->wt) {
    case 0: if (!read_varint(r, &f->v)) { r->bad = true; return false; } return true;
    case 1: if (r->end - r->p < 8) { r->bad = true; return false; } memcpy(&f->v, r->p, 8); r->p += 8; return true;
    case 5: if (r->end - r->p < 4) { r->bad = true; return false; } { uint32_t x; memcpy(&x, r->p, 4); f->v = x; } r->p += 4; return true;
    case 2: {
        uint64_t n;
        if (!read_varint(r, &n) || n > (uint64_t)(r->end - r->p)) { r->bad = true; return false; }
        f->data = r->p;
        f->len = (size_t)n;
        r->p += n;
        return true;
    }
    default: r->bad = true; return false;
    }
}
