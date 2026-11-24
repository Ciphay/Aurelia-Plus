#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "shim.h"

static Snapshot g_snap;
static bool g_have;
static bool g_inited;

void shim_log(const char *fmt, ...) {
    const char *path = getenv("AURELIA_SHIM_LOG");
    if (!path || !*path) path = "aurelia-shim.log";
    FILE *f = fopen(path, "ab");
    if (!f) return;
    fprintf(f, "[%lu] ", (unsigned long)GetTickCount());
    va_list ap;
    va_start(ap, fmt);
    vfprintf(f, fmt, ap);
    va_end(ap);
    fputc('\n', f);
    fclose(f);
}

uint32_t shim_process_appid(void) {
    char v[32];
    DWORD n = GetEnvironmentVariableA("SteamAppId", v, sizeof v);
    if (n > 0 && n < sizeof v) {
        unsigned long id = strtoul(v, NULL, 10);
        if (id > 0 && id < 0xFFFFFFFFul) return (uint32_t)id;
    }
    const Snapshot *s = shim_snapshot();
    return s && s->has_ticket ? s->ticket.appid : 0;
}

void shim_trace(const char *fmt, ...) {
    const char *on = getenv("AURELIA_SHIM_TRACE");
    if (!on || !*on) return;
    char line[512];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(line, sizeof line, fmt, ap);
    va_end(ap);
    shim_log("TRACE %s", line);
}

void shim_unimplemented(const char *iface, int slot, const char *method) {
    static struct { const char *iface; int slot; } seen[4096];
    static int n;
    for (int i = 0; i < n; i++) if (seen[i].iface == iface && seen[i].slot == slot) return;
    if (n < 4096) { seen[n].iface = iface; seen[n].slot = slot; n++; }
    shim_log("UNIMPLEMENTED %s slot %d %s", iface, slot, method);
}

bool shim_load_snapshot_json(const char *json, size_t len) {
    Snapshot s;
    if (!snapshot_load(json, len, &s)) {
        shim_log("invalid snapshot (%lu bytes)", (unsigned long)len);
        return false;
    }
    if (g_have) snapshot_free(&g_snap);
    g_snap = s;
    g_have = true;
    return true;
}

const Snapshot *shim_snapshot(void) { return g_have ? &g_snap : NULL; }

bool shim_session_online(void) { return g_have && bridge_reachable() && shim_connection_online(); }

bool shim_connection_online(void) {
    static volatile LONG stamp, last = -1;
    DWORD now = GetTickCount() | 1;
    if (stamp && last >= 0 && now - (DWORD)stamp < 1500) return last != 0;
    uint8_t b = 0;
    size_t n = 0;
    int32_t status = -1;
    bool on = g_have ? g_snap.online : false;
    if (bridge_call_fresh(0xFFFF, 15, NULL, 0, &status, &b, 1, &n, 700) && status == 0 && n >= 1) on = b != 0;
    InterlockedExchange(&last, on ? 1 : 0);
    InterlockedExchange(&stamp, (LONG)now);
    return on;
}

void shim_poll_connection(void) {
    static volatile LONG seen = -1;
    if (!g_have) return;
    LONG now = shim_connection_online() ? 1 : 0;
    LONG init = g_snap.online ? 1 : 0;
    LONG before = InterlockedCompareExchange(&seen, init, -1);
    if (before == -1) before = init;
    if (before == now) return;
    InterlockedExchange(&seen, now);
    shim_log("Steam connection: %s", now ? "restored" : "lost");
    if (now) {
        static const uint8_t empty = 0;
        shim_post_callback(101, &empty, 1);
    } else {
        int32_t result = 3;
        shim_post_callback(103, &result, sizeof result);
    }
}

