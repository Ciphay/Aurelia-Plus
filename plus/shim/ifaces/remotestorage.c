#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "../shim.h"

#define MAX_LIST 4096
#define MAX_ASYNC 32

static char *g_names[MAX_LIST];
static int32_t g_sizes[MAX_LIST];
static int g_n_listed;

typedef struct { uint64_t handle; uint8_t *data; uint32_t len; } AsyncRead;
static AsyncRead g_async[MAX_ASYNC];

typedef struct { uint64_t handle; HANDLE file; char name[260]; } Stream;
static Stream g_streams[8];
static uint64_t g_next_stream = 1;

static bool cloud_root(char *out, size_t cap) {
    const Snapshot *s = shim_snapshot();
    if (!s || !s->has_app || !s->app.cloud_dir || !s->app.cloud_dir[0]) return false;
    return shim_win_path(s->app.cloud_dir, out, cap) > 0;
}

static bool full_path(const char *name, char *out, size_t cap) {
    if (!name || !name[0] || name[0] == '/' || name[0] == '\\' || strchr(name, ':')) return false;
    for (const char *p = name; *p; p++)
        if ((p == name || p[-1] == '/' || p[-1] == '\\') && p[0] == '.' && p[1] == '.' && (p[2] == 0 || p[2] == '/' || p[2] == '\\')) return false;
    char root[1024];
    if (!cloud_root(root, sizeof root)) return false;
    int n = snprintf(out, cap, "%s\\%s", root, name);
    if (n < 0 || (size_t)n >= cap) return false;
    for (char *p = out + strlen(root) + 1; *p; p++) if (*p == '/') *p = '\\';
    return true;
}

static void make_dirs(char *path) {
    for (char *p = path; *p; p++) {
        if (*p != '\\' || p == path || p[-1] == ':') continue;
        *p = 0;
        CreateDirectoryA(path, NULL);
        *p = '\\';
    }
}

