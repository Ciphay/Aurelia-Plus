#include <setjmp.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "probe_table.h"

typedef void *(*CreateInterfaceFn)(const char *, int *);
typedef uint64_t (*Fn6)(void *, uint64_t, uint64_t, uint64_t, uint64_t, uint64_t, uint64_t);
static void *vslot_of(void *obj, int slot) { return (*(void ***)obj)[slot]; }

static FILE *g_out;
static __thread jmp_buf *t_jb;
static __thread DWORD t_code;
static char g_what[256];
typedef struct { void *obj; int slot; int hidden; uint64_t ret; DWORD code; volatile int done; } Job;

static LONG CALLBACK veh(PEXCEPTION_POINTERS p) {
    DWORD c = p->ExceptionRecord->ExceptionCode;
    if (!t_jb || c == 0xE06D7363 || c == 0x40010006 || c == 0x406D1388 || c == EXCEPTION_BREAKPOINT || (c & 0xF0000000) == 0x40000000) return EXCEPTION_CONTINUE_SEARCH;
    t_code = c;
    longjmp(*t_jb, 1);
}

static DWORD WINAPI call_thread(LPVOID arg) {
    Job *j = arg;
    jmp_buf jb;
    t_jb = &jb;
    if (setjmp(jb) == 0) {
        uint64_t scratch[64] = {0};
        j->ret = ((Fn6)vslot_of(j->obj, j->slot))(j->obj, j->hidden ? (uint64_t)(uintptr_t)scratch : 0, 0, 0, 0, 0, 0);
        j->code = 0;
    } else {
        j->code = t_code;
    }
    t_jb = NULL;
    j->done = 1;
    return 0;
}

static void *vslot(void *obj, int slot) { return (*(void ***)obj)[slot]; }

static int find_slot(const char *iface, const char *method) {
    for (int i = 0; i < PROBE_COUNT; i++)
        if (!strcmp(g_probe[i].iface, iface))
            for (int k = 0; k < g_probe[i].n; k++) if (!strcmp(g_probe[i].methods[k], method)) return k;
    return -1;
}

static int skipped(const char *list, const char *what) {
    if (!list) return 0;
    size_t n = strlen(what);
    for (const char *p = list; (p = strstr(p, what)); p += n) if ((p == list || p[-1] == ',') && (p[n] == ',' || !p[n])) return 1;
    return 0;
}

int main(int argc, char **argv) {
    const char *dll_path = argc > 1 ? argv[1] : getenv("PROBE_DLL"), *out_path = argc > 2 ? argv[2] : getenv("PROBE_OUT"), *appid = argc > 3 ? argv[3] : getenv("PROBE_APPID");
    if (!dll_path || !out_path) { fprintf(stderr, "usage: probe <dll> <output> [appid]\n"); return 2; }
    g_out = fopen(out_path, "a");
    if (!g_out) return 2;
    if (appid) { SetEnvironmentVariableA("SteamAppId", appid); SetEnvironmentVariableA("SteamGameId", appid); }
    const char *skip = getenv("PROBE_SKIP");
    HMODULE dll = LoadLibraryA(dll_path);
    if (!dll) { fprintf(g_out, "ERROR load %lu\n", GetLastError()); fclose(g_out); return 2; }
    CreateInterfaceFn create = (CreateInterfaceFn)GetProcAddress(dll, "CreateInterface");
    if (!create) { fprintf(g_out, "ERROR no CreateInterface\n"); fclose(g_out); return 2; }
    PVOID veh_handle = AddVectoredExceptionHandler(1, veh);
    const char *cv = "SteamClient023";
    void *client = create(cv, NULL);
    fprintf(g_out, "CLIENT %d\n", client != NULL);
    RemoveVectoredExceptionHandler(veh_handle);
    AddVectoredExceptionHandler(1, veh);
    if (!client) { fclose(g_out); return 2; }
    int s_pipe = find_slot(cv, "CreateSteamPipe"), s_user = find_slot(cv, "ConnectToGlobalUser"), s_gen = find_slot(cv, "GetISteamGenericInterface");
    uint32_t pipe = (uint32_t)((Fn6)vslot(client, s_pipe))(client, 0, 0, 0, 0, 0, 0);
    uint32_t user = (uint32_t)((Fn6)vslot(client, s_user))(client, pipe, 0, 0, 0, 0, 0);
    fprintf(g_out, "PIPE %u USER %u\n", pipe, user);
    fflush(g_out);
    if (!pipe || !user) { fprintf(g_out, "NOPIPE\n"); fclose(g_out); return 5; }
    DWORD hang_ms = getenv("PROBE_HANG_MS") ? (DWORD)atoi(getenv("PROBE_HANG_MS")) : 800;
    int start = getenv("PROBE_START") ? atoi(getenv("PROBE_START")) : 0, gidx = 0;
    for (int i = 0; i < PROBE_COUNT; i++) {
        const ProbeIface *pi = &g_probe[i];
        if (getenv("PROBE_ONLY") && !strstr(pi->iface, getenv("PROBE_ONLY"))) continue;
        if (!strncmp(pi->iface, "SteamClient", 11) || !strncmp(pi->iface, "SteamGameServer", 15) || !strncmp(pi->iface, "SteamNetworkingSocketsSerialized", 32)) continue;
        if (start > 0 && gidx + 0 < start && 0) continue;
        void *obj = (void *)((Fn6)vslot(client, s_gen))(client, user, pipe, (uint64_t)(uintptr_t)pi->iface, 0, 0, 0);
        fprintf(g_out, "I %s %d\n", pi->iface, obj != NULL);
        fflush(g_out);
        if (!obj) continue;
        for (int k = 0; k < pi->n; k++) {
            if (pi->skip[k]) continue;
            if (gidx++ < start) continue;
            snprintf(g_what, sizeof g_what, "%s.%s", pi->iface, pi->methods[k]);
            if (skipped(skip, g_what)) { fprintf(g_out, "M %s SKIP\n", g_what); continue; }
            fprintf(g_out, "C %d %s\n", gidx - 1, g_what);
            fflush(g_out);
            Job *job = calloc(1, sizeof *job);
            job->obj = obj; job->slot = k; job->hidden = pi->hidden[k];
            HANDLE th = CreateThread(NULL, 0, call_thread, job, 0, NULL);
            if (WaitForSingleObject(th, hang_ms) == WAIT_TIMEOUT) {
                fprintf(g_out, "HANG %s\n", g_what);
                fprintf(g_out, "M %s hang\n", g_what);
            } else if (job->code) {
                fprintf(g_out, "M %s crash=%lx\n", g_what, (unsigned long)job->code);
                free(job);
            } else {
                fprintf(g_out, "M %s ret=%llx\n", g_what, (unsigned long long)job->ret);
                free(job);
            }
            CloseHandle(th);
            fflush(g_out);
        }
    }
    fprintf(g_out, "END\n");
    fclose(g_out);
    return 0;
}