static volatile LONG g_local_ip, g_local_port;
void shim_set_local_ip_binding(uint32_t ip, uint16_t port) { g_local_ip = (LONG)ip; g_local_port = port; shim_log("SetLocalIPBinding %u.%u.%u.%u:%u", ip >> 24, (ip >> 16) & 255, (ip >> 8) & 255, ip & 255, port); }
uint32_t shim_local_ip_binding(void) { return (uint32_t)g_local_ip; }
uint16_t shim_local_port_binding(void) { return (uint16_t)g_local_port; }

static bool hex32(const char *h, uint8_t out[32]) {
    if (!h || strlen(h) != 64) return false;
    for (int i = 0; i < 32; i++) {
        unsigned v;
        if (sscanf(h + 2 * i, "%2x", &v) != 1) return false;
        out[i] = (uint8_t)v;
    }
    return true;
}

static void connect_bridge(void) {
    const char *port = getenv("AURELIA_BRIDGE_PORT");
    uint8_t token[32];
    if (!port || !hex32(getenv("AURELIA_BRIDGE_TOKEN"), token)) return;
    if (!bridge_connect("127.0.0.1", (uint16_t)atoi(port), token, 3000)) { shim_log("bridge unavailable on port %s", port); return; }
    char *json = NULL;
    size_t len = 0;
    if (bridge_fetch_snapshot(&json, &len, 10000)) {
        shim_load_snapshot_json(json, len);
        free(json);
    } else {
        shim_log("snapshot not received from the bridge");
    }
}

void shim_arity_mismatch(int iface, int slot, int delta) {
    if (delta <= 0) return;
    static char seen[512][64];
    if (iface < 0 || iface >= 512 || slot < 0 || slot >= 64 || seen[iface][slot]) return;
    seen[iface][slot] = 1;
    shim_log("ARITY %s slot %d (%s): the implementation popped %d more bytes than the catalog says", g_ifaces[iface].name, slot,
             g_ifaces[iface].slot_names[slot] ? g_ifaces[iface].slot_names[slot] : "?", delta);
}

const char *shim_iface_name(void *self) {
    for (int i = 0; i < g_n_ifaces; i++) if (g_ifaces[i].object == self) return g_ifaces[i].name;
    return NULL;
}

size_t shim_win_path(const char *path, char *out, size_t cap) {
    if (!path || !path[0]) return 0;
    size_t n = strlen(path), o = 0;
    bool unix_path = path[0] == '/';
    if (n + (unix_path ? 2 : 0) + 1 > cap) return 0;
    if (unix_path) { out[o++] = 'Z'; out[o++] = ':'; }
    for (size_t i = 0; i < n; i++) out[o++] = unix_path && path[i] == '/' ? '\\' : path[i];
    out[o] = 0;
    return o;
}

static const char *module_of(const void *addr, char *buf, size_t cap, uintptr_t *off) {
    HMODULE m = NULL;
    MEMORY_BASIC_INFORMATION mbi;
    if (!VirtualQuery(addr, &mbi, sizeof mbi) || mbi.Type != MEM_IMAGE) return NULL;
    if (!GetModuleHandleExA(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, (LPCSTR)addr, &m)) return NULL;
    char path[MAX_PATH];
    if (!GetModuleFileNameA(m, path, sizeof path)) return NULL;
    const char *base = strrchr(path, '\\');
    snprintf(buf, cap, "%s", base ? base + 1 : path);
    *off = (uintptr_t)addr - (uintptr_t)m;
    return buf;
}

