#include <stdint.h>
#include <string.h>

#include "../shim.h"

static uint64_t handle_of(const char *name) {
    uint64_t h = 1469598103934665603ULL;
    for (const char *p = name ? name : ""; *p; p++) { h ^= (uint8_t)*p; h *= 1099511628211ULL; }
    return h ? h : 1;
}

static uint64_t SHIM_TC action_handle(void *self, const char *name) { (void)self; return handle_of(name); }
static uint64_t SHIM_TC input_true(void *self) { (void)self; return 1; }
static uint64_t SHIM_TC input_false(void *self) { (void)self; return 0; }

void register_input(void) {
    const char *v1 = "STEAMCONTROLLER_INTERFACE_VERSION";
    shim_register(v1, "Init", (void *)input_true);
    shim_register(v1, "Shutdown", (void *)input_true);
    shim_register(v1, "RunFrame", (void *)input_false);
    shim_register(v1, "GetControllerState", (void *)input_false);
    shim_register(v1, "TriggerHapticPulse", (void *)input_false);
    shim_register(v1, "SetOverrideMode", (void *)input_false);
    static const char *const types[] = { "SteamInput", "SteamController" };
    for (size_t i = 0; i < 2; i++) {
        const char *p = types[i];
        shim_register_family(p, "GetActionSetHandle", (void *)action_handle);
        shim_register_family(p, "GetDigitalActionHandle", (void *)action_handle);
        shim_register_family(p, "GetAnalogActionHandle", (void *)action_handle);
        shim_register_family(p, "Init", (void *)input_true);
        shim_register_family(p, "Shutdown", (void *)input_true);
        shim_register_family(p, "BNewDataAvailable", (void *)input_false);
        shim_register_family(p, "BWaitForData", (void *)input_false);
    }
}
