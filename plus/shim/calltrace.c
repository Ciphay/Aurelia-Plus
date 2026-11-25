
#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "shim.h"

#if defined(__x86_64__) || defined(__i386__)
typedef struct { const char *iface; const char *slot; void *orig; volatile LONG count; int index; } CallRec;

static void call_trace_log(CallRec *r) {
    LONG n = InterlockedIncrement(&r->count);
    if (n <= 3 || n % 500 == 0) shim_log("CALL %s.%s (#%ld)%s", r->iface, r->slot ? r->slot : "?", (long)n, shim_is_policy_stub(r->orig) ? " [POLICY STUB: fixed answer, no callback]" : "");
}

#if defined(__x86_64__)
static uint8_t *put(uint8_t *p, const void *bytes, size_t n) { memcpy(p, bytes, n); return p + n; }
#endif

#if defined(__i386__)

static uint8_t *emit_stub(uint8_t *p, CallRec *rec) {
    void *logger = (void *)call_trace_log;
    *p++ = 0x51;
    *p++ = 0x68; memcpy(p, &rec, 4); p += 4;
    *p++ = 0xB8; memcpy(p, &logger, 4); p += 4;
    *p++ = 0xFF; *p++ = 0xD0;
    *p++ = 0x83; *p++ = 0xC4; *p++ = 0x04;
    *p++ = 0x59;
    *p++ = 0xB8; memcpy(p, &rec->orig, 4); p += 4;
    *p++ = 0xFF; *p++ = 0xE0;
    return p;
}
#else
static uint8_t *emit_stub(uint8_t *p, CallRec *rec) {
    static const uint8_t pro[] = {
        0x48, 0x81, 0xEC, 0x88, 0x00, 0x00, 0x00,
        0x48, 0x89, 0x4C, 0x24, 0x20, 0x48, 0x89, 0x54, 0x24, 0x28,
        0x4C, 0x89, 0x44, 0x24, 0x30, 0x4C, 0x89, 0x4C, 0x24, 0x38,
        0xF3, 0x0F, 0x7F, 0x44, 0x24, 0x40, 0xF3, 0x0F, 0x7F, 0x4C, 0x24, 0x50,
        0xF3, 0x0F, 0x7F, 0x54, 0x24, 0x60, 0xF3, 0x0F, 0x7F, 0x5C, 0x24, 0x70 };
    static const uint8_t epi[] = {
        0x48, 0x8B, 0x4C, 0x24, 0x20, 0x48, 0x8B, 0x54, 0x24, 0x28,
        0x4C, 0x8B, 0x44, 0x24, 0x30, 0x4C, 0x8B, 0x4C, 0x24, 0x38,
        0xF3, 0x0F, 0x6F, 0x44, 0x24, 0x40, 0xF3, 0x0F, 0x6F, 0x4C, 0x24, 0x50,
        0xF3, 0x0F, 0x6F, 0x54, 0x24, 0x60, 0xF3, 0x0F, 0x6F, 0x5C, 0x24, 0x70,
        0x48, 0x81, 0xC4, 0x88, 0x00, 0x00, 0x00 };
    void *logger = (void *)call_trace_log;
    uint8_t movrcx[2] = { 0x48, 0xB9 }, movrax[2] = { 0x48, 0xB8 }, call_rax[2] = { 0xFF, 0xD0 }, jmp_rax[2] = { 0xFF, 0xE0 };
    p = put(p, pro, sizeof pro);
    p = put(p, movrcx, 2); p = put(p, &rec, 8);
    p = put(p, movrax, 2); p = put(p, &logger, 8);
    p = put(p, call_rax, 2);
    p = put(p, epi, sizeof epi);
    p = put(p, movrax, 2); p = put(p, &rec->orig, 8);
    p = put(p, jmp_rax, 2);
    return p;
}
#endif

void shim_install_call_trace(void) {
    const char *on = getenv("AURELIA_SHIM_TRACE_CALLS");
    if (!on || !*on) return;
    size_t slots = 0;
    for (int i = 0; i < g_n_ifaces; i++) slots += (size_t)g_ifaces[i].n_slots;
    const size_t stub_max = 192;
    uint8_t *code = VirtualAlloc(NULL, slots * stub_max, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE);
    CallRec *recs = calloc(slots ? slots : 1, sizeof *recs);
    void **done = calloc((size_t)g_n_ifaces + 1, sizeof *done);
    if (!code || !recs || !done) { shim_log("CALL trace: out of memory"); return; }
    size_t used = 0;
    int wrapped = 0;
    for (int i = 0; i < g_n_ifaces; i++) {
        void **impl = g_ifaces[i].impl;
        int seen = 0;
        for (int j = 0; j < i; j++) if (done[j] == (void *)impl) seen = 1;
        done[i] = (void *)impl;
        if (seen) continue;
        for (int k = 0; k < g_ifaces[i].n_slots; k++) {
            CallRec *r = &recs[used];
            r->iface = g_ifaces[i].name;
            r->slot = g_ifaces[i].slot_names ? g_ifaces[i].slot_names[k] : NULL;
            r->orig = impl[k];
            if (on[0] == '2' && shim_is_policy_stub(impl[k])) shim_log("POLICY %s.%s", r->iface, r->slot ? r->slot : "?");
            r->index = k;
            uint8_t *start = code + used * stub_max;
            emit_stub(start, r);
            impl[k] = start;
            used++;
            wrapped++;
        }
    }
    shim_log("CALL trace: %d methods hooked", wrapped);
}
#else
void shim_install_call_trace(void) {}
#endif
