#include <stdio.h>
#include <stdlib.h>
#include <windows.h>
typedef void *(*CreateInterfaceFn)(const char *, int *);
int main(void) {
    char dll[512], secs[16];
    if (!GetEnvironmentVariableA("PROBE_DLL", dll, sizeof dll)) { puts("FAIL PROBE_DLL"); return 2; }
    HMODULE h = LoadLibraryA(dll);
    if (!h) { printf("FAIL load %lu\n", GetLastError()); return 3; }
    CreateInterfaceFn ci = (CreateInterfaceFn)GetProcAddress(h, "CreateInterface");
    if (!ci) { puts("FAIL CreateInterface"); return 4; }
    int rc = 0;
    void *client = ci("SteamClient021", &rc);
    printf("SteamClient021 = %p rc=%d\n", client, rc);
    int n = GetEnvironmentVariableA("PROBE_SECS", secs, sizeof secs) ? atoi(secs) : 20;
    Sleep((DWORD)n * 1000);
    puts("OK");
    return client ? 0 : 5;
}