static LONG CALLBACK fatal_exception_logger(EXCEPTION_POINTERS *ep) {
    DWORD code = ep->ExceptionRecord->ExceptionCode;
    if (code != EXCEPTION_ACCESS_VIOLATION && code != EXCEPTION_ILLEGAL_INSTRUCTION && code != EXCEPTION_STACK_OVERFLOW &&
        code != EXCEPTION_INT_DIVIDE_BY_ZERO && code != EXCEPTION_PRIV_INSTRUCTION)
        return EXCEPTION_CONTINUE_SEARCH;
    static volatile LONG seen;
    if (InterlockedIncrement(&seen) > 6) return EXCEPTION_CONTINUE_SEARCH;
    char mod[MAX_PATH];
    uintptr_t off = 0, ip, sp;
    const CONTEXT *c = ep->ContextRecord;
#ifdef _WIN64
    ip = (uintptr_t)c->Rip; sp = (uintptr_t)c->Rsp;
#else
    ip = (uintptr_t)c->Eip; sp = (uintptr_t)c->Esp;
#endif
    const char *m = module_of((void *)ip, mod, sizeof mod, &off);
    const ULONG_PTR *info = ep->ExceptionRecord->ExceptionInformation;
    shim_log("EXCEPTION %08lx at %s+0x%lx (%p)%s", (unsigned long)code, m ? m : "?", (unsigned long)off, (void *)ip,
             code == EXCEPTION_ACCESS_VIOLATION ? (info[0] == 0 ? " read" : info[0] == 1 ? " write" : " execute") : "");
    if (code == EXCEPTION_ACCESS_VIOLATION) shim_log("  accessed address %p", (void *)info[1]);
#ifdef _WIN64
    shim_log("  rax=%p rbx=%p rcx=%p rdx=%p rsi=%p rdi=%p", (void *)c->Rax, (void *)c->Rbx, (void *)c->Rcx, (void *)c->Rdx, (void *)c->Rsi, (void *)c->Rdi);
#else
    shim_log("  eax=%p ebx=%p ecx=%p edx=%p esi=%p edi=%p ebp=%p", (void *)c->Eax, (void *)c->Ebx, (void *)c->Ecx, (void *)c->Edx, (void *)c->Esi, (void *)c->Edi, (void *)c->Ebp);
#endif
    int shown = 0;
    for (int i = 0; i < 1024 && shown < 40; i++) {
        const uintptr_t *w = (const uintptr_t *)sp + i;
        if (IsBadReadPtr(w, sizeof *w)) break;
        uintptr_t o;
        if (module_of((void *)*w, mod, sizeof mod, &o)) { shim_log("  stack+%d: %s+0x%lx", i * (int)sizeof *w, mod, (unsigned long)o); shown++; }
    }
    return EXCEPTION_CONTINUE_SEARCH;
}

static int version_of(const char *name, size_t *family_len) {
    size_t n = strlen(name), d = n;
    while (d > 0 && name[d - 1] >= '0' && name[d - 1] <= '9') d--;
    *family_len = d;
    return d < n ? atoi(name + d) : -1;
}

static bool has_names(const IfaceDesc *d) {
    for (int s = 0; s < d->n_slots; s++) if (d->slot_names[s]) return true;
    return false;
}

static void alias_unknown_versions(void) {
    for (int i = 0; i < g_n_ifaces; i++) {
        const IfaceDesc *u = &g_ifaces[i];
        if (has_names(u)) continue;
        size_t fl, ol;
        int v = version_of(u->name, &fl);
        if (v < 0) continue;
        int best = -1, best_v = 0;
        for (int j = 0; j < g_n_ifaces; j++) {
            const IfaceDesc *o = &g_ifaces[j];
            if (j == i || !has_names(o)) continue;
            int ov = version_of(o->name, &ol);
            if (ol != fl || strncmp(o->name, u->name, fl) || ov < 0 || ov == v) continue;
            bool better;
            if (best < 0) better = true;
            else if (ov > v && best_v > v) better = ov < best_v;
            else if (ov > v) better = true;
            else better = best_v < v && ov > best_v;
            if (better) { best = j; best_v = ov; }
        }
        if (best < 0) continue;
        const IfaceDesc *o = &g_ifaces[best];
        int n = u->n_slots < o->n_slots ? u->n_slots : o->n_slots;
        for (int s = 0; s < n; s++) u->vtable[s] = o->vtable[s];
        shim_trace("interface %s has no catalog entry: using the table of %s", u->name, o->name);
    }
}

