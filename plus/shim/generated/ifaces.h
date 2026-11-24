
#ifndef AURELIA_IFACES_H
#define AURELIA_IFACES_H
#include <stdint.h>

typedef struct { void **vtable; } IfaceObj;
typedef struct {
    const char *name;
    int n_slots;
    void **vtable;
    const char *const *slot_names;
    IfaceObj *object;
    void **impl;
    const short *argbytes;
} IfaceDesc;

extern const IfaceDesc g_ifaces[];
extern const int g_n_ifaces;

void shim_arity_mismatch(int iface, int slot, int delta);

uint64_t shim_callresult_fail(void);

void shim_unimplemented(const char *iface, int slot, const char *method);

int shim_register(const char *iface, const char *method, void *fn);
#endif
