#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "../shim.h"

enum {
    WAIT_MS = 30000,
    CB_PUBLISH = 1309, CB_DELETE = 1311, CB_UPDATE = 1316,
    EMSG_UCM_PUBLISH = 7311, EMSG_UCM_DELETE = 7315, EMSG_UCM_UPDATE = 7325,
    MAX_LEGACY = 16,
};

static uint32_t app_id(void) { const Snapshot *s = shim_snapshot(); return s && s->has_ticket ? s->ticket.appid : 0; }

typedef struct { const char **strings; int32_t n; } ParamStringArray;

static void put_tags(PbW *w, uint32_t field, const ParamStringArray *tags) {
    for (int i = 0; tags && tags->strings && i < tags->n; i++) if (tags->strings[i]) pbw_str(w, field, tags->strings[i]);
}

static bool parse_publish(const CmReply *r, void *ctx, void *out, int *size) {
    (void)ctx;
    PbR in;
    PbField f;
    int32_t eresult = 2;
    uint64_t id = 0;
    uint8_t legal = 0;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field == 1) eresult = (int32_t)f.v;
        else if (f.field == 2) id = f.v;
        else if (f.field == 3) legal = f.v != 0;
    }
    if (in.bad || *size < 24) return false;
    shim_log("legacy publish: header eresult=%d, body eresult=%d, id=%llu, body of %lu bytes", r->eresult, eresult, (unsigned long long)id, (unsigned long)r->len);
    memset(out, 0, 24);
    memcpy(out, &eresult, 4);
    memcpy((uint8_t *)out + 8, &id, 8);
    ((uint8_t *)out)[16] = legal;
    *size = 24;
    return true;
}

static bool parse_update(const CmReply *r, void *ctx, void *out, int *size) {
    PbR in;
    PbField f;
    int32_t eresult = 2;
    uint8_t legal = 0;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) {
        if (f.field == 1) eresult = (int32_t)f.v;
        else if (f.field == 2) legal = f.v != 0;
    }
    if (in.bad || *size < 24) return false;
    memset(out, 0, 24);
    memcpy(out, &eresult, 4);
    memcpy((uint8_t *)out + 8, ctx, 8);
    ((uint8_t *)out)[16] = legal;
    *size = 24;
    return true;
}

static bool parse_delete(const CmReply *r, void *ctx, void *out, int *size) {
    PbR in;
    PbField f;
    int32_t eresult = 2;
    pbr_init(&in, r->body, r->len);
    while (pbr_next(&in, &f)) if (f.field == 1) eresult = (int32_t)f.v;
    if (in.bad || *size < 16) return false;
    memset(out, 0, 16);
    memcpy(out, &eresult, 4);
    memcpy((uint8_t *)out + 8, ctx, 8);
    *size = 16;
    return true;
}

static uint64_t *id_ctx(uint64_t id) {
    uint64_t *p = malloc(sizeof *p);
    if (p) *p = id;
    return p;
}

static uint64_t SHIM_TC publish_workshop_file(void *self, const char *file, const char *preview, uint32_t consumer, const char *title, const char *description,
                                              int visibility, const ParamStringArray *tags, int file_type) {
    (void)self;
    if (!file || !*file) return 0;
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_str(&w, 2, file);
    if (preview && *preview) pbw_str(&w, 3, preview);
    pbw_varint(&w, 4, consumer ? consumer : app_id());
    pbw_str(&w, 5, title ? title : "");
    pbw_str(&w, 6, description ? description : "");
    put_tags(&w, 8, tags);
    pbw_varint(&w, 9, 1);
    pbw_varint(&w, 10, (uint64_t)(int64_t)visibility);
    pbw_varint(&w, 11, (uint64_t)(uint32_t)file_type);
    shim_cm_async(call, CB_PUBLISH, EMSG_UCM_PUBLISH, false, NULL, &w, WAIT_MS, parse_publish, NULL);
    return call;
}

static uint64_t publish_message(const char *file, const char *preview, uint32_t consumer, const char *title, const char *description, int visibility,
                                const ParamStringArray *tags, bool workshop, uint32_t file_type, uint32_t video_provider, const char *video_account, const char *video_id) {
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    if (file && *file) pbw_str(&w, 2, file);
    if (preview && *preview) pbw_str(&w, 3, preview);
    pbw_varint(&w, 4, consumer ? consumer : app_id());
    pbw_str(&w, 5, title ? title : "");
    pbw_str(&w, 6, description ? description : "");
    put_tags(&w, 8, tags);
    pbw_varint(&w, 9, workshop);
    pbw_varint(&w, 10, (uint64_t)(int64_t)visibility);
    pbw_varint(&w, 11, file_type);
    if (video_provider) pbw_varint(&w, 13, video_provider);
    if (video_account && *video_account) pbw_str(&w, 14, video_account);
    if (video_id && *video_id) pbw_str(&w, 15, video_id);
    shim_cm_async(call, CB_PUBLISH, EMSG_UCM_PUBLISH, false, NULL, &w, WAIT_MS, parse_publish, NULL);
    return call;
}

