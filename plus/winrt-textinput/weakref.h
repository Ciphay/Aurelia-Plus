#ifndef __WINE_WEAKREF_H
#define __WINE_WEAKREF_H

#include "weakreference.h"

struct weak_reference_source
{
    IWeakReferenceSource IWeakReferenceSource_iface;
    IWeakReference *weak_reference;
};

HRESULT weak_reference_source_init( struct weak_reference_source *source, IUnknown *object );
ULONG weak_reference_strong_add_ref( struct weak_reference_source *source );
ULONG weak_reference_strong_release( struct weak_reference_source *source );

#endif
