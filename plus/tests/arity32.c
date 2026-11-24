#include <stdio.h>
#include <string.h>

#include "ifaces.h"

static int g_unimpl, g_mismatch, g_last_delta;
uint64_t shim_callresult_fail(void) { return 0x1234; }
void shim_unimplemented(const char *iface, int slot, const char *method) { (void)iface; (void)slot; (void)method; g_unimpl++; }
void shim_arity_mismatch(int iface, int slot, int delta) { (void)iface; (void)slot; g_mismatch++; g_last_delta = delta; }

typedef uint32_t (__attribute__((thiscall)) *Call3)(void *, uint32_t, uint32_t, uint32_t);
typedef uint32_t (__attribute__((thiscall)) *Call0)(void *);

static uint32_t __attribute__((thiscall)) impl_exact(void *self, uint32_t a, uint32_t b, uint32_t c) { (void)self; return a + b * 10 + c * 100; }
static uint32_t __attribute__((thiscall)) impl_fewer(void *self, uint32_t a) { (void)self; return a + 1000; }
static uint32_t __attribute__((thiscall)) impl_more(void *self, uint32_t a, uint32_t b, uint32_t c, uint32_t d, uint32_t e) { (void)self; (void)d; (void)e; return a + b + c + 5000; }

#define CHECK(c) do { if (!(c)) { printf("FAIL %s:%d %s\n", __FILE__, __LINE__, #c); return 1; } } while (0)

int main(void) {
    const IfaceDesc *d = &g_ifaces[0];
    void *obj = d->object;
    Call3 call3 = (Call3)d->vtable[1];
    Call0 call0 = (Call0)d->vtable[0];
    volatile uint32_t canary = 0xC0FFEE;

    call3(obj, 1, 2, 3);
    CHECK(g_unimpl == 1 && canary == 0xC0FFEE && g_mismatch == 0);

    shim_register("IfaceA001", "Three", (void *)impl_exact);
    CHECK(call3(obj, 1, 2, 3) == 321 && g_mismatch == 0 && canary == 0xC0FFEE);

    shim_register("IfaceA001", "Three", (void *)impl_fewer);
    CHECK(call3(obj, 5, 2, 3) == 1005 && canary == 0xC0FFEE);
    CHECK(g_mismatch == 1 && g_last_delta == -8);

    shim_register("IfaceA001", "Three", (void *)impl_more);
    CHECK(call3(obj, 1, 2, 3) == 5006 && canary == 0xC0FFEE);
    CHECK(g_mismatch == 2 && g_last_delta == 8);

    call0(obj);
    CHECK(canary == 0xC0FFEE);
    printf("OK arity\n");
    return 0;
}
