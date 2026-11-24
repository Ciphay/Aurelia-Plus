#include <stdint.h>
#include <stdio.h>

#include "ifaces.h"

static long g_unimpl;

uint64_t shim_callresult_fail(void) { return 0x1234; }
void shim_unimplemented(const char *iface, int slot, const char *method) {
    (void)iface; (void)slot; (void)method;
    g_unimpl++;
}

static uint64_t redirected_impl(void *self) {
    (void)self;
    return 0xABCDu;
}

typedef uint64_t (*Fn)(void *, uint64_t *);

int main(void) {
    long calls = 0;
    for (int i = 0; i < g_n_ifaces; i++) {
        const IfaceDesc *d = &g_ifaces[i];
        for (int s = 0; s < d->n_slots; s++) {
            if (!d->vtable[s]) {
                printf("NULL slot %s %d\n", d->name, s);
                return 1;
            }
            uint64_t scratch = 0xFFFFFFFFFFFFFFFFULL;
            ((Fn)d->vtable[s])(d->object, &scratch);
            calls++;
        }
    }
    int redirected = 0;
    for (int i = 0; i < g_n_ifaces && !redirected; i++) {
        const IfaceDesc *d = &g_ifaces[i];
        for (int s = 0; s < d->n_slots; s++) {
            if (!d->slot_names[s]) continue;
            if (shim_register(d->name, d->slot_names[s], (void *)redirected_impl) != 0) return 2;
            uint64_t scratch2 = 0;
            redirected = ((Fn)d->vtable[s])(d->object, &scratch2) == 0xABCDu;
            break;
        }
    }
    printf("OK interfaces=%d calls=%ld unimplemented=%ld redirected=%d\n", g_n_ifaces, calls, g_unimpl, redirected);
    return redirected ? 0 : 3;
}