static uint64_t SHIM_TC publish_file_v5(void *self, const char *file, const char *preview, uint32_t consumer, const char *title, const char *description, int visibility, const ParamStringArray *tags) {
    (void)self;
    if (!file || !*file) return 0;
    return publish_message(file, preview, consumer, title, description, visibility, tags, false, 0, 0, NULL, NULL);
}
static uint64_t SHIM_TC publish_workshop_v5(void *self, const char *file, const char *preview, uint32_t consumer, const char *title, const char *description, const ParamStringArray *tags) {
    (void)self;
    if (!file || !*file) return 0;
    return publish_message(file, preview, consumer, title, description, 0, tags, true, 0, 0, NULL, NULL);
}
typedef struct {
    uint64_t item;
    const char *file, *preview, *title, *description;
    int32_t visibility;
    const ParamStringArray *tags;
    uint8_t update_file, update_preview, update_title, update_description, update_visibility, update_tags;
} UpdateRequestV5;

static uint64_t SHIM_TC update_published_v5(void *self, UpdateRequestV5 req) {
    (void)self;
    const UpdateRequestV5 *r = &req;
    PbW w = {0};
    pbw_varint(&w, 1, app_id());
    pbw_fixed64(&w, 2, r->item);
    if (r->update_file && r->file) { pbw_str(&w, 3, r->file); pbw_varint(&w, 9, 1); }
    if (r->update_preview && r->preview) { pbw_str(&w, 4, r->preview); pbw_varint(&w, 10, 1); }
    if (r->update_title) { pbw_str(&w, 5, r->title ? r->title : ""); pbw_varint(&w, 11, 1); }
    if (r->update_description) { pbw_str(&w, 6, r->description ? r->description : ""); pbw_varint(&w, 12, 1); }
    if (r->update_tags) { put_tags(&w, 7, r->tags); pbw_varint(&w, 13, 1); }
    if (r->update_visibility) { pbw_varint(&w, 8, (uint64_t)(int64_t)r->visibility); pbw_varint(&w, 14, 1); }
    uint64_t call = shim_callresult_reserve();
    shim_cm_async(call, CB_UPDATE, EMSG_UCM_UPDATE, false, NULL, &w, WAIT_MS, parse_update, id_ctx(r->item));
    return call;
}

typedef struct {
    bool used;
    uint64_t handle, item;
    char *file, *preview, *title, *description, *change;
    int visibility;
    bool has_visibility;
    char **tags;
    int ntags;
    bool has_tags;
} LegacyUpdate;

static SRWLOCK g_lock = SRWLOCK_INIT;
static LegacyUpdate g_updates[MAX_LEGACY];
static uint64_t g_next = 0x6500;

static void clear_update(LegacyUpdate *u) {
    free(u->file); free(u->preview); free(u->title); free(u->description); free(u->change);
    for (int i = 0; i < u->ntags; i++) free(u->tags[i]);
    free(u->tags);
    memset(u, 0, sizeof *u);
}

static LegacyUpdate *find_update(uint64_t handle) {
    for (int i = 0; i < MAX_LEGACY; i++) if (g_updates[i].used && g_updates[i].handle == handle) return &g_updates[i];
    return NULL;
}

static void set_str(char **dst, const char *src) {
    free(*dst);
    *dst = src ? _strdup(src) : NULL;
}

static uint64_t SHIM_TC create_update_request(void *self, uint64_t item) {
    (void)self;
    uint64_t handle = ~0ULL;
    AcquireSRWLockExclusive(&g_lock);
    for (int i = 0; i < MAX_LEGACY; i++) {
        if (g_updates[i].used) continue;
        memset(&g_updates[i], 0, sizeof g_updates[i]);
        g_updates[i].used = true;
        g_updates[i].handle = handle = ++g_next;
        g_updates[i].item = item;
        break;
    }
    ReleaseSRWLockExclusive(&g_lock);
    return handle;
}

#define WITH_LEGACY(handle, body) do { \
    bool ok_ = false; \
    AcquireSRWLockExclusive(&g_lock); \
    LegacyUpdate *u = find_update(handle); \
    if (u) { body; ok_ = true; } \
    ReleaseSRWLockExclusive(&g_lock); \
    return ok_; } while (0)