void shim_init(void) {
    if (g_inited) return;
    g_inited = true;
    if (!g_have) connect_bridge();
    AddVectoredExceptionHandler(1, fatal_exception_logger);
    void **stub_copy[2048] = {0};
    char dump_path[MAX_PATH];
    bool dump = g_n_ifaces <= 2048 && GetEnvironmentVariableA("AURELIA_SHIM_DUMP_STUBS", dump_path, sizeof dump_path) > 0;
    shim_log("shim_init: dump=%d n=%d", (int)dump, g_n_ifaces);
    for (int i = 0; dump && i < g_n_ifaces; i++) {
        stub_copy[i] = malloc((size_t)g_ifaces[i].n_slots * sizeof(void *));
        if (stub_copy[i]) memcpy(stub_copy[i], g_ifaces[i].impl, (size_t)g_ifaces[i].n_slots * sizeof(void *));
    }
    register_policies();
    register_client();
    register_user();
    register_apps();
    register_utils();
    register_misc();
    register_userstats();
    register_leaderboards();
    register_appticket();
    register_inventory();
    register_ugc();
    register_ugcquery();
    register_ugcwrite();
    register_remotepublish();
    register_remotestorage();
    register_matchmakingservers();
    register_lobbies();
    register_serverlists();
    register_gameserver();
    register_applist();
    register_screenshots();
    register_input();
    register_http();
    register_networkingutils();
    register_networking();
    register_p2pnet();
    register_sockets();
    register_overlay();
    register_extras();
    register_community();
    register_otherstats();
    alias_unknown_versions();
    if (dump) {
        for (int i = 0; i < g_n_ifaces; i++) {
            for (int k = 0; stub_copy[i] && k < g_ifaces[i].n_slots; k++)
                if (stub_copy[i][k] == g_ifaces[i].impl[k]) shim_log("STUB %s %d %s", g_ifaces[i].name, k, g_ifaces[i].slot_names[k] ? g_ifaces[i].slot_names[k] : "?");
            free(stub_copy[i]);
        }
    }
    if (!shim_snapshot() || shim_snapshot()->online) {
        static const uint8_t empty = 0;
        shim_post_callback(101, &empty, 1);
    } else {
        int32_t failure[2] = { 3, 1 };
        shim_post_callback(102, failure, sizeof failure);
    }
}

void shim_register_family_ab(const char *prefix, const char *method, int argbytes, void *fn) {
    size_t n = strlen(prefix);
    for (int i = 0; i < g_n_ifaces; i++) {
        const IfaceDesc *d = &g_ifaces[i];
        if (strncmp(d->name, prefix, n)) continue;
        for (int s = 0; s < d->n_slots; s++)
            if (d->slot_names[s] && !strcmp(d->slot_names[s], method) && d->argbytes[s] == argbytes) d->impl[s] = fn;
    }
}

void shim_register_family(const char *prefix, const char *method, void *fn) {
    size_t n = strlen(prefix);
    for (int i = 0; i < g_n_ifaces; i++)
        if (!strncmp(g_ifaces[i].name, prefix, n)) shim_register(g_ifaces[i].name, method, fn);
}

void *CreateInterface(const char *name, int *return_code) {
    shim_init();
    for (int i = 0; i < g_n_ifaces; i++) {
        if (!strcmp(g_ifaces[i].name, name)) {
            if (return_code) *return_code = 0;
            p2p_interface_requested(name);
            if (!strncmp(name, "SteamClient", 11)) {
#ifdef SHIM_WITH_GNS
                char gns[8];
                if (!(GetEnvironmentVariableA("AURELIA_SHIM_GNS", gns, sizeof gns) && gns[0] == '0')) shim_gns_start_async(0);
#endif
                userstats_preload(); serverlists_preload(); account_preload();
            }
            return g_ifaces[i].object;
        }
    }
    shim_log("CreateInterface: unknown version %s", name);
    if (return_code) *return_code = 1;
    return NULL;
}