static bool write_file(const char *name, const void *data, uint32_t cb) {
    char path[1400];
    if (!full_path(name, path, sizeof path)) return false;
    make_dirs(path);
    HANDLE f = CreateFileA(path, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    if (f == INVALID_HANDLE_VALUE) return false;
    DWORD done = 0;
    bool ok = cb == 0 || (WriteFile(f, data, cb, &done, NULL) && done == cb);
    CloseHandle(f);
    return ok;
}

static int32_t read_file(const char *name, void *buf, uint32_t offset, uint32_t cap) {
    char path[1400];
    if (!full_path(name, path, sizeof path)) return 0;
    HANDLE f = CreateFileA(path, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, NULL);
    if (f == INVALID_HANDLE_VALUE) return 0;
    DWORD got = 0;
    SetFilePointer(f, (LONG)offset, NULL, FILE_BEGIN);
    if (!ReadFile(f, buf, cap, &got, NULL)) got = 0;
    CloseHandle(f);
    return (int32_t)got;
}

static bool stat_file(const char *name, int32_t *size, int64_t *mtime) {
    char path[1400];
    WIN32_FILE_ATTRIBUTE_DATA d;
    if (!full_path(name, path, sizeof path) || !GetFileAttributesExA(path, GetFileExInfoStandard, &d) || (d.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY)) return false;
    if (size) *size = (int32_t)d.nFileSizeLow;
    if (mtime) {
        uint64_t t = ((uint64_t)d.ftLastWriteTime.dwHighDateTime << 32) | d.ftLastWriteTime.dwLowDateTime;
        *mtime = (int64_t)(t / 10000000ULL) - 11644473600LL;
    }
    return true;
}

static uint64_t SHIM_TC file_write(void *self, const char *name, const void *data, int32_t cb) { (void)self; return cb >= 0 && write_file(name, data, (uint32_t)cb); }
static int32_t SHIM_TC file_read(void *self, const char *name, void *buf, int32_t cap) { (void)self; return cap < 0 ? 0 : read_file(name, buf, 0, (uint32_t)cap); }
static uint64_t SHIM_TC file_exists(void *self, const char *name) { (void)self; return stat_file(name, NULL, NULL); }
static uint64_t SHIM_TC file_persisted(void *self, const char *name) { (void)self; return stat_file(name, NULL, NULL); }
static int32_t SHIM_TC file_size(void *self, const char *name) { (void)self; int32_t sz = 0; return stat_file(name, &sz, NULL) ? sz : 0; }
static int64_t SHIM_TC file_timestamp(void *self, const char *name) { (void)self; int64_t t = 0; return stat_file(name, NULL, &t) ? t : 0; }
static uint64_t SHIM_TC file_delete(void *self, const char *name) { (void)self; char p[1400]; return full_path(name, p, sizeof p) && DeleteFileA(p); }
static uint64_t SHIM_TC file_forget(void *self, const char *name) { (void)self; return name && *name; }
static uint64_t SHIM_TC ret_true(void *self) { (void)self; return 1; }
static uint32_t SHIM_TC sync_platforms(void *self, const char *name) { (void)self; return name && *name && stat_file(name, NULL, NULL) ? 0xFFFFFFFFu : 0; }
static uint64_t SHIM_TC set_sync_platforms(void *self, const char *name, uint32_t platforms) { (void)self; (void)platforms; return name && *name; }

static void free_list(void) {
    for (int i = 0; i < g_n_listed; i++) free(g_names[i]);
    g_n_listed = 0;
}

static void list_dir(const char *dir, const char *rel) {
    char pat[1500];
    WIN32_FIND_DATAA fd;
    snprintf(pat, sizeof pat, "%s\\*", dir);
    HANDLE h = FindFirstFileA(pat, &fd);
    if (h == INVALID_HANDLE_VALUE) return;
    do {
        if (!strcmp(fd.cFileName, ".") || !strcmp(fd.cFileName, "..")) continue;
        char sub[1500], r[600];
        snprintf(sub, sizeof sub, "%s\\%s", dir, fd.cFileName);
        snprintf(r, sizeof r, "%s%s%s", rel, rel[0] ? "/" : "", fd.cFileName);
        if (fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) { list_dir(sub, r); continue; }
        if (g_n_listed < MAX_LIST) { g_names[g_n_listed] = _strdup(r); g_sizes[g_n_listed] = (int32_t)fd.nFileSizeLow; g_n_listed++; }
    } while (FindNextFileA(h, &fd));
    FindClose(h);
}

static int32_t SHIM_TC file_count(void *self) {
    (void)self;
    char root[1024];
    free_list();
    if (cloud_root(root, sizeof root)) list_dir(root, "");
    return g_n_listed;
}
static const char *SHIM_TC file_name_and_size(void *self, int i, int32_t *size) {
    (void)self;
    if (i < 0 || i >= g_n_listed) { if (size) *size = 0; return ""; }
    if (size) *size = g_sizes[i];
    return g_names[i];
}
static uint64_t used_bytes(void) {
    uint64_t sum = 0;
    int n = file_count(NULL);
    for (int i = 0; i < n; i++) sum += (uint32_t)g_sizes[i];
    return sum;
}

static uint64_t cloud_quota(void) {
    const Snapshot *s = shim_snapshot();
    return s && s->has_app ? s->app.cloud_quota : 0;
}
static uint64_t SHIM_TC get_quota(void *self, uint64_t *total, uint64_t *available) {
    (void)self;
    uint64_t q = cloud_quota(), used = q ? used_bytes() : 0;
    if (total) *total = q;
    if (available) *available = q > used ? q - used : 0;
    return 1;
}
static uint64_t SHIM_TC get_quota32(void *self, int32_t *total, int32_t *available) {
    (void)self;
    uint64_t q = cloud_quota(), used = q ? used_bytes() : 0;
    uint64_t left = q > used ? q - used : 0;
    if (total) *total = q > 0x7fffffffULL ? 0x7fffffff : (int32_t)q;
    if (available) *available = left > 0x7fffffffULL ? 0x7fffffff : (int32_t)left;
    return 1;
}

static uint64_t SHIM_TC cloud_enabled_for_app(void *self) { (void)self; return cloud_quota() > 0; }

static uint64_t SHIM_TC file_write_async(void *self, const char *name, const void *data, uint32_t cb) {
    (void)self;
    int32_t result = write_file(name, data, cb) ? 1 : 2;
    return shim_callresult_post(1331, &result, 4);
}
static uint64_t SHIM_TC file_read_async(void *self, const char *name, uint32_t offset, uint32_t cb) {
    (void)self;
    if (!name || !*name) return 0;
    uint8_t *buf = malloc(cb ? cb : 1);
    int32_t got = buf ? read_file(name, buf, offset, cb) : 0;
    int32_t ok = buf && stat_file(name, NULL, NULL) ? 1 : 9;
    uint8_t res[24] = {0};
    uint64_t handle = shim_callresult_reserve();
    memcpy(res, &handle, 8); memcpy(res + 8, &ok, 4); memcpy(res + 12, &offset, 4); memcpy(res + 16, &got, 4);
    for (int i = 0; i < MAX_ASYNC; i++) {
        if (g_async[i].handle) continue;
        g_async[i].handle = handle; g_async[i].data = buf; g_async[i].len = (uint32_t)got;
        buf = NULL;
        break;
    }
    free(buf);
    shim_callresult_complete(handle, 1332, res, sizeof res);
    return handle;
}
static uint64_t SHIM_TC file_read_async_complete(void *self, uint64_t handle, void *buf, uint32_t cap) {
    (void)self;
    if (!handle) return 0;
    for (int i = 0; i < MAX_ASYNC; i++) {
        if (g_async[i].handle != handle) continue;
        uint32_t n = g_async[i].len < cap ? g_async[i].len : cap;
        if (buf && n) memcpy(buf, g_async[i].data, n);
        free(g_async[i].data);
        memset(&g_async[i], 0, sizeof g_async[i]);
        return 1;
    }
    return 0;
}

static uint64_t SHIM_TC stream_open(void *self, const char *name) {
    (void)self;
    char path[1400];
    if (!full_path(name, path, sizeof path)) return 0;
    for (int i = 0; i < 8; i++) {
        if (g_streams[i].handle) continue;
        make_dirs(path);
        strcat(path, ".part");
        HANDLE f = CreateFileA(path, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
        if (f == INVALID_HANDLE_VALUE) return 0;
        g_streams[i].handle = g_next_stream++;
        g_streams[i].file = f;
        snprintf(g_streams[i].name, sizeof g_streams[i].name, "%s", name);
        return g_streams[i].handle;
    }
    return 0;
}
static Stream *find_stream(uint64_t h) { for (int i = 0; h && i < 8; i++) if (g_streams[i].handle == h) return &g_streams[i]; return NULL; }
static uint64_t SHIM_TC stream_chunk(void *self, uint64_t h, const void *data, int32_t cb) {
    (void)self;
    Stream *s = find_stream(h);
    DWORD done = 0;
    return s && cb >= 0 && WriteFile(s->file, data, (DWORD)cb, &done, NULL) && done == (DWORD)cb;
}
static uint64_t stream_end(Stream *s, bool keep) {
    char path[1400], part[1410];
    bool ok = full_path(s->name, path, sizeof path);
    CloseHandle(s->file);
    if (ok) {
        snprintf(part, sizeof part, "%s.part", path);
        ok = keep ? MoveFileExA(part, path, MOVEFILE_REPLACE_EXISTING) : DeleteFileA(part);
    }
    memset(s, 0, sizeof *s);
    return ok;
}
static uint64_t SHIM_TC stream_close(void *self, uint64_t h) { (void)self; Stream *s = find_stream(h); return s && stream_end(s, true); }
static uint64_t SHIM_TC stream_cancel(void *self, uint64_t h) { (void)self; Stream *s = find_stream(h); return s && stream_end(s, false); }

enum { CHANGES_MAX = 64, CHANGE_NAME_MAX = 256, OP_CLOUD_CHANGES = 8 };
static SRWLOCK g_changes_lock = SRWLOCK_INIT;
static char g_changes[CHANGES_MAX][CHANGE_NAME_MAX];
static int g_nchanges = -1;

static void load_changes(void) {
    AcquireSRWLockExclusive(&g_changes_lock);
    bool done = g_nchanges >= 0;
    ReleaseSRWLockExclusive(&g_changes_lock);
    if (done) return;
    static uint8_t reply[4 + CHANGES_MAX * (2 + CHANGE_NAME_MAX)];
    uint8_t req = OP_CLOUD_CHANGES;
    size_t n = 0;
    int32_t status = -1;
    int count = 0;
    char names[CHANGES_MAX][CHANGE_NAME_MAX] = {{0}};
    if (bridge_call_fresh(0xFFFF, 13, &req, 1, &status, reply, sizeof reply, &n, 3000) && status == 0 && n >= 4) {
        uint32_t total;
        memcpy(&total, reply, 4);
        size_t at = 4;
        for (uint32_t i = 0; i < total && count < CHANGES_MAX && at + 2 <= n; i++) {
            uint16_t len;
            memcpy(&len, reply + at, 2);
            at += 2;
            if (at + len > n) break;
            size_t copy = len < CHANGE_NAME_MAX - 1 ? len : CHANGE_NAME_MAX - 1;
            memcpy(names[count], reply + at, copy);
            at += len;
            count++;
        }
    }
    AcquireSRWLockExclusive(&g_changes_lock);
    if (g_nchanges < 0) { memcpy(g_changes, names, sizeof g_changes); g_nchanges = count; }
    ReleaseSRWLockExclusive(&g_changes_lock);
}

static int SHIM_TC local_file_change_count(void *self) {
    (void)self;
    load_changes();
    AcquireSRWLockShared(&g_changes_lock);
    int n = g_nchanges > 0 ? g_nchanges : 0;
    ReleaseSRWLockShared(&g_changes_lock);
    return n;
}

static const char *SHIM_TC local_file_change(void *self, int index, int *change, int *path_type) {
    (void)self;
    load_changes();
    const char *name = "";
    AcquireSRWLockShared(&g_changes_lock);
    if (index >= 0 && index < g_nchanges) {
        name = g_changes[index];
        if (change) *change = 1;
        if (path_type) *path_type = 2;
    }
    ReleaseSRWLockShared(&g_changes_lock);
    return name;
}

void register_remotestorage(void) {
    shim_register_family("STEAMREMOTESTORAGE_INTERFACE_VERSION", "GetLocalFileChangeCount", (void *)local_file_change_count);
    shim_register_family("STEAMREMOTESTORAGE_INTERFACE_VERSION", "GetLocalFileChange", (void *)local_file_change);
    const char *p = "STEAMREMOTESTORAGE_INTERFACE_VERSION";
    shim_register_family(p, "FileWrite", (void *)file_write);
    shim_register_family(p, "FileRead", (void *)file_read);
    shim_register_family(p, "FileWriteAsync", (void *)file_write_async);
    shim_register_family(p, "FileReadAsync", (void *)file_read_async);
    shim_register_family(p, "FileReadAsyncComplete", (void *)file_read_async_complete);
    shim_register_family(p, "FileForget", (void *)file_forget);
    shim_register_family(p, "FileDelete", (void *)file_delete);
    shim_register_family(p, "FileExists", (void *)file_exists);
    shim_register_family(p, "FilePersisted", (void *)file_persisted);
    shim_register_family(p, "GetFileSize", (void *)file_size);
    shim_register_family(p, "GetFileTimestamp", (void *)file_timestamp);
    shim_register_family(p, "GetFileCount", (void *)file_count);
    shim_register_family(p, "GetFileNameAndSize", (void *)file_name_and_size);
    shim_register_family(p, "GetQuota", (void *)get_quota);
    for (int v = 1; v <= 12; v++) {
        char name[64];
        snprintf(name, sizeof name, "STEAMREMOTESTORAGE_INTERFACE_VERSION%03d", v);
        shim_register(name, "GetQuota", (void *)get_quota32);
    }
    shim_register_family(p, "IsCloudEnabledForAccount", (void *)ret_true);
    shim_register_family(p, "IsCloudEnabledForApp", (void *)cloud_enabled_for_app);
    shim_register_family(p, "SetCloudEnabledForApp", (void *)ret_true);
    shim_register_family(p, "SetSyncPlatforms", (void *)set_sync_platforms);
    shim_register_family(p, "GetSyncPlatforms", (void *)sync_platforms);
    shim_register_family(p, "FileWriteStreamOpen", (void *)stream_open);
    shim_register_family(p, "FileWriteStreamWriteChunk", (void *)stream_chunk);
    shim_register_family(p, "FileWriteStreamClose", (void *)stream_close);
    shim_register_family(p, "FileWriteStreamCancel", (void *)stream_cancel);
}
