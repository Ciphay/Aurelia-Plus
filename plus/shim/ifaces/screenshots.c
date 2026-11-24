#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

#include "../shim.h"

static bool g_hooked;
static uint32_t g_next_handle = 1;

static uint32_t crc32_update(uint32_t crc, const uint8_t *p, size_t n) {
    static uint32_t table[256];
    if (!table[1]) for (uint32_t i = 0; i < 256; i++) { uint32_t c = i; for (int k = 0; k < 8; k++) c = c & 1 ? 0xEDB88320u ^ (c >> 1) : c >> 1; table[i] = c; }
    crc = ~crc;
    for (size_t i = 0; i < n; i++) crc = table[(crc ^ p[i]) & 255] ^ (crc >> 8);
    return ~crc;
}

static void be32(uint8_t *o, uint32_t v) { o[0] = (uint8_t)(v >> 24); o[1] = (uint8_t)(v >> 16); o[2] = (uint8_t)(v >> 8); o[3] = (uint8_t)v; }

static bool write_chunk(FILE *f, const char *type, const uint8_t *data, uint32_t len) {
    uint8_t head[8], tail[4];
    be32(head, len);
    memcpy(head + 4, type, 4);
    uint32_t crc = crc32_update(0, (const uint8_t *)type, 4);
    if (len) crc = crc32_update(crc, data, len);
    be32(tail, crc);
    return fwrite(head, 1, 8, f) == 8 && (!len || fwrite(data, 1, len, f) == len) && fwrite(tail, 1, 4, f) == 4;
}

static bool write_png(const char *path, const uint8_t *rgb, int w, int h) {
    if (w <= 0 || h <= 0) return false;
    size_t row = (size_t)w * 3 + 1, raw_len = row * (size_t)h;
    uint8_t *raw = malloc(raw_len);
    if (!raw) return false;
    for (int y = 0; y < h; y++) { raw[row * (size_t)y] = 0; memcpy(raw + row * (size_t)y + 1, rgb + (size_t)y * (size_t)w * 3, (size_t)w * 3); }
    size_t blocks = (raw_len + 65534) / 65535, z_len = 2 + raw_len + blocks * 5 + 4;
    uint8_t *z = malloc(z_len);
    if (!z) { free(raw); return false; }
    size_t o = 0;
    z[o++] = 0x78; z[o++] = 0x01;
    uint32_t a = 1, b = 0;
    for (size_t i = 0; i < raw_len; i++) { a = (a + raw[i]) % 65521; b = (b + a) % 65521; }
    for (size_t pos = 0; pos < raw_len; ) {
        size_t n = raw_len - pos > 65535 ? 65535 : raw_len - pos;
        z[o++] = pos + n == raw_len;
        z[o++] = (uint8_t)n; z[o++] = (uint8_t)(n >> 8); z[o++] = (uint8_t)~n; z[o++] = (uint8_t)(~n >> 8);
        memcpy(z + o, raw + pos, n); o += n; pos += n;
    }
    be32(z + o, (b << 16) | a); o += 4;
    FILE *f = fopen(path, "wb");
    bool ok = f != NULL;
    if (ok) {
        static const uint8_t sig[8] = { 0x89, 'P', 'N', 'G', 0x0D, 0x0A, 0x1A, 0x0A };
        uint8_t ihdr[13];
        be32(ihdr, (uint32_t)w); be32(ihdr + 4, (uint32_t)h);
        ihdr[8] = 8; ihdr[9] = 2; ihdr[10] = 0; ihdr[11] = 0; ihdr[12] = 0;
        ok = fwrite(sig, 1, 8, f) == 8 && write_chunk(f, "IHDR", ihdr, 13) && write_chunk(f, "IDAT", z, (uint32_t)o) && write_chunk(f, "IEND", NULL, 0);
        fclose(f);
    }
    free(raw); free(z);
    return ok;
}

