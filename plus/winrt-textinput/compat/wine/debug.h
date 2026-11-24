#pragma once
#include <windows.h>
#include <stdio.h>
#include <wchar.h>
#define WINE_DEFAULT_DEBUG_CHANNEL(c)
#define TRACE(...) do {} while (0)
#define WARN(...) do {} while (0)
#define FIXME(...) do {} while (0)
#define ERR(...) do {} while (0)
static inline const char *debugstr_guid(const GUID *g) {
    static char buf[4][40];
    static int n;
    char *b = buf[n++ & 3];
    if (!g) return "(null)";
    snprintf(b, 40, "{%08lx-%04x-%04x-%02x%02x-%02x%02x%02x%02x%02x%02x}", (unsigned long)g->Data1, g->Data2, g->Data3,
             g->Data4[0], g->Data4[1], g->Data4[2], g->Data4[3], g->Data4[4], g->Data4[5], g->Data4[6], g->Data4[7]);
    return b;
}
#ifndef ReadNoFence
#define ReadNoFence(p) (*(volatile LONG *)(p))
#endif