static uint64_t SHIM_TC update_file(void *self, uint64_t h, const char *v) { (void)self; WITH_LEGACY(h, set_str(&u->file, v)); }
static uint64_t SHIM_TC update_preview(void *self, uint64_t h, const char *v) { (void)self; WITH_LEGACY(h, set_str(&u->preview, v)); }
static uint64_t SHIM_TC update_title(void *self, uint64_t h, const char *v) { (void)self; WITH_LEGACY(h, set_str(&u->title, v ? v : "")); }
static uint64_t SHIM_TC update_description(void *self, uint64_t h, const char *v) { (void)self; WITH_LEGACY(h, set_str(&u->description, v ? v : "")); }
static uint64_t SHIM_TC update_change(void *self, uint64_t h, const char *v) { (void)self; WITH_LEGACY(h, set_str(&u->change, v ? v : "")); }
static uint64_t SHIM_TC update_visibility(void *self, uint64_t h, int v) { (void)self; WITH_LEGACY(h, { u->visibility = v; u->has_visibility = true; }); }
static uint64_t SHIM_TC update_tags(void *self, uint64_t h, const ParamStringArray *tags) {
    (void)self;
    WITH_LEGACY(h, {
        for (int i = 0; i < u->ntags; i++) free(u->tags[i]);
        free(u->tags);
        u->tags = NULL;
        u->ntags = 0;
        u->has_tags = true;
        int n = tags && tags->strings && tags->n > 0 ? tags->n : 0;
        u->tags = n ? calloc((size_t)n, sizeof *u->tags) : NULL;
        for (int i = 0; u->tags && i < n; i++) if (tags->strings[i]) u->tags[u->ntags++] = _strdup(tags->strings[i]);
    });
}

static uint64_t SHIM_TC commit_update(void *self, uint64_t handle) {
    (void)self;
    PbW w = {0};
    uint64_t item = 0;
    AcquireSRWLockExclusive(&g_lock);
    LegacyUpdate *u = find_update(handle);
    if (!u) { ReleaseSRWLockExclusive(&g_lock); return 0; }
    item = u->item;
    pbw_varint(&w, 1, app_id());
    pbw_fixed64(&w, 2, item);
    if (u->file) { pbw_str(&w, 3, u->file); pbw_varint(&w, 9, 1); }
    if (u->preview) { pbw_str(&w, 4, u->preview); pbw_varint(&w, 10, 1); }
    if (u->title) { pbw_str(&w, 5, u->title); pbw_varint(&w, 11, 1); }
    if (u->description) { pbw_str(&w, 6, u->description); pbw_varint(&w, 12, 1); }
    if (u->has_tags) { for (int i = 0; i < u->ntags; i++) pbw_str(&w, 7, u->tags[i]); pbw_varint(&w, 13, 1); }
    if (u->has_visibility) { pbw_varint(&w, 8, (uint64_t)(int64_t)u->visibility); pbw_varint(&w, 14, 1); }
    if (u->change) pbw_str(&w, 15, u->change);
    clear_update(u);
    ReleaseSRWLockExclusive(&g_lock);
    uint64_t call = shim_callresult_reserve();
    shim_cm_async(call, CB_UPDATE, EMSG_UCM_UPDATE, false, NULL, &w, WAIT_MS, parse_update, id_ctx(item));
    return call;
}

static uint64_t SHIM_TC delete_published_file(void *self, uint64_t item) {
    (void)self;
    uint64_t call = shim_callresult_reserve();
    PbW w = {0};
    pbw_fixed64(&w, 1, item);
    pbw_varint(&w, 2, app_id());
    shim_cm_async(call, CB_DELETE, EMSG_UCM_DELETE, false, NULL, &w, WAIT_MS, parse_delete, id_ctx(item));
    return call;
}

void register_remotepublish(void) {
    const char *p = "STEAMREMOTESTORAGE_INTERFACE_VERSION";
    shim_register_family_ab(p, "PublishWorkshopFile", 32, (void *)publish_workshop_file);
    shim_register_family_ab(p, "PublishWorkshopFile", 24, (void *)publish_workshop_v5);
    shim_register_family_ab(p, "PublishFile", 28, (void *)publish_file_v5);
    shim_register_family_ab(p, "UpdatePublishedFile", 40, (void *)update_published_v5);
    shim_register_family(p, "CreatePublishedFileUpdateRequest", (void *)create_update_request);
    shim_register_family(p, "UpdatePublishedFileFile", (void *)update_file);
    shim_register_family(p, "UpdatePublishedFilePreviewFile", (void *)update_preview);
    shim_register_family(p, "UpdatePublishedFileTitle", (void *)update_title);
    shim_register_family(p, "UpdatePublishedFileDescription", (void *)update_description);
    shim_register_family(p, "UpdatePublishedFileVisibility", (void *)update_visibility);
    shim_register_family(p, "UpdatePublishedFileTags", (void *)update_tags);
    shim_register_family(p, "UpdatePublishedFileSetChangeDescription", (void *)update_change);
    shim_register_family(p, "CommitPublishedFileUpdate", (void *)commit_update);
    shim_register_family(p, "DeletePublishedFile", (void *)delete_published_file);
}
