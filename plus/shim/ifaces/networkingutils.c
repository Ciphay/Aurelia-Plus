#include <stdio.h>
#include <string.h>
#include <windows.h>

#include "../shim.h"

enum { AVAIL_CANNOT_TRY = -102, AVAIL_CURRENT = 100 };

static int64_t SHIM_TC local_timestamp(void *self) {
    (void)self;
    LARGE_INTEGER freq, now;
    QueryPerformanceFrequency(&freq);
    QueryPerformanceCounter(&now);
    return (int64_t)((double)now.QuadPart * 1000000.0 / (double)freq.QuadPart);
}

static int SHIM_TC relay_network_status(void *self, int32_t *out) {
    GNS_DELEGATE(shim_gns_relay_status(out));
    (void)self;
    if (out) {
        memset(out, 0, 16 + 256);
        out[0] = AVAIL_CURRENT;
        out[2] = AVAIL_CURRENT;
        out[3] = AVAIL_CANNOT_TRY;
        snprintf((char *)(out + 4), 256, "direct P2P transport of Aurelia Plus (no Valve relay)");
    }
    return AVAIL_CURRENT;
}

static void SHIM_TC init_relay_access(void *self) {
    GNS_DELEGATE_VOID(shim_gns_init_relay());
    (void)self;
    uint8_t cb[272];
    memset(cb, 0, sizeof cb);
    relay_network_status(NULL, (int32_t *)cb);
    shim_post_callback(1281, cb, sizeof cb);
}

static void *SHIM_TC allocate_message(void *self, int cb) { (void)self; GNS_DELEGATE(shim_gns_allocate_message(cb)); return shim_net_allocate_message(cb); }

void register_networkingutils(void) {
    const char *p = "SteamNetworkingUtils0";
    shim_register_family(p, "GetLocalTimestamp", (void *)local_timestamp);
    shim_register_family(p, "GetRelayNetworkStatus", (void *)relay_network_status);
    shim_register_family(p, "InitRelayNetworkAccess", (void *)init_relay_access);
    shim_register_family(p, "AllocateMessage", (void *)allocate_message);
}