static bool screenshots_dir(char *out, size_t cap) {
    const Snapshot *s = shim_snapshot();
    char win[1024];
    if (!s || !s->has_app || !s->app.cloud_dir || !shim_win_path(s->app.cloud_dir, win, sizeof win)) return false;
    for (int cut = 0; cut < 2; cut++) { char *sl = strrchr(win, '\\'); if (sl) *sl = 0; else win[0] = 0; }
    int n = snprintf(out, cap, "%s%s760\\remote\\%u\\screenshots", win, win[0] ? "\\" : "", s->app.appid);
    if (n < 0 || (size_t)n >= cap) return false;
    for (char *p = out + 1; *p; p++) if (*p == '\\') { *p = 0; CreateDirectoryA(out, NULL); *p = '\\'; }
    CreateDirectoryA(out, NULL);
    return true;
}

static void post_ready(uint32_t handle, int32_t eresult) {
    uint8_t cb[8];
    memcpy(cb, &handle, 4); memcpy(cb + 4, &eresult, 4);
    shim_post_callback(2301, cb, sizeof cb);
}

static uint32_t SHIM_TC write_screenshot(void *self, const void *rgb, uint32_t cb, int w, int h) {
    (void)self;
    char dir[600], path[700];
    if (!rgb || w <= 0 || h <= 0 || (uint64_t)cb < (uint64_t)w * (uint64_t)h * 3 || !screenshots_dir(dir, sizeof dir)) return 0;
    uint32_t handle = g_next_handle++;
    snprintf(path, sizeof path, "%s\\%u_%u.png", dir, (unsigned)GetTickCount(), handle);
    bool ok = write_png(path, rgb, w, h);
    post_ready(handle, ok ? 1 : 2);
    shim_log("screenshot %s: %s", ok ? "saved" : "failed", path);
    return ok ? handle : 0;
}

static uint32_t SHIM_TC add_screenshot_to_library(void *self, const char *file, const char *thumb, int w, int h) {
    (void)self; (void)thumb; (void)w; (void)h;
    char dir[600], path[700];
    if (!file || !screenshots_dir(dir, sizeof dir)) return 0;
    const char *base = strrchr(file, '\\'); const char *sl = strrchr(file, '/');
    base = base && sl ? (base > sl ? base : sl) : base ? base : sl;
    uint32_t handle = g_next_handle++;
    snprintf(path, sizeof path, "%s\\%u_%s", dir, handle, base ? base + 1 : file);
    bool ok = CopyFileA(file, path, FALSE) != 0;
    post_ready(handle, ok ? 1 : 2);
    return ok ? handle : 0;
}

static void SHIM_TC hook_screenshots(void *self, uint8_t hook) { (void)self; g_hooked = hook; }
static uint64_t SHIM_TC is_hooked(void *self) { (void)self; return g_hooked; }
static void SHIM_TC trigger_screenshot(void *self) {
    (void)self;
    if (g_hooked) { uint8_t none = 0; shim_post_callback(2302, &none, 1); }
}
static uint64_t SHIM_TC valid_handle_true(void *self, uint32_t handle) { (void)self; return handle != 0 && handle < g_next_handle; }

void register_screenshots(void) {
    const char *p = "STEAMSCREENSHOTS_INTERFACE";
    shim_register_family(p, "WriteScreenshot", (void *)write_screenshot);
    shim_register_family(p, "AddScreenshotToLibrary", (void *)add_screenshot_to_library);
    shim_register_family(p, "HookScreenshots", (void *)hook_screenshots);
    shim_register_family(p, "IsScreenshotsHooked", (void *)is_hooked);
    shim_register_family(p, "TriggerScreenshot", (void *)trigger_screenshot);
    shim_register_family(p, "SetLocation", (void *)valid_handle_true);
    shim_register_family(p, "TagUser", (void *)valid_handle_true);
    shim_register_family(p, "TagPublishedFile", (void *)valid_handle_true);
}
