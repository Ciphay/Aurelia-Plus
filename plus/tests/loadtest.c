#include <stdio.h>
#include <string.h>
#include <windows.h>

int main(int argc, char **argv) {
    if (argc < 3) return 2;
    HMODULE m = LoadLibraryA(argv[1]);
    if (!m) { printf("FAIL load %lu\n", GetLastError()); return 1; }
    FILE *f = fopen(argv[2], "r");
    if (!f) return 2;
    char line[256];
    int total = 0, missing = 0;
    while (fgets(line, sizeof line, f)) {
        char *n = line;
        while (*n == ' ') n++;
        n[strcspn(n, "\r\n")] = 0;
        if (!*n || !strcmp(n, "EXPORTS")) continue;
        total++;
        if (!GetProcAddress(m, n)) { printf("MISSING %s\n", n); missing++; }
    }
    fclose(f);
    typedef void *(*CI)(const char *, int *);
    CI ci = (CI)GetProcAddress(m, "CreateInterface");
    int rc = -1;
    void *o = ci ? ci("SteamClient023", &rc) : NULL;
    printf("CreateInterface %s rc=%d\n", o ? "ok" : "null", rc);
    if (missing || !o || rc) return 1;
    printf("OK exports=%d\n", total);
    return 0;
}
