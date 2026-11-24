#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define _GNU_SOURCE
#include <windows.h>

#include "shim.h"

#define CHECK(c) do { if (!(c)) { printf("FAIL %s:%d %s\n", __FILE__, __LINE__, #c); return 0; } } while (0)

static const char FIXTURE[] =
    "{\"schema\":1,\"account\":{\"steamid\":76561198000000001,\"persona\":\"Tester\",\"country\":\"BR\"},"
    "\"apps\":[{\"appid\":10,\"name\":\"Alpha\",\"installed\":false},"
    "{\"appid\":2357570,\"name\":\"Overwatch\\u00ae\",\"installed\":true}],"
    "\"ticket\":{\"appid\":2357570,\"ownership_ticket_b64\":\"AQID\","
    "\"game_connect_token_b64\":\"AQIDBAUGBwgBTF4CAQAQARESExQ=\"},"
    "\"app\":{\"appid\":2357570,\"install_dir\":\"/games/common/Overwatch\",\"cloud_dir\":\"selftest-cloud\\\\remote\",\"cloud_quota\":1073741824,\"build_id\":26384159,\"beta_name\":\"public\",\"betas\":[{\"name\":\"public\",\"description\":\"\",\"build_id\":26384159,\"time_updated\":1700000000,\"private\":false},{\"name\":\"beta\",\"description\":\"Next patch\",\"build_id\":26400000,\"time_updated\":1700000100,\"private\":false},{\"name\":\"internal\",\"description\":\"Team\",\"build_id\":26500000,\"time_updated\":1700000200,\"private\":true}],"
    "\"dlc\":[{\"appid\":2357571,\"owned\":true,\"installed\":true},{\"appid\":2357572,\"owned\":true,\"installed\":false}]},"
    "\"friends\":[{\"steamid\":76561198000000042,\"persona\":\"Ana\",\"state\":1,\"game_appid\":570},"
    "{\"steamid\":76561198000000043,\"persona\":\"Beto\",\"state\":0,\"game_appid\":0}]}";

static void *vt(void *obj, int slot) { return ((void ***)obj)[0][slot]; }
static void *create(const char *name) { int rc = 0; return CreateInterface(name, &rc); }

typedef int (SHIM_TC *Fn_i_v)(void *);
typedef int (SHIM_TC *Fn_i_i)(void *, int);
typedef void *(SHIM_TC *Fn_p_iip)(void *, int, int, const char *);
typedef void *(SHIM_TC *Fn_p_ip)(void *, int, const char *);
typedef uint64_t *(SHIM_TC *Fn_hidden)(void *, uint64_t *);
typedef uint32_t (SHIM_TC *Fn_u_u)(void *, uint32_t);
typedef uint32_t (SHIM_TC *Fn_u_v)(void *);
typedef const char *(SHIM_TC *Fn_s_v)(void *);
typedef uint32_t (SHIM_TC *Fn_auth)(void *, void *, int, uint32_t *, const void *);
typedef uint64_t (SHIM_TC *Fn_b_ub)(void *, uint64_t, uint8_t *);
typedef uint64_t (SHIM_TC *Fn_b_ubuf)(void *, uint64_t, void *, int, int, uint8_t *);
typedef uint32_t (SHIM_TC *Fn_auth3)(void *, void *, int, uint32_t *);

static void *g_client, *g_user_obj, *g_apps, *g_utils;
static int g_pipe, g_user;

static int setup(void) {
    CHECK(shim_load_snapshot_json(FIXTURE, strlen(FIXTURE)));
    shim_init();
    g_client = create("SteamClient023");
    CHECK(g_client);
    g_pipe = ((Fn_i_v)vt(g_client, 0))(g_client);
    g_user = ((Fn_i_i)vt(g_client, 2))(g_client, g_pipe);
    CHECK(g_pipe != 0 && g_user != 0);
    g_user_obj = ((Fn_p_iip)vt(g_client, 5))(g_client, g_user, g_pipe, "SteamUser023");
    g_apps = ((Fn_p_iip)vt(g_client, 15))(g_client, g_user, g_pipe, "STEAMAPPS_INTERFACE_VERSION008");
    g_utils = ((Fn_p_ip)vt(g_client, 9))(g_client, g_pipe, "SteamUtils010");
    CHECK(g_user_obj && g_apps && g_utils);
    return 1;
}

static int t_frame_encoding(void) {
    uint8_t tok[32], buf[64];
    for (int i = 0; i < 32; i++) tok[i] = (uint8_t)i;
    size_t n = bridge_encode_hello(buf, sizeof buf, tok);
    CHECK(n == 39);
    CHECK(buf[0] == 35 && buf[1] == 0 && buf[2] == 0 && buf[3] == 0);
    CHECK(buf[4] == 1 && buf[5] == 1 && buf[6] == 0);
    CHECK(memcmp(buf + 7, tok, 32) == 0);
    CHECK(bridge_encode_snapshot_req(buf, sizeof buf) == 5);
    CHECK(buf[0] == 1 && buf[4] == 3);
    uint8_t ok[] = {3, 0, 0, 0, 2, 1, 0};
    BridgeFrame f; size_t used = 0;
    CHECK(bridge_decode(ok, sizeof ok, &f, &used) == 1 && f.tag == 2 && used == 7 && f.body_len == 2);
    CHECK(bridge_decode(ok, 5, &f, &used) == 0);
    uint8_t huge[] = {0, 0, 0, 2, 3};
    CHECK(bridge_decode(huge, sizeof huge, &f, &used) == -1);
    return 1;
}

static int t_call_encoding(void) {
    uint8_t out[64];
    uint8_t payload[2] = { 9, 8 };
    size_t n = bridge_encode_call(out, sizeof out, 5, 0xFFFF, 1, payload, 2);
    CHECK(n == 15);
    CHECK(out[0] == 11 && out[1] == 0 && out[2] == 0 && out[3] == 0);
    CHECK(out[4] == 5);
    CHECK(out[5] == 5 && out[6] == 0 && out[7] == 0 && out[8] == 0);
    CHECK(out[9] == 0xFF && out[10] == 0xFF && out[11] == 1 && out[12] == 0);
    CHECK(out[13] == 9 && out[14] == 8);
    CHECK(bridge_encode_call(out, 10, 5, 1, 1, payload, 2) == 0);
    int32_t status = 123;
    CHECK(!bridge_call(0xFFFF, 1, payload, 2, &status, NULL, 0, NULL, 100));
    return 1;
}

static int t_snapshot(void) {
    Snapshot s;
    CHECK(snapshot_load(FIXTURE, strlen(FIXTURE), &s));
    CHECK(s.schema == 1 && s.steamid == 76561198000000001ULL);
    CHECK(strcmp(s.persona, "Tester") == 0 && strcmp(s.country, "BR") == 0);
    CHECK(s.n_apps == 2 && s.apps[0].appid == 10 && !s.apps[0].installed);
    CHECK(s.apps[1].appid == 2357570 && s.apps[1].installed && strcmp(s.apps[1].name, "Overwatch\xC2\xAE") == 0);
    CHECK(s.has_ticket && s.ticket.appid == 2357570 && s.ticket.ownership_len == 3 && s.ticket.token_len == 20);
    CHECK(s.ticket.ownership[0] == 1 && s.ticket.token[19] == 20);
    CHECK(s.has_app && s.app.appid == 2357570 && !strcmp(s.app.install_dir, "/games/common/Overwatch"));
    CHECK(s.n_friends == 2 && s.friends[0].steamid == 76561198000000042ULL && !strcmp(s.friends[0].persona, "Ana") && s.friends[0].game_appid == 570);
    CHECK(s.app.n_dlc == 2 && s.app.dlc[0].appid == 2357571 && s.app.dlc[0].owned && s.app.dlc[0].installed && !s.app.dlc[1].installed);
    snapshot_free(&s);
    CHECK(!snapshot_load("{\"schema\":", 10, &s));
    return 1;
}

static int t_unknown_interface(void) {
    _putenv("AURELIA_SHIM_LOG=selftest-unknown.log");
    remove("selftest-unknown.log");
    int rc = 0;
    CHECK(CreateInterface("SteamNope999", &rc) == NULL && rc == 1);
    FILE *f = fopen("selftest-unknown.log", "rb");
    CHECK(f);
    char buf[512] = {0};
    fread(buf, 1, sizeof buf - 1, f);
    fclose(f);
    CHECK(strstr(buf, "SteamNope999") != NULL);
    _putenv("AURELIA_SHIM_LOG=");
    return 1;
}

static int t_user(void) {
    CHECK(setup());
    CHECK(((Fn_i_v)vt(g_user_obj, 0))(g_user_obj) == g_user);
    CHECK((((Fn_i_v)vt(g_user_obj, 1))(g_user_obj) & 0xff) == 0);
    uint64_t id = 0;
    uint64_t *r = ((Fn_hidden)vt(g_user_obj, 2))(g_user_obj, &id);
    CHECK(r == &id && id == 76561198000000001ULL);
    return 1;
}

static int t_bridge_down(void) {
    uint8_t tok[32] = {0};
    DWORD t0 = GetTickCount();
    CHECK(!bridge_connect("127.0.0.1", 1, tok, 500));
    CHECK(GetTickCount() - t0 < 1500);
    uint64_t id = 0;
    ((Fn_hidden)vt(g_user_obj, 2))(g_user_obj, &id);
    CHECK(id == 76561198000000001ULL);
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_cl_wr)(void *, const char *, const void *, int32_t);
typedef int32_t (SHIM_TC *Fn_cl_rd)(void *, const char *, void *, int32_t);
typedef uint64_t (SHIM_TC *Fn_cl_s)(void *, const char *);
typedef int32_t (SHIM_TC *Fn_cl_size)(void *, const char *);
typedef int64_t (SHIM_TC *Fn_cl_ts)(void *, const char *);
typedef int32_t (SHIM_TC *Fn_cl_cnt)(void *);
typedef const char *(SHIM_TC *Fn_cl_nm)(void *, int, int32_t *);
typedef uint64_t (SHIM_TC *Fn_cl_q)(void *, uint64_t *, uint64_t *);
typedef uint64_t (SHIM_TC *Fn_cl_q32)(void *, int32_t *, int32_t *);
typedef uint64_t (SHIM_TC *Fn_cl_wa)(void *, const char *, const void *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_cl_ra)(void *, const char *, uint32_t, uint32_t);
typedef uint64_t (SHIM_TC *Fn_cl_rac)(void *, uint64_t, void *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_cl_so)(void *, const char *);
typedef uint64_t (SHIM_TC *Fn_cl_sc)(void *, uint64_t, const void *, int32_t);
typedef uint64_t (SHIM_TC *Fn_cl_sx)(void *, uint64_t);

static int slot_of(const char *iface, const char *method) {
    for (int k = 0; k < g_n_ifaces; k++) {
        if (strcmp(g_ifaces[k].name, iface)) continue;
        for (int i = 0; i < g_ifaces[k].n_slots; i++)
            if (g_ifaces[k].slot_names[i] && !strcmp(g_ifaces[k].slot_names[i], method)) return i;
    }
    return -1;
}

static int t_remote_storage(void) {
    CHECK(setup());
    const char *iv = "STEAMREMOTESTORAGE_INTERFACE_VERSION016";
    void *rs = ((Fn_p_iip)vt(g_client, 17))(g_client, g_user, g_pipe, iv);
    CHECK(rs);
    int s_write = slot_of(iv, "FileWrite"), s_read = slot_of(iv, "FileRead"), s_exists = slot_of(iv, "FileExists");
    int s_size = slot_of(iv, "GetFileSize"), s_count = slot_of(iv, "GetFileCount"), s_name = slot_of(iv, "GetFileNameAndSize");
    int s_del = slot_of(iv, "FileDelete"), s_ts = slot_of(iv, "GetFileTimestamp"), s_quota = slot_of(iv, "GetQuota");
    CHECK(s_write >= 0 && s_read >= 0 && s_exists >= 0 && s_size >= 0 && s_count >= 0 && s_name >= 0 && s_del >= 0 && s_ts >= 0 && s_quota >= 0);
    CHECK((((Fn_cl_wr)vt(rs, s_write))(rs, "save/slot1.dat", "hello", 5) & 0xff) == 1);
    CHECK((((Fn_cl_s)vt(rs, s_exists))(rs, "save/slot1.dat") & 0xff) == 1);
    CHECK((((Fn_cl_s)vt(rs, s_exists))(rs, "save/none.dat") & 0xff) == 0);
    CHECK(((Fn_cl_size)vt(rs, s_size))(rs, "save/slot1.dat") == 5);
    CHECK(((Fn_cl_ts)vt(rs, s_ts))(rs, "save/slot1.dat") > 1700000000);
    char buf[16] = {0};
    CHECK(((Fn_cl_rd)vt(rs, s_read))(rs, "save/slot1.dat", buf, sizeof buf) == 5 && !memcmp(buf, "hello", 5));
    CHECK((((Fn_cl_wr)vt(rs, s_write))(rs, "../escape.dat", "x", 1) & 0xff) == 0);
    CHECK((((Fn_cl_wr)vt(rs, s_write))(rs, "C:\\evil.dat", "x", 1) & 0xff) == 0);
    CHECK(((Fn_cl_cnt)vt(rs, s_count))(rs) == 1);
    int32_t sz = 0;
    CHECK(!strcmp(((Fn_cl_nm)vt(rs, s_name))(rs, 0, &sz), "save/slot1.dat") && sz == 5);
    uint64_t total = 0, avail = 0;
    CHECK((((Fn_cl_q)vt(rs, s_quota))(rs, &total, &avail) & 0xff) == 1 && total == 1073741824ULL && avail < total && avail > 0);
    int s_enabled = slot_of(iv, "IsCloudEnabledForApp");
    CHECK(s_enabled >= 0 && (((Fn_i_v)vt(rs, s_enabled))(rs) & 0xff) == 1);

    int s_wa = slot_of(iv, "FileWriteAsync"), s_ra = slot_of(iv, "FileReadAsync"), s_rac = slot_of(iv, "FileReadAsyncComplete");
    CHECK(s_wa >= 0 && s_ra >= 0 && s_rac >= 0);
    uint64_t hw = ((Fn_cl_wa)vt(rs, s_wa))(rs, "save/async.dat", "abcdef", 6);
    int32_t wres = 0;
    uint8_t failed = 1;
    CHECK(hw && (((Fn_b_ubuf)vt(g_utils, 13))(g_utils, hw, &wres, sizeof wres, 1331, &failed) & 0xff) == 1 && wres == 1);
    uint64_t hr = ((Fn_cl_ra)vt(rs, s_ra))(rs, "save/async.dat", 2, 3);
    uint8_t rr[24] = {0};
    CHECK(hr && (((Fn_b_ubuf)vt(g_utils, 13))(g_utils, hr, rr, sizeof rr, 1332, &failed) & 0xff) == 1);
    int32_t eres, got; memcpy(&eres, rr + 8, 4); memcpy(&got, rr + 16, 4);
    CHECK(eres == 1 && got == 3);
    char part[8] = {0};
    CHECK((((Fn_cl_rac)vt(rs, s_rac))(rs, hr, part, sizeof part) & 0xff) == 1 && !memcmp(part, "cde", 3));

    int s_so = slot_of(iv, "FileWriteStreamOpen"), s_sc = slot_of(iv, "FileWriteStreamWriteChunk"), s_sx = slot_of(iv, "FileWriteStreamClose");
    CHECK(s_so >= 0 && s_sc >= 0 && s_sx >= 0);
    uint64_t st = ((Fn_cl_so)vt(rs, s_so))(rs, "save/stream.dat");
    CHECK(st != 0);
    CHECK((((Fn_cl_sc)vt(rs, s_sc))(rs, st, "12345", 5) & 0xff) == 1);
    CHECK((((Fn_cl_sc)vt(rs, s_sc))(rs, st, "678", 3) & 0xff) == 1);
    CHECK((((Fn_cl_s)vt(rs, s_exists))(rs, "save/stream.dat") & 0xff) == 0);
    CHECK((((Fn_cl_sx)vt(rs, s_sx))(rs, st) & 0xff) == 1);
    CHECK(((Fn_cl_size)vt(rs, s_size))(rs, "save/stream.dat") == 8);

    CHECK((((Fn_cl_s)vt(rs, s_del))(rs, "save/slot1.dat") & 0xff) == 1);
    CHECK((((Fn_cl_s)vt(rs, s_exists))(rs, "save/slot1.dat") & 0xff) == 0);

    {
        const char *old = "STEAMREMOTESTORAGE_INTERFACE_VERSION002";
        void *rs2 = ((Fn_p_iip)vt(g_client, 17))(g_client, g_user, g_pipe, old);
        CHECK(rs2);
        int s_q2 = slot_of(old, "GetQuota");
        CHECK(s_q2 == 6);
        int32_t pair[4] = { (int32_t)0xAAAAAAAA, (int32_t)0xBBBBBBBB, (int32_t)0xCCCCCCCC, (int32_t)0xDDDDDDDD };
        CHECK((((Fn_cl_q32)vt(rs2, s_q2))(rs2, &pair[0], &pair[2]) & 0xff) == 1);
        CHECK(pair[0] == 1073741824 && pair[1] == (int32_t)0xBBBBBBBB);
        CHECK(pair[2] > 0 && pair[2] <= 1073741824 && pair[3] == (int32_t)0xDDDDDDDD);
    }

    static const char NO_CLOUD[] =
        "{\"schema\":1,\"account\":{\"steamid\":76561198000000001,\"persona\":\"Tester\",\"country\":\"BR\"},\"apps\":[],"
        "\"app\":{\"appid\":500,\"install_dir\":\"/g\",\"cloud_dir\":\"selftest-cloud\\\\remote\",\"dlc\":[]}}";
    CHECK(shim_load_snapshot_json(NO_CLOUD, strlen(NO_CLOUD)));
    CHECK((((Fn_cl_q)vt(rs, s_quota))(rs, &total, &avail) & 0xff) == 1 && total == 0 && avail == 0);
    CHECK((((Fn_i_v)vt(rs, s_enabled))(rs) & 0xff) == 0);
    CHECK(shim_load_snapshot_json(FIXTURE, strlen(FIXTURE)));
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_lb_create)(void *, int, int);
typedef uint64_t (SHIM_TC *Fn_lb_join)(void *, uint64_t);
typedef uint64_t (SHIM_TC *Fn_lb_list)(void *);
typedef uint64_t (SHIM_TC *Fn_lb_setdata)(void *, uint64_t, const char *, const char *);
typedef const char *(SHIM_TC *Fn_lb_getdata)(void *, uint64_t, const char *);
typedef int (SHIM_TC *Fn_lb_members)(void *, uint64_t);
typedef uint64_t *(SHIM_TC *Fn_lb_member)(void *, uint64_t *, uint64_t, int);

typedef uint32_t (SHIM_TC *Fn_sk_listen)(void *, int, uint32_t, uint32_t, uint8_t);
typedef uint32_t (SHIM_TC *Fn_sk_conn)(void *, uint32_t, uint32_t, int);
typedef uint64_t (SHIM_TC *Fn_sk_send)(void *, uint32_t, const void *, uint32_t, uint8_t);
typedef uint64_t (SHIM_TC *Fn_sk_avail)(void *, uint32_t, uint32_t *);
typedef uint64_t (SHIM_TC *Fn_sk_recv)(void *, uint32_t, void *, uint32_t, uint32_t *);
typedef uint64_t (SHIM_TC *Fn_sk_lavail)(void *, uint32_t, uint32_t *, uint32_t *);
typedef uint64_t (SHIM_TC *Fn_sk_lrecv)(void *, uint32_t, void **, uint32_t *, uint32_t *);
typedef uint64_t (SHIM_TC *Fn_sk_destroy)(void *, uint32_t, uint8_t);
typedef int (SHIM_TC *Fn_sk_max)(void *, uint32_t);

static int t_sockets(void) {
    CHECK(setup());
    const char *iv = "SteamNetworking003";
    void *nw = ((Fn_p_iip)vt(g_client, slot_of("SteamClient023", "GetISteamNetworking")))(g_client, g_user, g_pipe, iv);
    CHECK(nw);
    uint32_t listen = ((Fn_sk_listen)vt(nw, slot_of(iv, "CreateListenSocket")))(nw, 0, 0, 27015, 0);
    CHECK(listen != 0);
    CHECK(((Fn_sk_conn)vt(nw, slot_of(iv, "CreateConnectionSocket")))(nw, 0x08080808u, 27015, 5) == 0);
    uint32_t c = ((Fn_sk_conn)vt(nw, slot_of(iv, "CreateConnectionSocket")))(nw, 0x7f000001u, 27015, 5);
    CHECK(c != 0);
    CHECK((((Fn_sk_send)vt(nw, slot_of(iv, "SendDataOnSocket")))(nw, c, "hello", 5, 1) & 0xff) == 1);
    uint32_t size = 0, srv = 0;
    CHECK((((Fn_sk_lavail)vt(nw, slot_of(iv, "IsDataAvailable")))(nw, listen, &size, &srv) & 0xff) == 1 && size == 5 && srv != 0);
    void *data = NULL;
    CHECK((((Fn_sk_lrecv)vt(nw, slot_of(iv, "RetrieveData")))(nw, listen, &data, &size, &srv) & 0xff) == 1 && size == 5 && !memcmp(data, "hello", 5));
    free(data);
    CHECK((((Fn_sk_send)vt(nw, slot_of(iv, "SendDataOnSocket")))(nw, srv, "ok", 2, 1) & 0xff) == 1);
    CHECK((((Fn_sk_avail)vt(nw, slot_of(iv, "IsDataAvailableOnSocket")))(nw, c, &size) & 0xff) == 1 && size == 2);
    char buf[16] = {0};
    CHECK((((Fn_sk_recv)vt(nw, slot_of(iv, "RetrieveDataFromSocket")))(nw, c, buf, sizeof buf, &size) & 0xff) == 1 && size == 2 && !memcmp(buf, "ok", 2));
    CHECK(((Fn_sk_max)vt(nw, slot_of(iv, "GetMaxPacketSize")))(nw, c) > 0);
    CHECK((((Fn_sk_destroy)vt(nw, slot_of(iv, "DestroySocket")))(nw, c, 0) & 0xff) == 1);
    CHECK((((Fn_sk_destroy)vt(nw, slot_of(iv, "DestroyListenSocket")))(nw, listen, 0) & 0xff) == 1);
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_pc_players)(void *);
typedef int (SHIM_TC *Fn_pc_clans)(void *);

static int t_players_and_clans(void) {
    CHECK(setup());
    const char *iv = "STEAMUSERSTATS_INTERFACE_VERSION007";
    void *st = ((Fn_p_iip)vt(g_client, slot_of("SteamClient023", "GetISteamUserStats")))(g_client, g_user, g_pipe, iv);
    CHECK(st);
    uint64_t h = ((Fn_pc_players)vt(st, slot_of(iv, "GetNumberOfCurrentPlayers")))(st);
    uint8_t res[8] = { 9, 9, 9, 9, 9, 9, 9, 9 }, failed = 0;
    CHECK(h);
    for (int i = 0; i < 100 && !(((Fn_b_ub)vt(g_utils, 11))(g_utils, h, &failed) & 0xff); i++) Sleep(50);
    CHECK((((Fn_b_ubuf)vt(g_utils, 13))(g_utils, h, res, sizeof res, 1107, &failed) & 0xff) == 1);
    CHECK(failed == 1);
    const char *fv = "SteamFriends005";
    void *fr = ((Fn_p_iip)vt(g_client, slot_of("SteamClient023", "GetISteamFriends")))(g_client, g_user, g_pipe, fv);
    CHECK(fr && ((Fn_pc_clans)vt(fr, slot_of(fv, "GetClanCount")))(fr) == 0);
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_df_call)(void *, uint64_t);
typedef uint64_t *(SHIM_TC *Fn_df_hidden)(void *, uint64_t *, uint64_t, int);

static int t_default_behavior(void) {
    CHECK(setup());
    const char *iv = "STEAMHTMLSURFACE_INTERFACE_VERSION_005";
    void *st = ((Fn_p_iip)vt(g_client, slot_of("SteamClient023", "GetISteamHTMLSurface")))(g_client, g_user, g_pipe, iv);
    CHECK(st);
    uint64_t h = ((uint64_t (SHIM_TC *)(void *, const char *, const char *))vt(st, slot_of(iv, "CreateBrowser")))(st, "agent", NULL);
    CHECK(h != 0);
    uint8_t failed = 0;
    CHECK((((Fn_b_ub)vt(g_utils, 11))(g_utils, h, &failed) & 0xff) == 1 && failed == 0);
    failed = 0;
    CHECK((((Fn_b_ub)vt(g_utils, 11))(g_utils, 0, &failed) & 0xff) == 1 && failed == 1);
    CHECK(((int (SHIM_TC *)(void *, uint64_t))vt(g_utils, 12))(g_utils, 0) == 2);
    uint8_t buf[8];
    failed = 0;
    CHECK((((Fn_b_ubuf)vt(g_utils, 13))(g_utils, h, buf, sizeof buf, 4501, &failed) & 0xff) == 1 && failed == 0);
    CHECK((((Fn_b_ubuf)vt(g_utils, 13))(g_utils, h, buf, sizeof buf, 4501, &failed) & 0xff) == 0);
    const char *fv = "SteamFriends005";
    void *fr = ((Fn_p_iip)vt(g_client, slot_of("SteamClient023", "GetISteamFriends")))(g_client, g_user, g_pipe, fv);
    CHECK(fr);
    uint64_t id = 0xDEADBEEFDEADBEEFULL;
    CHECK(((Fn_df_hidden)vt(fr, slot_of(fv, "GetFriendFromSourceByIndex")))(fr, &id, 1ULL, 0) == &id && id == 0);
    return 1;
}

typedef int (SHIM_TC *Fn_us_init)(void *, void *, int, uint64_t, uint32_t, uint32_t, uint32_t);
typedef void (SHIM_TC *Fn_us_term)(void *, uint32_t, uint32_t);
typedef int (SHIM_TC *Fn_us_begin)(void *, const void *, int, uint64_t);
typedef uint32_t (SHIM_TC *Fn_us_voice)(void *, uint32_t *);
typedef uint64_t (SHIM_TC *Fn_gs_conn)(void *, uint32_t, const void *, uint32_t, uint64_t *);
typedef void (SHIM_TC *Fn_gs_logon)(void *, const char *);
typedef uint64_t (SHIM_TC *Fn_gs_void)(void *);
typedef void (SHIM_TC *Fn_gs_voidv)(void *);

static int sl(const char *iface, const char *method) {
    int slot = slot_of(iface, method);
    if (slot < 0) printf("NO SLOT %s %s\n", iface, method);
    return slot;
}

static int wait_callback(int id, uint8_t *data, int cap);
static int call_taken(uint64_t h, int cb, void *out, int cap);

static int t_user_and_gameserver(void) {
    CHECK(setup());
    const char *uv = "SteamUser013";
    void *u = ((Fn_p_iip)vt(g_client, sl("SteamClient023", "GetISteamUser")))(g_client, g_user, g_pipe, uv);
    CHECK(u);
    uint8_t blob[512] = {0};
    int n = ((Fn_us_init)vt(u, sl(uv, "InitiateGameConnection")))(u, blob, sizeof blob, 0, 0x0100007f, 27015, 0);
    CHECK(n > 4 + 20);
    uint32_t token_len;
    memcpy(&token_len, blob, 4);
    CHECK(token_len == 20);
    ((Fn_us_term)vt(u, sl(uv, "TerminateGameConnection")))(u, 0x0100007f, 27015);
    ShimCallbackMsg stale;
    while (shim_next_callback(&stale)) shim_free_last_callback();
    int own = ((Fn_us_begin)vt(u, sl(uv, "BeginAuthSession")))(u, blob, n, 76561198000000001ULL);
    CHECK(own == 1);
    int other = ((Fn_us_begin)vt(u, sl(uv, "BeginAuthSession")))(u, blob, n, 76561198000000099ULL);
    CHECK(other == 1);
    int got143 = 0;
    while (shim_next_callback(&stale)) { if (stale.id == 143) got143++; shim_free_last_callback(); }
    CHECK(got143 == 2);
    uint8_t voice[16];
    uint32_t written = 0;
    CHECK(((Fn_us_voice)vt(u, sl(uv, "GetAvailableVoice")))(u, &written) == 2);
    ((void (SHIM_TC *)(void *))vt(u, sl(uv, "StartVoiceRecording")))(u);
    CHECK(((Fn_us_voice)vt(u, sl(uv, "GetAvailableVoice")))(u, &written) == 3);
    ((void (SHIM_TC *)(void *))vt(u, sl(uv, "StopVoiceRecording")))(u);
    CHECK(((Fn_us_voice)vt(u, sl(uv, "GetAvailableVoice")))(u, &written) == 3);
    Sleep(5200);
    CHECK(((Fn_us_voice)vt(u, sl(uv, "GetAvailableVoice")))(u, &written) == 2);
    (void)voice;

    const char *gv = "SteamGameServer010";
    void *gs = ((Fn_p_iip)vt(g_client, sl("SteamClient023", "GetISteamGameServer")))(g_client, g_user, g_pipe, gv);
    CHECK(gs);
    uint64_t id = 0;
    CHECK((((Fn_gs_conn)vt(gs, sl(gv, "SendUserConnectAndAuthenticate")))(gs, 0x0100007f, blob, (uint32_t)n, &id) & 0xff) == 0);
    CHECK(id == 0);
    uint8_t forged[64] = {0};
    uint32_t forged_len = 20;
    memcpy(forged, &forged_len, 4);
    uint64_t foreign = 76561198000000099ULL;
    memcpy(forged + 12, &foreign, 8);
    id = 7;
    CHECK((((Fn_gs_conn)vt(gs, sl(gv, "SendUserConnectAndAuthenticate")))(gs, 0x0100007f, forged, 24, &id) & 0xff) == 0 && id == 0);
    int approve = 0, deny = 0, failure = 0;
    while (shim_next_callback(&stale)) { if (stale.id == 201) approve++; if (stale.id == 202) deny++; shim_free_last_callback(); }
    CHECK(approve == 0 && deny == 2);
    CHECK((((Fn_gs_void)vt(gs, sl(gv, "BLoggedOn")))(gs) & 0xff) == 0);
    ((Fn_gs_voidv)vt(gs, sl(gv, "LogOn")))(gs);
    uint8_t fcb[16];
    failure = wait_callback(102, fcb, sizeof fcb);
    CHECK(failure == 1);
    CHECK((((Fn_gs_void)vt(gs, sl(gv, "BLoggedOn")))(gs) & 0xff) == 0);
    return 1;
}

typedef int (SHIM_TC *Fn_x_filter9)(void *, char *, uint32_t, const char *, uint8_t);
typedef int (SHIM_TC *Fn_x_filter10)(void *, int, uint64_t, const char *, char *, uint32_t);
typedef uint32_t (SHIM_TC *Fn_x_u)(void *);
typedef uint32_t (SHIM_TC *Fn_x_apps)(void *, uint32_t *, uint32_t);
typedef int (SHIM_TC *Fn_x_name)(void *, uint32_t, char *, int);
typedef uint32_t (SHIM_TC *Fn_x_ticket)(void *, uint32_t, void *, uint32_t, uint32_t *, uint32_t *, uint32_t *, uint32_t *);
typedef const char *(SHIM_TC *Fn_x_univ)(void *, int);
typedef uint64_t (SHIM_TC *Fn_x_beta)(void *, char *, int);
typedef uint64_t (SHIM_TC *Fn_x_numbetas)(void *, int32_t *, int32_t *);
typedef uint64_t (SHIM_TC *Fn_x_betainfo)(void *, int32_t, uint32_t *, uint32_t *, char *, int32_t, char *, int32_t);

static int t_utils_apps_client_extras(void) {
    CHECK(setup());
    char out[32];
    const char *u9n = "SteamUtils009", *u10n = "SteamUtils010";
    void *utils9 = ((Fn_p_ip)vt(g_client, slot_of("SteamClient023", "GetISteamUtils")))(g_client, g_pipe, u9n);
    void *utils10 = ((Fn_p_ip)vt(g_client, slot_of("SteamClient023", "GetISteamUtils")))(g_client, g_pipe, u10n);
    CHECK(utils9 && utils10);
    CHECK(((Fn_x_filter9)vt(utils9, slot_of(u9n, "FilterText")))(utils9, out, sizeof out, "hello world", 0) == 11 && !strcmp(out, "hello world"));
    CHECK(((Fn_x_filter10)vt(utils10, slot_of(u10n, "FilterText")))(utils10, 1, 0, "text", out, sizeof out) == 0 && !strcmp(out, "text"));
    CHECK((((Fn_x_u)vt(utils10, slot_of(u10n, "GetCurrentBatteryPower")))(utils10) & 0xff) == 255);

    const char *an = "STEAMAPPS_INTERFACE_VERSION008";
    CHECK(((Fn_x_u)vt(g_apps, slot_of(an, "GetAppBuildId")))(g_apps) == 26384159u);
    char beta[16] = "x";
    CHECK((((Fn_x_beta)vt(g_apps, slot_of(an, "GetCurrentBetaName")))(g_apps, beta, sizeof beta) & 0xff) == 0 && beta[0] == 0);

    {
        int32_t avail = -1, priv = -1;
        CHECK((((Fn_x_numbetas)vt(g_apps, slot_of(an, "GetNumBetas")))(g_apps, &avail, &priv) & 0xff) == 3 && avail == 2 && priv == 1);
        uint32_t flags = 0, build = 0;
        char name[32] = "", desc[32] = "";
        CHECK(((Fn_x_betainfo)vt(g_apps, slot_of(an, "GetBetaInfo")))(g_apps, 0, &flags, &build, name, sizeof name, desc, sizeof desc) & 0xff);
        CHECK(!strcmp(name, "public") && flags == (2 | 1 | 8) && build == 26384159u);
        CHECK(((Fn_x_betainfo)vt(g_apps, slot_of(an, "GetBetaInfo")))(g_apps, 1, &flags, &build, name, sizeof name, desc, sizeof desc) & 0xff);
        CHECK(!strcmp(name, "beta") && !strcmp(desc, "Next patch") && flags == 2 && build == 26400000u);
        CHECK(((Fn_x_betainfo)vt(g_apps, slot_of(an, "GetBetaInfo")))(g_apps, 2, &flags, &build, name, sizeof name, desc, sizeof desc) & 0xff);
        CHECK(!strcmp(name, "internal") && flags == 4);
        CHECK(!(((Fn_x_betainfo)vt(g_apps, slot_of(an, "GetBetaInfo")))(g_apps, 3, &flags, &build, name, sizeof name, desc, sizeof desc) & 0xff));
        CHECK(!(((Fn_x_betainfo)vt(g_apps, slot_of(an, "GetBetaInfo")))(g_apps, -1, &flags, &build, name, sizeof name, desc, sizeof desc) & 0xff));
        typedef uint64_t (SHIM_TC *Fn_x_setbeta)(void *, const char *);
        CHECK(!(((Fn_x_setbeta)vt(g_apps, slot_of(an, "SetActiveBeta")))(g_apps, "internal") & 0xff));
        CHECK(!(((Fn_x_setbeta)vt(g_apps, slot_of(an, "SetActiveBeta")))(g_apps, "does-not-exist") & 0xff));
        CHECK(!(((Fn_x_setbeta)vt(g_apps, slot_of(an, "SetActiveBeta")))(g_apps, NULL) & 0xff));
    }

    void *c17 = create("SteamClient017");
    CHECK(c17);
    const char *ln = "STEAMAPPLIST_INTERFACE_VERSION001";
    void *al = ((Fn_p_iip)vt(c17, slot_of("SteamClient017", "GetISteamAppList")))(c17, g_user, g_pipe, ln);
    CHECK(al);
    CHECK(((Fn_x_u)vt(al, slot_of(ln, "GetNumInstalledApps")))(al) == 1);
    uint32_t ids[4] = {0};
    CHECK(((Fn_x_apps)vt(al, slot_of(ln, "GetInstalledApps")))(al, ids, 4) == 1 && ids[0] == 2357570u);
    char name[64];
    CHECK(((Fn_x_name)vt(al, slot_of(ln, "GetAppName")))(al, 2357570u, name, sizeof name) == (int)strlen("Overwatch\xC2\xAE") + 1);
    CHECK(((Fn_x_name)vt(al, slot_of(ln, "GetAppName")))(al, 424242u, name, sizeof name) == -1);

    const char *tn = "STEAMAPPTICKET_INTERFACE_VERSION001";
    void *tk = ((Fn_p_iip)vt(c17, slot_of("SteamClient017", "GetISteamGenericInterface")))(c17, g_user, g_pipe, tn);
    CHECK(tk);
    uint8_t tbuf[256];
    uint32_t off_app = 0, off_id = 0, off_sig = 0, cb_sig = 0;
    uint32_t n = ((Fn_x_ticket)vt(tk, slot_of(tn, "GetAppOwnershipTicketData")))(tk, 2357570u, tbuf, sizeof tbuf, &off_app, &off_id, &off_sig, &cb_sig);
    CHECK(n == 3 && off_app == 16 && off_id == 8 && cb_sig == 0);
    CHECK(((Fn_x_ticket)vt(tk, slot_of(tn, "GetAppOwnershipTicketData")))(tk, 999u, tbuf, sizeof tbuf, &off_app, &off_id, &off_sig, &cb_sig) == 0);

    void *c6 = create("SteamClient006");
    CHECK(c6 && !strcmp(((Fn_x_univ)vt(c6, slot_of("SteamClient006", "GetUniverseName")))(c6, 1), "Public"));
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_fg_dep)(void *, uint64_t, uint64_t *, uint32_t *, uint16_t *);
typedef uint64_t (SHIM_TC *Fn_fg_two)(void *, uint64_t, uint64_t *, uint32_t *, uint16_t *, uint16_t *);

static int t_friend_game_variants(void) {
    CHECK(setup());
    const char *fv = "SteamFriends005";
    void *fr = ((Fn_p_iip)vt(g_client, slot_of("SteamClient023", "GetISteamFriends")))(g_client, g_user, g_pipe, fv);
    CHECK(fr);
    int dep = slot_of(fv, "Deprecated_GetFriendGamePlayed"), cur = slot_of(fv, "GetFriendGamePlayed2");
    uint64_t gid = 0; uint32_t ip = 9; uint16_t port = 9, qport = 9;
    if (dep >= 0) {
        CHECK((((Fn_fg_dep)vt(fr, dep))(fr, 76561198000000042ULL, &gid, &ip, &port) & 0xff) == 1 && gid == 570 && ip == 0 && port == 0);
        CHECK((((Fn_fg_dep)vt(fr, dep))(fr, 76561198000000043ULL, &gid, &ip, &port) & 0xff) == 0);
    }
    if (cur >= 0) {
        gid = 0;
        CHECK((((Fn_fg_two)vt(fr, cur))(fr, 76561198000000042ULL, &gid, &ip, &port, &qport) & 0xff) == 1 && gid == 570 && qport == 0);
    }
    return 1;
}

typedef uint32_t (SHIM_TC *Fn_sc_write)(void *, const void *, uint32_t, int, int);
typedef uint64_t (SHIM_TC *Fn_sc_tag)(void *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_sc_v)(void *);
typedef void (SHIM_TC *Fn_sc_hook)(void *, uint8_t);
typedef void (SHIM_TC *Fn_sc_trig)(void *);

static int t_screenshots(void) {
    CHECK(setup());
    const char *iv = "STEAMSCREENSHOTS_INTERFACE_VERSION003";
    void *sc = ((Fn_p_iip)vt(g_client, slot_of("SteamClient023", "GetISteamScreenshots")))(g_client, g_user, g_pipe, iv);
    CHECK(sc);
    ShimCallbackMsg stale;
    while (shim_next_callback(&stale)) shim_free_last_callback();
    uint8_t rgb[2 * 2 * 3] = { 255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255 };
    uint32_t h = ((Fn_sc_write)vt(sc, slot_of(iv, "WriteScreenshot")))(sc, rgb, sizeof rgb, 2, 2);
    CHECK(h != 0);
    CHECK(((Fn_sc_write)vt(sc, slot_of(iv, "WriteScreenshot")))(sc, rgb, 4, 2, 2) == 0);
    int ready = 0;
    while (shim_next_callback(&stale)) { if (stale.id == 2301) ready++; shim_free_last_callback(); }
    CHECK(ready == 1);
    WIN32_FIND_DATAA fd;
    HANDLE f = FindFirstFileA("760\\remote\\2357570\\screenshots\\*.png", &fd);
    CHECK(f != INVALID_HANDLE_VALUE);
    char path[600];
    snprintf(path, sizeof path, "760\\remote\\2357570\\screenshots\\%s", fd.cFileName);
    FindClose(f);
    FILE *png = fopen(path, "rb");
    CHECK(png != NULL);
    uint8_t head[24] = {0};
    CHECK(fread(head, 1, sizeof head, png) == sizeof head);
    fclose(png);
    static const uint8_t sig[8] = { 0x89, 'P', 'N', 'G', 0x0D, 0x0A, 0x1A, 0x0A };
    CHECK(!memcmp(head, sig, 8) && !memcmp(head + 12, "IHDR", 4) && head[19] == 2 && head[23] == 2);
    CHECK((((Fn_sc_tag)vt(sc, slot_of(iv, "TagUser")))(sc, h) & 0xff) == 1);
    CHECK((((Fn_sc_tag)vt(sc, slot_of(iv, "TagUser")))(sc, 9999) & 0xff) == 0);
    ((Fn_sc_hook)vt(sc, slot_of(iv, "HookScreenshots")))(sc, 0);
    CHECK((((Fn_sc_v)vt(sc, slot_of(iv, "IsScreenshotsHooked")))(sc) & 0xff) == 0);
    ((Fn_sc_hook)vt(sc, slot_of(iv, "HookScreenshots")))(sc, 1);
    CHECK((((Fn_sc_v)vt(sc, slot_of(iv, "IsScreenshotsHooked")))(sc) & 0xff) == 1);
    ((Fn_sc_trig)vt(sc, slot_of(iv, "TriggerScreenshot")))(sc);
    int requested = 0;
    while (shim_next_callback(&stale)) { if (stale.id == 2302) requested++; shim_free_last_callback(); }
    CHECK(requested == 1);
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_in_handle)(void *, const char *);
typedef uint64_t (SHIM_TC *Fn_in_flag)(void *);
typedef uint32_t (SHIM_TC *Fn_in_count)(void *, uint64_t *);
#ifdef _WIN64
typedef uint64_t (SHIM_TC *Fn_in_digital)(void *, uint64_t, uint64_t);
#else
typedef void *(SHIM_TC *Fn_in_digital)(void *, void *, uint64_t, uint64_t);
#endif
typedef void *(SHIM_TC *Fn_in_hidden)(void *, void *, uint64_t, uint64_t);
typedef const char *(SHIM_TC *Fn_in_str)(void *, int);

static int t_input(void) {
    CHECK(setup());
    const char *iv = "SteamInput006";
    void *in = ((Fn_p_iip)vt(g_client, slot_of("SteamClient023", "GetISteamInput")))(g_client, g_user, g_pipe, iv);
    CHECK(in);
    CHECK((((Fn_in_flag)vt(in, slot_of(iv, "Init")))(in) & 0xff) == 1);
    uint64_t a = ((Fn_in_handle)vt(in, slot_of(iv, "GetDigitalActionHandle")))(in, "Jump");
    uint64_t b = ((Fn_in_handle)vt(in, slot_of(iv, "GetDigitalActionHandle")))(in, "Jump");
    uint64_t c = ((Fn_in_handle)vt(in, slot_of(iv, "GetDigitalActionHandle")))(in, "Fire");
    CHECK(a != 0 && a == b && a != c);
    CHECK(((Fn_in_handle)vt(in, slot_of(iv, "GetActionSetHandle")))(in, "InGame") != 0);
    uint64_t handles[16];
    CHECK(((Fn_in_count)vt(in, slot_of(iv, "GetConnectedControllers")))(in, handles) == 0);
    const char *cv1 = "STEAMCONTROLLER_INTERFACE_VERSION";
    void *c1 = ((Fn_p_iip)vt(g_client, slot_of("SteamClient023", "GetISteamController")))(g_client, g_user, g_pipe, cv1);
    CHECK(c1);
    CHECK((((Fn_in_flag)vt(c1, slot_of(cv1, "Init")))(c1) & 0xff) == 1);
#ifdef _WIN64
    CHECK(((Fn_in_digital)vt(in, slot_of(iv, "GetDigitalActionData")))(in, 0, a) == 0);
#else
    uint8_t d2[4] = { 0xAA, 0xAA, 0xAA, 0xAA };
    CHECK(((Fn_in_digital)vt(in, slot_of(iv, "GetDigitalActionData")))(in, d2, 0, a) == d2 && d2[0] == 0 && d2[1] == 0 && d2[2] == 0xAA);
#endif
    uint8_t analog[20]; memset(analog, 0xAA, sizeof analog);
    CHECK(((Fn_in_hidden)vt(in, slot_of(iv, "GetAnalogActionData")))(in, analog, 0, a) == analog);
    for (int i = 0; i < 16; i++) CHECK(analog[i] == 0);
    CHECK(analog[16] == 0xAA);
    uint8_t motion[44]; memset(motion, 0xAA, sizeof motion);
    CHECK(((Fn_in_hidden)vt(in, slot_of(iv, "GetMotionData")))(in, motion, 0, 0) == motion);
    for (int i = 0; i < 40; i++) CHECK(motion[i] == 0);
    CHECK(motion[40] == 0xAA);
    const char *s = ((Fn_in_str)vt(in, slot_of(iv, "GetStringForActionOrigin")))(in, 1);
    CHECK(s && s[0] == 0);
    return 1;
}

typedef uint32_t (SHIM_TC *Fn_ht_create)(void *, int, const char *);
typedef uint64_t (SHIM_TC *Fn_ht_pair)(void *, uint32_t, const char *, const char *);
typedef uint64_t (SHIM_TC *Fn_ht_body)(void *, uint32_t, const char *, const uint8_t *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_ht_send)(void *, uint32_t, uint64_t *);
typedef uint64_t (SHIM_TC *Fn_ht_u)(void *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_ht_size)(void *, uint32_t, uint32_t *);
typedef uint64_t (SHIM_TC *Fn_ht_data)(void *, uint32_t, uint8_t *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_ht_hsize)(void *, uint32_t, const char *, uint32_t *);
typedef uint64_t (SHIM_TC *Fn_ht_hval)(void *, uint32_t, const char *, uint8_t *, uint32_t);

static int wait_call(uint64_t call) {
    uint8_t failed = 0;
    for (int i = 0; i < 150; i++) {
        if ((((Fn_b_ub)vt(g_utils, 11))(g_utils, call, &failed) & 0xff) == 1) return 1;
        Sleep(100);
    }
    return 0;
}

static int t_http(void) {
    CHECK(setup());
    const char *base = getenv("HTTP_TEST_URL");
    CHECK(base != NULL);
    const char *iv = "STEAMHTTP_INTERFACE_VERSION003";
    void *ht = ((Fn_p_iip)vt(g_client, slot_of("SteamClient023", "GetISteamHTTP")))(g_client, g_user, g_pipe, iv);
    CHECK(ht);
    char url[256];
    snprintf(url, sizeof url, "%s/hello", base);
    uint32_t h = ((Fn_ht_create)vt(ht, slot_of(iv, "CreateHTTPRequest")))(ht, 1, url);
    CHECK(h != 0);
    CHECK((((Fn_ht_pair)vt(ht, slot_of(iv, "SetHTTPRequestHeaderValue")))(ht, h, "X-Test", "abc") & 0xff) == 1);
    CHECK((((Fn_ht_pair)vt(ht, slot_of(iv, "SetHTTPRequestGetOrPostParameter")))(ht, h, "q", "a b") & 0xff) == 1);
    uint64_t call = 0;
    CHECK((((Fn_ht_send)vt(ht, slot_of(iv, "SendHTTPRequest")))(ht, h, &call) & 0xff) == 1 && call != 0);
    CHECK(wait_call(call));
    uint8_t res[32] = {0}, failed = 1;
    CHECK((((Fn_b_ubuf)vt(g_utils, 13))(g_utils, call, res, sizeof res, 2101, &failed) & 0xff) == 1 && failed == 0);
    uint32_t rh, status, size; memcpy(&rh, res, 4); memcpy(&status, res + 20, 4); memcpy(&size, res + 24, 4);
    CHECK(rh == h && res[16] == 1 && status == 200 && size == 5);
    uint32_t got = 0;
    CHECK((((Fn_ht_size)vt(ht, slot_of(iv, "GetHTTPResponseBodySize")))(ht, h, &got) & 0xff) == 1 && got == 5);
    char body[16] = {0};
    CHECK((((Fn_ht_data)vt(ht, slot_of(iv, "GetHTTPResponseBodyData")))(ht, h, (uint8_t *)body, sizeof body) & 0xff) == 1 && !strcmp(body, "hello"));
    uint32_t hs = 0;
    CHECK((((Fn_ht_hsize)vt(ht, slot_of(iv, "GetHTTPResponseHeaderSize")))(ht, h, "X-Reply", &hs) & 0xff) == 1 && hs == 4);
    char hv[16] = {0};
    CHECK((((Fn_ht_hval)vt(ht, slot_of(iv, "GetHTTPResponseHeaderValue")))(ht, h, "x-reply", (uint8_t *)hv, sizeof hv) & 0xff) == 1 && !strcmp(hv, "yes"));
    CHECK((((Fn_ht_u)vt(ht, slot_of(iv, "ReleaseHTTPRequest")))(ht, h) & 0xff) == 1);

    snprintf(url, sizeof url, "%s/post", base);
    h = ((Fn_ht_create)vt(ht, slot_of(iv, "CreateHTTPRequest")))(ht, 3, url);
    CHECK(h != 0);
    CHECK((((Fn_ht_body)vt(ht, slot_of(iv, "SetHTTPRequestRawPostBody")))(ht, h, "text/plain", (const uint8_t *)"payload", 7) & 0xff) == 1);
    call = 0;
    CHECK((((Fn_ht_send)vt(ht, slot_of(iv, "SendHTTPRequest")))(ht, h, &call) & 0xff) == 1 && wait_call(call));
    memset(res, 0, sizeof res);
    CHECK((((Fn_b_ubuf)vt(g_utils, 13))(g_utils, call, res, sizeof res, 2101, &failed) & 0xff) == 1);
    memcpy(&status, res + 20, 4);
    CHECK(res[16] == 1 && status == 201);

    h = ((Fn_ht_create)vt(ht, slot_of(iv, "CreateHTTPRequest")))(ht, 1, "http://127.0.0.1:1/nothing");
    call = 0;
    CHECK((((Fn_ht_send)vt(ht, slot_of(iv, "SendHTTPRequest")))(ht, h, &call) & 0xff) == 1 && wait_call(call));
    memset(res, 0, sizeof res);
    CHECK((((Fn_b_ubuf)vt(g_utils, 13))(g_utils, call, res, sizeof res, 2101, &failed) & 0xff) == 1 && res[16] == 0);
    CHECK(((Fn_ht_create)vt(ht, slot_of(iv, "CreateHTTPRequest")))(ht, 0, url) == 0);
    return 1;
}

typedef int64_t (SHIM_TC *Fn_nu_time)(void *);
typedef int (SHIM_TC *Fn_nu_relay)(void *, int32_t *);
typedef uint32_t (SHIM_TC *Fn_ns_listen)(void *, const void *, int, const void *);

static int t_networking_utils(void) {
    CHECK(setup());
    const char *iv = "SteamNetworkingUtils003";
    void *nu = ((Fn_p_iip)vt(g_client, slot_of("SteamClient023", "GetISteamGenericInterface")))(g_client, g_user, g_pipe, iv);
    CHECK(nu);
    int64_t t1 = ((Fn_nu_time)vt(nu, slot_of(iv, "GetLocalTimestamp")))(nu);
    Sleep(20);
    int64_t t2 = ((Fn_nu_time)vt(nu, slot_of(iv, "GetLocalTimestamp")))(nu);
    CHECK(t1 > 0 && t2 - t1 >= 15000 && t2 - t1 < 5000000);
    int32_t status[68];
    memset(status, 0x55, sizeof status);
    CHECK(((Fn_nu_relay)vt(nu, slot_of(iv, "GetRelayNetworkStatus")))(nu, status) == 100 && status[0] == 100 && status[3] == -102);
    CHECK(((const char *)(status + 4))[0] != 0);
    {
        typedef uint64_t (SHIM_TC *Fn_cfs)(void *, const char *);
        void *ut = ((Fn_p_iip)vt(g_client, slot_of("SteamClient023", "GetISteamGenericInterface")))(g_client, g_user, g_pipe, "SteamUtils010");
        CHECK(ut);
        uint8_t r4[4];
        CHECK(call_taken(((Fn_cfs)vt(ut, slot_of("SteamUtils010", "CheckFileSignature")))(ut, "left4dead2.exe"), 705, r4, sizeof r4));
        int32_t sig;
        memcpy(&sig, r4, 4);
        CHECK(sig == 3);
    }
    const char *sv = "SteamNetworkingSockets012";
    void *ns = ((Fn_p_iip)vt(g_client, slot_of("SteamClient023", "GetISteamGenericInterface")))(g_client, g_user, g_pipe, sv);
    if (ns) CHECK(((Fn_ns_listen)vt(ns, slot_of(sv, "ConnectByIPAddress")))(ns, NULL, 0, NULL) == 0);
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_rp_set)(void *, const char *, const char *);
typedef const char *(SHIM_TC *Fn_rp_get)(void *, uint64_t, const char *);

static int t_rich_presence(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    void *fr = ((Fn_p_iip)vt(client, 8))(client, 1, 1, "SteamFriends017");
    CHECK(fr);
    CHECK((((Fn_rp_set)vt(fr, 43))(fr, "status", "In Menus") & 0xff) == 1);
    CHECK((((Fn_rp_set)vt(fr, 43))(fr, "steam_display", "#Menu") & 0xff) == 1);
    CHECK((((Fn_rp_set)vt(fr, 43))(fr, "status", "In Game") & 0xff) == 1);
    CHECK(!strcmp(((Fn_rp_get)vt(fr, 45))(fr, 76561198000000001ULL, "status"), "In Game"));
    CHECK(!strcmp(((Fn_rp_get)vt(fr, 45))(fr, 76561198000000001ULL, "nothing"), ""));
    CHECK(((Fn_i_v)vt(fr, 46)) != NULL);
    ((Fn_i_v)vt(fr, 44))(fr);
    CHECK(!strcmp(((Fn_rp_get)vt(fr, 45))(fr, 76561198000000001ULL, "status"), ""));
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_fr_has)(void *, uint64_t, int);
typedef int (SHIM_TC *Fn_fr_i)(void *, int);
typedef uint64_t *(SHIM_TC *Fn_fr_idx)(void *, uint64_t *, int, int);
typedef int (SHIM_TC *Fn_fr_id)(void *, uint64_t);
typedef const char *(SHIM_TC *Fn_fr_name)(void *, uint64_t);
typedef uint64_t (SHIM_TC *Fn_fr_game)(void *, uint64_t, uint8_t *);

static int t_friends(void) {
    CHECK(setup());
    void *fr = ((Fn_p_iip)vt(g_client, 8))(g_client, g_user, g_pipe, "SteamFriends017");
    CHECK(fr);
    CHECK(!strcmp(((Fn_s_v)vt(fr, 0))(fr), "Tester"));
    CHECK(((Fn_i_v)vt(fr, 2))(fr) == 1);
    CHECK(((Fn_fr_i)vt(fr, 3))(fr, 4) == 2);
    CHECK(((Fn_fr_i)vt(fr, 3))(fr, 0) == 0);
    uint64_t id = 0;
    CHECK(((Fn_fr_idx)vt(fr, 4))(fr, &id, 1, 4) == &id && id == 76561198000000043ULL);
    CHECK(((Fn_fr_id)vt(fr, 5))(fr, 76561198000000042ULL) == 3);
    CHECK(((Fn_fr_id)vt(fr, 5))(fr, 1) == 0);
    CHECK(((Fn_fr_id)vt(fr, 6))(fr, 76561198000000042ULL) == 1);
    CHECK(!strcmp(((Fn_fr_name)vt(fr, 7))(fr, 76561198000000042ULL), "Ana"));
    CHECK(!strcmp(((Fn_fr_name)vt(fr, 7))(fr, 76561198000000001ULL), "Tester"));
    uint8_t info[24] = {0};
    CHECK((((Fn_fr_game)vt(fr, 8))(fr, 76561198000000042ULL, info) & 0xff) == 1);
    uint64_t gid; memcpy(&gid, info, 8);
    CHECK(gid == 570);
    CHECK((((Fn_fr_game)vt(fr, 8))(fr, 76561198000000043ULL, info) & 0xff) == 0);
    CHECK((((Fn_fr_has)vt(fr, 17))(fr, 76561198000000042ULL, 4) & 0xff) == 1);
    CHECK((((Fn_fr_has)vt(fr, 17))(fr, 5, 4) & 0xff) == 0);
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_dlc)(void *, int, uint32_t *, uint8_t *, char *, int);
typedef uint32_t (SHIM_TC *Fn_dir)(void *, uint32_t, char *, uint32_t);

static int t_apps(void) {
    CHECK((((Fn_u_u)vt(g_apps, 6))(g_apps, 10) & 0xff) == 1);
    CHECK((((Fn_u_u)vt(g_apps, 6))(g_apps, 2357570) & 0xff) == 1);
    CHECK((((Fn_u_u)vt(g_apps, 6))(g_apps, 99999) & 0xff) == 0);
    CHECK((((Fn_i_v)vt(g_apps, 0))(g_apps) & 0xff) == 1);
    CHECK((((Fn_u_u)vt(g_apps, 7))(g_apps, 2357570) & 0xff) == 0);
    uint64_t owner = 0;
    CHECK(((Fn_hidden)vt(g_apps, 20))(g_apps, &owner) == &owner && owner == 76561198000000001ULL);
    CHECK(strcmp(((Fn_s_v)vt(g_apps, 4))(g_apps), "english") == 0);
    CHECK(((Fn_u_v)vt(g_apps, 10))(g_apps) == 2);
    uint32_t dlc_id = 0; uint8_t avail = 0; char dlc_name[64] = "x";
    CHECK((((Fn_dlc)vt(g_apps, 11))(g_apps, 1, &dlc_id, &avail, dlc_name, sizeof dlc_name) & 0xff) == 1);
    CHECK(dlc_id == 2357572 && avail == 1);
    CHECK((((Fn_dlc)vt(g_apps, 11))(g_apps, 2, &dlc_id, &avail, dlc_name, sizeof dlc_name) & 0xff) == 0);
    CHECK((((Fn_u_u)vt(g_apps, 7))(g_apps, 2357571) & 0xff) == 1);
    CHECK((((Fn_u_u)vt(g_apps, 7))(g_apps, 2357572) & 0xff) == 0);
    CHECK((((Fn_u_u)vt(g_apps, 6))(g_apps, 2357572) & 0xff) == 1);
    char dir[260] = {0};
    CHECK(((Fn_dir)vt(g_apps, 18))(g_apps, 2357570, dir, sizeof dir) == strlen("Z:\\games\\common\\Overwatch") + 1);
    CHECK(!strcmp(dir, "Z:\\games\\common\\Overwatch"));
    CHECK(((Fn_dir)vt(g_apps, 18))(g_apps, 999, dir, sizeof dir) == 0);
    return 1;
}

static int t_utils(void) {
    CHECK(((Fn_u_v)vt(g_utils, 9))(g_utils) == 2357570);
    CHECK(strcmp(((Fn_s_v)vt(g_utils, 4))(g_utils), "BR") == 0);
    CHECK((((Fn_i_v)vt(g_utils, 17))(g_utils) & 0xff) == 0);
    CHECK(((Fn_u_v)vt(g_utils, 2))(g_utils) == 1);
    return 1;
}

uint64_t Steam_GetAPICallResult(int pipe, uint64_t call, void *out, int cb, int expected, int *failed);

static int t_call_results(void) {
    ShimCallbackMsg stale;
    while (shim_next_callback(&stale)) shim_free_last_callback();
    uint32_t payload[3] = { 7, 8, 9 };
    uint64_t h1 = shim_callresult_post(1101, payload, sizeof payload);
    uint64_t h2 = shim_callresult_post(1102, payload, 8);
    CHECK(h1 && h2 && h1 != h2);
    uint8_t failed = 1;
    CHECK((((Fn_b_ub)vt(g_utils, 11))(g_utils, h1, &failed) & 0xff) == 1 && failed == 0);
    CHECK((((Fn_b_ub)vt(g_utils, 11))(g_utils, 12345, &failed) & 0xff) == 0);
    ShimCallbackMsg m;
    int completed = 0;
    while (shim_next_callback(&m)) {
        if (m.id == 703 && m.size == 16) {
            uint64_t handle; int32_t cb; uint32_t sz;
            memcpy(&handle, m.param, 8); memcpy(&cb, m.param + 8, 4); memcpy(&sz, m.param + 12, 4);
            CHECK(handle == h1 || handle == h2);
            CHECK(cb == (handle == h1 ? 1101 : 1102) && sz == (handle == h1 ? 12u : 8u));
            completed++;
        }
        shim_free_last_callback();
    }
    CHECK(completed == 2);
    uint32_t got[3] = {0};
    CHECK((((Fn_b_ubuf)vt(g_utils, 13))(g_utils, h1, got, sizeof got, 1102, &failed) & 0xff) == 0);
    CHECK((((Fn_b_ubuf)vt(g_utils, 13))(g_utils, h1, got, sizeof got, 1101, &failed) & 0xff) == 1 && failed == 0);
    CHECK(got[0] == 7 && got[1] == 8 && got[2] == 9);
    CHECK((((Fn_b_ubuf)vt(g_utils, 13))(g_utils, h1, got, sizeof got, 1101, &failed) & 0xff) == 0);
    CHECK((((Fn_b_ub)vt(g_utils, 11))(g_utils, h2, &failed) & 0xff) == 1);
    uint64_t h3 = shim_callresult_post(1101, payload, sizeof payload);
    uint8_t io = 1;
    memset(got, 0, sizeof got);
    CHECK((Steam_GetAPICallResult(g_pipe, h3, got, sizeof got, 1102, (int *)&io) & 0xff) == 0);
    CHECK((Steam_GetAPICallResult(g_pipe, h3, got, sizeof got, 1101, (int *)&io) & 0xff) == 1 && io == 0);
    CHECK(got[0] == 7 && got[1] == 8 && got[2] == 9);
    CHECK((Steam_GetAPICallResult(g_pipe, h3, got, sizeof got, 1101, (int *)&io) & 0xff) == 0);
    while (shim_next_callback(&stale)) shim_free_last_callback();
    return 1;
}

uint64_t Steam_GetGSHandle(int user, int pipe);
uint64_t Steam_GSBLoggedOn(void *h), Steam_GSBSecure(void *h), Steam_GSGetSteamID(void *h);
uint64_t Steam_GSSetServerType(void *h, int32_t appid, uint32_t flags, uint32_t ip, uint16_t port, const char *dir, const char *version);
uint64_t Steam_GSUpdateStatus(void *h, int players, int max_players, int bots, const char *name, const char *map);
uint64_t Steam_InitiateGameConnection(int user, int pipe, void *blob, int cb_max, uint64_t server, uint64_t game, uint32_t ip, uint16_t port, uint8_t secure);
void Steam_TerminateGameConnection(int user, int pipe, uint32_t ip, uint16_t port);
uint64_t Steam_BConnected(int user, int pipe), Steam_BLoggedOn(int user, int pipe);
void Steam_SetLocalIPBinding(uint32_t ip, uint16_t port);
void Steam_GSSetSpawnCount(void *h, uint32_t count);
static int t_legacy_exports(void) {
    void *h = (void *)(uintptr_t)Steam_GetGSHandle(g_user, g_pipe);
    CHECK(h != NULL);
    CHECK(Steam_GSBLoggedOn(NULL) == 0 && Steam_GSBSecure(NULL) == 0 && Steam_GSGetSteamID(NULL) == 0);
    CHECK((Steam_GSBLoggedOn(h) & 0xff) == 0 && Steam_GSGetSteamID(h) == 0 && (Steam_GSBSecure(h) & 0xff) == 0);
    CHECK(Steam_GSSetServerType(NULL, 480, 0, 0x0100007f, 27015, "dir", "1.0.0.0") == 0);
    CHECK(Steam_GSSetServerType(h, -1, 0, 0x0100007f, 27015, "dir", "1.0.0.0") == 0);
    CHECK((Steam_GSSetServerType(h, 480, 0, 0x0100007f, 27015, "dir", "1.0.0.0") & 0xff) == 1);
    CHECK((Steam_GSUpdateStatus(h, 1, 16, 0, "name", "map") & 0xff) == 1 && Steam_GSUpdateStatus(NULL, 1, 16, 0, "n", "m") == 0);
    uint8_t blob[512];
    CHECK(Steam_InitiateGameConnection(g_user, g_pipe, blob, sizeof blob, 0, (uint64_t)0xffffffffu, 0x0100007f, 27015, 0) == 0);
    uint64_t n = Steam_InitiateGameConnection(g_user, g_pipe, blob, sizeof blob, 0, 480, 0x0100007f, 27015, 0);
    CHECK(n > 0 && n <= sizeof blob);
    Steam_TerminateGameConnection(g_user, g_pipe, 0x0100007f, 27015);
    CHECK(Steam_BConnected(g_user, g_pipe) == 0 && Steam_BLoggedOn(g_user, g_pipe) == 0);
    CHECK(shim_local_ip_binding() == 0);
    Steam_SetLocalIPBinding(0x7f000001, 0);
    CHECK(shim_local_ip_binding() == 0x7f000001);
    SOCKET us = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    CHECK(shim_bind_local_udp((uintptr_t)us, 0, false));
    struct sockaddr_in bound = {0};
    int bl = sizeof bound;
    CHECK(getsockname(us, (struct sockaddr *)&bound, &bl) == 0 && ntohl(bound.sin_addr.s_addr) == 0x7f000001);
    closesocket(us);
    Steam_SetLocalIPBinding(0, 0);
    Steam_GSSetSpawnCount(NULL, 3);
    Steam_GSSetSpawnCount(h, 3);
    Steam_GSSetSpawnCount(h, 4);
    return 1;
}

static int t_auth_ticket(void) {
    uint8_t buf[512];
    uint32_t n = 0;
    uint32_t h = ((Fn_auth)vt(g_user_obj, 13))(g_user_obj, buf, sizeof buf, &n, NULL);
    CHECK(h != 0 && n == 59);
    uint32_t v;
    memcpy(&v, buf, 4);        CHECK(v == 20);
    CHECK(buf[4] == 1 && buf[23] == 20);
    memcpy(&v, buf + 24, 4);   CHECK(v == 24);
    memcpy(&v, buf + 52, 4);   CHECK(v == 3);
    CHECK(buf[56] == 1 && buf[58] == 3);
    uint32_t a1, b1, seq1;
    memcpy(&a1, buf + 36, 4); memcpy(&b1, buf + 40, 4); memcpy(&seq1, buf + 48, 4);
    CHECK((a1 != 0 || b1 != 0) && seq1 == 1);
    n = 0;
    CHECK(((Fn_auth)vt(g_user_obj, 13))(g_user_obj, buf, 10, &n, NULL) == 0);
    uint8_t buf2[512];
    CHECK(((Fn_auth)vt(g_user_obj, 13))(g_user_obj, buf2, sizeof buf2, &n, NULL) != 0);
    uint32_t a2, b2, seq2;
    memcpy(&a2, buf2 + 36, 4); memcpy(&b2, buf2 + 40, 4); memcpy(&seq2, buf2 + 48, 4);
    CHECK(seq2 == 2 && (a2 != a1 || b2 != b1));
    return 1;
}

static int t_runframe_noops(void) {
    CHECK(setup());
    _putenv("AURELIA_SHIM_LOG=selftest-runframe.log");
    remove("selftest-runframe.log");
    static const char *const names[] = { "SteamInput006", "SteamController008" };
    for (size_t i = 0; i < 2; i++) {
        void *o = create(names[i]);
        CHECK(o);
        const IfaceDesc *d = NULL;
        for (int k = 0; k < g_n_ifaces; k++) if (!strcmp(g_ifaces[k].name, names[i])) d = &g_ifaces[k];
        int slot = -1;
        for (int k = 0; d && k < d->n_slots; k++) if (d->slot_names[k] && !strcmp(d->slot_names[k], "RunFrame")) slot = k;
        CHECK(slot >= 0);
        ((Fn_i_v)vt(o, slot))(o);
    }
    FILE *f = fopen("selftest-runframe.log", "rb");
    if (f) {
        char buf[512] = {0};
        fread(buf, 1, sizeof buf - 1, f);
        fclose(f);
        CHECK(strstr(buf, "UNIMPLEMENTED") == NULL);
    }
    _putenv("AURELIA_SHIM_LOG=");
    return 1;
}

typedef int (SHIM_TC *Fn_i_pi)(void *, char *, int);

uint64_t Steam_BGetCallback(int pipe, void *msg, int *call);
uint64_t Steam_FreeLastCallback(int pipe);

#pragma pack(push, 8)
typedef struct { int32_t user; int32_t id; uint8_t *param; int32_t size; } TestCallbackMsg;
#pragma pack(pop)

static int drain(int want, uint8_t *out, int cap, int *seen, int *n_seen) {
    TestCallbackMsg m;
    int call = 0, found = -1;
    *n_seen = 0;
    while (Steam_BGetCallback(g_pipe, &m, &call) & 0xff) {
        seen[(*n_seen)++] = m.id;
        if (m.id == want && found < 0) { found = m.size; memcpy(out, m.param, (size_t)(m.size < cap ? m.size : cap)); }
        Steam_FreeLastCallback(g_pipe);
    }
    return found;
}

static int t_callbacks(void) {
    CHECK(setup());
    uint8_t buf[512], param[64];
    int seen[32], n = 0;
    int sz = drain(101, param, sizeof param, seen, &n);
    CHECK(sz >= 0);
    uint32_t n_ticket = 0;
    uint32_t h = ((Fn_auth)vt(g_user_obj, 13))(g_user_obj, buf, sizeof buf, &n_ticket, NULL);
    CHECK(h != 0);
    sz = drain(163, param, sizeof param, seen, &n);
    CHECK(sz == 8);
    uint32_t got_h; int32_t result;
    memcpy(&got_h, param, 4); memcpy(&result, param + 4, 4);
    CHECK(got_h == h && result == 1);
    CHECK(drain(163, param, sizeof param, seen, &n) < 0 && n == 0);
    typedef uint32_t (SHIM_TC *Fn_b)(void *);
    CHECK((((Fn_b)vt(((Fn_p_iip)vt(g_client, 13))(g_client, g_user, g_pipe, "STEAMUSERSTATS_INTERFACE_VERSION012"), 0))(((Fn_p_iip)vt(g_client, 13))(g_client, g_user, g_pipe, "STEAMUSERSTATS_INTERFACE_VERSION012")) & 0xff) == 1);
    sz = -1;
    for (int k = 0; k < 100 && sz < 0; k++) { sz = drain(1101, param, sizeof param, seen, &n); if (sz < 0) Sleep(50); }
    CHECK(sz == 24);
    uint64_t gameid, sid; memcpy(&gameid, param, 8); memcpy(&result, param + 8, 4); memcpy(&sid, param + 16, 8);
    CHECK(gameid == 2357570 && result == 1 && sid == 76561198000000001ULL);
    return 1;
}

static int t_overwatch_first_calls(void) {
    CHECK(setup());
    _putenv("AURELIA_SHIM_LOG=selftest-ow.log");
    remove("selftest-ow.log");
    char cmd[16] = "x";
    CHECK(((Fn_i_pi)vt(g_apps, 26))(g_apps, cmd, sizeof cmd) == 0 && cmd[0] == 0);
    void *stats = ((Fn_p_iip)vt(g_client, 13))(g_client, g_user, g_pipe, "STEAMUSERSTATS_INTERFACE_VERSION012");
    CHECK(stats);
    CHECK((((Fn_i_v)vt(stats, 0))(stats) & 0xff) == 1);
    void *shots = ((Fn_p_iip)vt(g_client, 18))(g_client, g_user, g_pipe, "STEAMSCREENSHOTS_INTERFACE_VERSION003");
    CHECK(shots);
    ((Fn_i_i)vt(shots, 3))(shots, 1);
    FILE *f = fopen("selftest-ow.log", "rb");
    if (f) {
        char buf[512] = {0};
        fread(buf, 1, sizeof buf - 1, f);
        fclose(f);
        CHECK(strstr(buf, "UNIMPLEMENTED") == NULL);
    }
    _putenv("AURELIA_SHIM_LOG=");
    return 1;
}

static int t_ticket_override(void) {
    CHECK(setup());
    FILE *f = fopen("selftest-ticket.bin", "wb");
    CHECK(f);
    fwrite("\x01\x02\x03\x04\x05", 1, 5, f);
    fclose(f);
    _putenv("AURELIA_SHIM_TICKET_FILE=selftest-ticket.bin");
    uint8_t buf[64] = {0};
    uint32_t n = 0;
    uint32_t h = ((Fn_auth)vt(g_user_obj, 13))(g_user_obj, buf, sizeof buf, &n, NULL);
    _putenv("AURELIA_SHIM_TICKET_FILE=");
    CHECK(h != 0 && n == 5 && buf[0] == 1 && buf[4] == 5);
    return 1;
}

typedef uint32_t (SHIM_TC *Fn_auth5)(void *, void *, int, uint32_t *, const void *);

static int t_auth_identity_logged(void) {
    CHECK(setup());
    _putenv("AURELIA_SHIM_LOG=selftest-identity.log");
    remove("selftest-identity.log");
    uint8_t ident[136] = {0};
    int32_t type = 2, size = 9;
    memcpy(ident, &type, 4);
    memcpy(ident + 4, &size, 4);
    memcpy(ident + 8, "overwatch", 9);
    uint8_t buf[512];
    uint32_t n = 0;
    CHECK(((Fn_auth5)vt(g_user_obj, 13))(g_user_obj, buf, sizeof buf, &n, ident) != 0);
    FILE *f = fopen("selftest-identity.log", "rb");
    CHECK(f);
    char log[1024] = {0};
    fread(log, 1, sizeof log - 1, f);
    fclose(f);
    _putenv("AURELIA_SHIM_LOG=");
    CHECK(strstr(log, "identity type=2 size=9") != NULL && strstr(log, "overwatch") != NULL);
    return 1;
}

static int t_auth_ticket_v022(void) {
    CHECK(setup());
    void *u22 = ((Fn_p_iip)vt(g_client, 5))(g_client, g_user, g_pipe, "SteamUser022");
    CHECK(u22);
    uint8_t buf[512];
    uint32_t n = 0;
    CHECK(((Fn_auth3)vt(u22, 13))(u22, buf, sizeof buf, &n) != 0 && n == 59);
    return 1;
}

static int t_trace(void) {
    CHECK(setup());
    _putenv("AURELIA_SHIM_LOG=selftest-trace.log");
    _putenv("AURELIA_SHIM_TRACE=1");
    remove("selftest-trace.log");
    void *u = ((Fn_p_iip)vt(g_client, 5))(g_client, g_user, g_pipe, "SteamUser023");
    CHECK(u);
    ((Fn_i_v)vt(u, 1))(u);
    uint8_t cb[24];
    int call = 0;
    Steam_BGetCallback(g_pipe, cb, &call);
    _putenv("AURELIA_SHIM_TRACE=");
    _putenv("AURELIA_SHIM_LOG=");
    FILE *f = fopen("selftest-trace.log", "rb");
    CHECK(f);
    char log[2048] = {0};
    fread(log, 1, sizeof log - 1, f);
    fclose(f);
    CHECK(strstr(log, "TRACE GetISteamUser version=SteamUser023") != NULL);
    CHECK(strstr(log, "TRACE BLoggedOn") != NULL);
    CHECK(strstr(log, "TRACE Steam_BGetCallback") != NULL);
    return 1;
}

typedef int (SHIM_TC *Fn_i_ss)(void *, const char *, const char *);
typedef int (SHIM_TC *Fn_i_s)(void *, const char *);

static int t_overwatch_post_login_calls(void) {
    CHECK(setup());
    _putenv("AURELIA_SHIM_LOG=selftest-postlogin.log");
    remove("selftest-postlogin.log");
    void *friends = ((Fn_p_iip)vt(g_client, 8))(g_client, g_user, g_pipe, "SteamFriends017");
    void *stats = ((Fn_p_iip)vt(g_client, 13))(g_client, g_user, g_pipe, "STEAMUSERSTATS_INTERFACE_VERSION012");
    CHECK(friends && stats);
    CHECK((((Fn_i_ss)vt(friends, 43))(friends, "status", "In Menus") & 0xff) == 1);
    CHECK(((Fn_i_i)vt(friends, 3))(friends, 4) == 2);
    CHECK((((Fn_i_s)vt(stats, 7))(stats, "ACH_TEST") & 0xff) == 0);
    CHECK((((Fn_i_v)vt(stats, 10))(stats) & 0xff) == 1);
    FILE *f = fopen("selftest-postlogin.log", "rb");
    if (f) {
        char buf[512] = {0};
        fread(buf, 1, sizeof buf - 1, f);
        fclose(f);
        CHECK(strstr(buf, "UNIMPLEMENTED") == NULL);
    }
    _putenv("AURELIA_SHIM_LOG=");
    return 1;
}

static int t_bridge_ticket(void) {
    shim_init();
    CHECK(shim_snapshot() && shim_snapshot()->steamid == 76561198000000001ULL);
    void *client = create("SteamClient023");
    CHECK(client);
    void *u = ((Fn_p_iip)vt(client, 5))(client, 1, 1, "SteamUser022");
    CHECK(u);
    uint8_t buf[512];
    uint32_t n = 0;
    uint32_t h = ((Fn_auth3)vt(u, 13))(u, buf, sizeof buf, &n);
    CHECK(h != 0 && n == 18 && memcmp(buf, "TICKET-FROM-BRIDGE", 18) == 0);
    int cancel = -1;
    for (int k = 0; k < g_n_ifaces; k++) {
        if (strcmp(g_ifaces[k].name, "SteamUser022")) continue;
        for (int i = 0; i < g_ifaces[k].n_slots; i++)
            if (g_ifaces[k].slot_names[i] && !strcmp(g_ifaces[k].slot_names[i], "CancelAuthTicket")) cancel = i;
    }
    CHECK(cancel >= 0);
    ((Fn_u_u)vt(u, cancel))(u, h);
    Sleep(1500);
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_b_sp)(void *, const char *, void *);
typedef const char *(SHIM_TC *Fn_b_ss)(void *, const char *, const char *);
typedef uint64_t (SHIM_TC *Fn_us_stat)(void *, uint64_t, const char *, void *);
typedef uint64_t (SHIM_TC *Fn_us_avg)(void *, const char *, float, double);
typedef uint64_t (SHIM_TC *Fn_b_si)(void *, const char *, int32_t);
typedef uint64_t (SHIM_TC *Fn_b_sf)(void *, const char *, float);
typedef uint64_t (SHIM_TC *Fn_b_spp)(void *, const char *, void *, void *);
typedef uint32_t (SHIM_TC *Fn_u_v)(void *);
typedef const char *(SHIM_TC *Fn_s_u)(void *, uint32_t);

static int flush_callbacks(int id) {
    int seen = 0;
    ShimCallbackMsg m;
    while (shim_next_callback(&m)) { if (m.id == id) seen++; shim_free_last_callback(); }
    return seen;
}

static int t_user_stats(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    void *st = ((Fn_p_iip)vt(client, 13))(client, 1, 1, "STEAMUSERSTATS_INTERFACE_VERSION012");
    CHECK(st);
    flush_callbacks(0);
    CHECK((((Fn_i_v)vt(st, 0))(st) & 0xff) == 1);
    int got_stats = 0;
    for (int k = 0; k < 100 && !got_stats; k++) { got_stats += flush_callbacks(1101); if (!got_stats) Sleep(50); }
    CHECK(got_stats == 1);
    int32_t kills = 0;
    CHECK((((Fn_b_sp)vt(st, 2))(st, "kills", &kills) & 0xff) == 1 && kills == 42);
    float acc = 0;
    CHECK((((Fn_b_sp)vt(st, 1))(st, "accuracy", &acc) & 0xff) == 1 && acc == 0.5f);
    CHECK(!(((Fn_b_sp)vt(st, 1))(st, "kills", &acc) & 0xff) && acc == 0.0f);
    CHECK(!(((Fn_b_sp)vt(st, 2))(st, "nao_existe", &kills) & 0xff) && kills == 0);
    CHECK(((Fn_u_v)vt(st, 14))(st) == 2);
    {
        const char *iv12 = "STEAMUSERSTATS_INTERFACE_VERSION012";
        CHECK(!strcmp(((Fn_b_ss)vt(st, slot_of(iv12, "GetAchievementDisplayAttribute")))(st, "ACH_FIRST", "name"), "First"));
        CHECK(!strcmp(((Fn_b_ss)vt(st, slot_of(iv12, "GetAchievementDisplayAttribute")))(st, "ACH_FIRST", "desc"), "Win once"));
        CHECK(!strcmp(((Fn_b_ss)vt(st, slot_of(iv12, "GetAchievementDisplayAttribute")))(st, "ACH_FIRST", "hidden"), "0"));
        CHECK(!strcmp(((Fn_b_ss)vt(st, slot_of(iv12, "GetAchievementDisplayAttribute")))(st, "ACH_SECOND", "name"), "ACH_SECOND"));
        float pct = 0;
        CHECK((((Fn_b_sp)vt(st, slot_of(iv12, "GetAchievementAchievedPercent")))(st, "ACH_FIRST", &pct) & 0xff) == 1 && pct == 12.5f);
        int32_t other = 0;
        CHECK((((Fn_us_stat)vt(st, slot_of(iv12, "GetUserStat")))(st, 76561198000000001ULL, "kills", &other) & 0xff) == 1 && other == 42);
        CHECK((((Fn_us_stat)vt(st, slot_of(iv12, "GetUserStat")))(st, 76561198000000099ULL, "kills", &other) & 0xff) == 0);
        uint8_t got_ach = 9;
        CHECK((((Fn_us_stat)vt(st, slot_of(iv12, "GetUserAchievement")))(st, 76561198000000001ULL, "ACH_FIRST", &got_ach) & 0xff) == 1 && got_ach == 1);
        CHECK((((Fn_us_avg)vt(st, slot_of(iv12, "UpdateAvgRateStat")))(st, "rate", 10.0f, 4.0) & 0xff) == 1);
        float avg = 0;
        CHECK((((Fn_b_sp)vt(st, slot_of(iv12, "GetStat_2")))(st, "rate", &avg) & 0xff) == 1 && avg == 2.5f);
        CHECK((((Fn_us_avg)vt(st, slot_of(iv12, "UpdateAvgRateStat")))(st, "rate", 10.0f, 6.0) & 0xff) == 1);
        CHECK((((Fn_b_sp)vt(st, slot_of(iv12, "GetStat_2")))(st, "rate", &avg) & 0xff) == 1 && avg == 2.0f);
    }
    CHECK(!strcmp(((Fn_s_u)vt(st, 15))(st, 0), "ACH_FIRST"));
    uint8_t got = 0; uint32_t when = 0;
    CHECK((((Fn_b_spp)vt(st, 9))(st, "ACH_FIRST", &got, &when) & 0xff) == 1 && got == 1 && when == 1700000000u);
    CHECK((((Fn_b_sp)vt(st, 6))(st, "ACH_SECOND", &got) & 0xff) == 1 && got == 0);
    CHECK((((Fn_i_s)vt(st, 7))(st, "ACH_SECOND") & 0xff) == 1);
    CHECK((((Fn_b_si)vt(st, 3))(st, "kills", 43) & 0xff) == 1);
    CHECK((((Fn_b_sf)vt(st, 4))(st, "accuracy", 0.75f) & 0xff) == 1);
    CHECK((((Fn_i_s)vt(st, 7))(st, "NAO_EXISTE") & 0xff) == 0);
    CHECK((((Fn_i_v)vt(st, 10))(st) & 0xff) == 1);
    int stored = 0, achieved = 0;
    ShimCallbackMsg m;
    while (shim_next_callback(&m)) {
        if (m.id == 1102) stored++;
        if (m.id == 1103) { achieved++; CHECK(!strcmp((char *)m.param + 9, "ACH_SECOND")); }
        shim_free_last_callback();
    }
    CHECK(stored == 1 && achieved == 1);
    Sleep(400);
    return 1;
}

static int t_bridge_refuses(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    void *u = ((Fn_p_iip)vt(client, 5))(client, 1, 1, "SteamUser022");
    CHECK(u);
    uint8_t buf[512];
    uint32_t n = 99;
    CHECK(((Fn_auth3)vt(u, 13))(u, buf, sizeof buf, &n) == 0);
    CHECK(n == 0);
    return 1;
}

static int t_ticket_offline(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    void *u = ((Fn_p_iip)vt(client, 5))(client, 1, 1, "SteamUser022");
    CHECK(u);
    uint8_t buf[512];
    uint32_t n = 0;
    CHECK(((Fn_auth3)vt(u, 13))(u, buf, sizeof buf, &n) != 0);
    CHECK(n > 24);
    uint32_t token_len;
    memcpy(&token_len, buf, 4);
    CHECK(token_len == 20 && !memcmp(buf + 4, "\x01\x02\x03\x04\x05\x06\x07\x08\x01\x4c\x5e\x02\x01\x00\x10\x01\x11\x12\x13\x14", 20));
    return 1;
}

__attribute__((noinline)) static void selftest_crash_site(volatile int *p) { *p = 1; }
static int t_crash(void) {
    shim_init();
    selftest_crash_site(NULL);
    return 1;
}

static int t_protobuf(void) {
    PbW w = {0};
    pbw_varint(&w, 1, 150);
    CHECK(w.len == 3 && w.p[0] == 0x08 && w.p[1] == 0x96 && w.p[2] == 0x01);
    pbw_str(&w, 2, "testing");
    pbw_fixed32(&w, 3, 0x11223344);
    pbw_fixed64(&w, 4, 0x1122334455667788ULL);
    PbW inner = {0};
    pbw_varint(&inner, 1, 7);
    pbw_msg(&w, 5, &inner);
    PbR r;
    PbField f;
    pbr_init(&r, w.p, w.len);
    CHECK(pbr_next(&r, &f) && f.field == 1 && f.wt == 0 && f.v == 150);
    CHECK(pbr_next(&r, &f) && f.field == 2 && f.wt == 2 && f.len == 7 && !memcmp(f.data, "testing", 7));
    CHECK(pbr_next(&r, &f) && f.field == 3 && f.wt == 5 && f.v == 0x11223344);
    CHECK(pbr_next(&r, &f) && f.field == 4 && f.wt == 1 && f.v == 0x1122334455667788ULL);
    CHECK(pbr_next(&r, &f) && f.field == 5 && f.wt == 2 && f.len == 2);
    CHECK(!pbr_next(&r, &f) && !r.bad);
    uint8_t truncated[] = { 0x12, 0x05, 'a' };
    pbr_init(&r, truncated, sizeof truncated);
    CHECK(!pbr_next(&r, &f) && r.bad);
    pbw_free(&inner);
    pbw_free(&w);
    return 1;
}

static bool parse_echo(const CmReply *r, void *ctx, void *out, int *size) {
    (void)ctx;
    if (r->eresult != 1 || (int)r->len > *size) return false;
    memcpy(out, r->body, r->len);
    *size = (int)r->len;
    return true;
}

static struct { int id; int size; uint8_t data[64]; } g_stash[256];
static int g_n_stash;

static void stash_callback(const ShimCallbackMsg *m) {
    if (m->id == 703 || g_n_stash == 256) return;
    g_stash[g_n_stash].id = m->id;
    g_stash[g_n_stash].size = m->size < 64 ? m->size : 64;
    memcpy(g_stash[g_n_stash].data, m->param, (size_t)g_stash[g_n_stash].size);
    g_n_stash++;
}

static int g_wait_iters = 100;
static int wait_call_result(uint64_t h, bool *failed) {
    for (int i = 0; i < g_wait_iters; i++) {
        ShimCallbackMsg m;
        while (shim_next_callback(&m)) { stash_callback(&m); shim_free_last_callback(); }
        if (shim_callresult_completed(h, failed)) return 1;
        Sleep(50);
    }
    return 0;
}

static int t_cm_relay(void) {
    shim_init();
    PbW w = {0};
    pbw_varint(&w, 1, 150);
    pbw_str(&w, 2, "hi");
    CmReply r = {0};
    CHECK(shim_cm_service("Test.Echo#1", w.p, w.len, 3000, &r));
    CHECK(r.emsg == 147 && r.eresult == 1 && r.len == w.len && !memcmp(r.body, w.p, w.len));
    shim_cm_reply_free(&r);
    CHECK(shim_cm_call(6602, true, NULL, w.p, w.len, 3000, &r) && r.emsg == 6603 && r.eresult == 1);
    shim_cm_reply_free(&r);
    CHECK(!shim_cm_service("Test.Silent#1", w.p, w.len, 500, &r));
    CHECK(shim_cm_send(5432, true, w.p, w.len));
    CHECK(shim_cm_subscribe(6613));
    int32_t e;
    uint8_t *b;
    size_t l;
    CHECK(!shim_cm_poll(&e, &b, &l));
    CHECK(shim_cm_service("Test.Push#1", w.p, w.len, 3000, &r));
    shim_cm_reply_free(&r);
    CHECK(shim_cm_poll(&e, &b, &l) && e == 6613 && l == 3 && !memcmp(b, "abc", 3));
    free(b);
    CHECK(!shim_cm_poll(&e, &b, &l));
    uint64_t h = shim_callresult_reserve();
    PbW body = w;
    memset(&w, 0, sizeof w);
    shim_cm_async(h, 9999, 0, true, "Test.Echo#1", &body, 3000, parse_echo, NULL);
    bool failed = true;
    CHECK(wait_call_result(h, &failed) && !failed);
    uint8_t out[16];
    CHECK(shim_callresult_take(h, out, sizeof out, 9999, &failed) && !failed && out[0] == 0x08);
    uint64_t h2 = shim_callresult_reserve();
    PbW silent = {0};
    pbw_varint(&silent, 1, 1);
    shim_cm_async(h2, 9999, 0, true, "Test.Silent#1", &silent, 300, parse_echo, NULL);
    CHECK(wait_call_result(h2, &failed) && failed);
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_lb_find)(void *, const char *, int, int);
typedef uint64_t (SHIM_TC *Fn_lb_name)(void *, const char *);
typedef const char *(SHIM_TC *Fn_lb_str)(void *, uint64_t);
typedef int (SHIM_TC *Fn_lb_int)(void *, uint64_t);
typedef uint64_t (SHIM_TC *Fn_lb_dl)(void *, uint64_t, int, int, int);
typedef uint64_t (SHIM_TC *Fn_lb_entry)(void *, uint64_t, int, void *, int32_t *, int);
typedef uint64_t (SHIM_TC *Fn_lb_up)(void *, uint64_t, int, int32_t, const int32_t *, int);
typedef uint64_t (SHIM_TC *Fn_lb_call0)(void *);
typedef uint64_t (SHIM_TC *Fn_lb_call1)(void *, int);
typedef uint64_t (SHIM_TC *Fn_lb_gs)(void *, const char *, void *);
typedef int (SHIM_TC *Fn_lb_hist)(void *, const char *, void *, uint32_t);
typedef int (SHIM_TC *Fn_lb_most)(void *, char *, uint32_t, float *, uint8_t *);

static int call_taken(uint64_t h, int cb, void *out, int cap) {
    bool failed = true;
    if (!wait_call_result(h, &failed) || failed) return 0;
    return shim_callresult_take(h, out, cap, cb, &failed) && !failed;
}

static int t_leaderboards(void) {
    shim_init();
    const char *iv = "STEAMUSERSTATS_INTERFACE_VERSION012";
    void *client = create("SteamClient023");
    CHECK(client);
    void *st = ((Fn_p_iip)vt(client, 13))(client, 1, 1, iv);
    CHECK(st);
    CHECK((((Fn_i_v)vt(st, 0))(st) & 0xff) == 1);
    uint8_t res[64];
    uint64_t h = ((Fn_lb_find)vt(st, slot_of(iv, "FindOrCreateLeaderboard")))(st, "Fastest", 2, 1);
    CHECK(call_taken(h, 1104, res, sizeof res));
    uint64_t board;
    memcpy(&board, res, 8);
    CHECK(board == 7 && res[8] == 1);
    CHECK(!strcmp(((Fn_lb_str)vt(st, slot_of(iv, "GetLeaderboardName")))(st, board), "Fastest"));
    CHECK(((Fn_lb_int)vt(st, slot_of(iv, "GetLeaderboardEntryCount")))(st, board) == 5);
    CHECK(((Fn_lb_int)vt(st, slot_of(iv, "GetLeaderboardSortMethod")))(st, board) == 2);
    CHECK(((Fn_lb_int)vt(st, slot_of(iv, "GetLeaderboardDisplayType")))(st, board) == 1);
    h = ((Fn_lb_name)vt(st, slot_of(iv, "FindLeaderboard")))(st, "Nope");
    CHECK(call_taken(h, 1104, res, sizeof res) && res[8] == 0);
    h = ((Fn_lb_dl)vt(st, slot_of(iv, "DownloadLeaderboardEntries")))(st, board, 0, 1, 3);
    CHECK(call_taken(h, 1105, res, sizeof res));
    uint64_t set;
    int32_t count;
    memcpy(&set, res + 8, 8);
    memcpy(&count, res + 16, 4);
    CHECK(count == 2);
    uint8_t entry[32];
    int32_t details[4] = {0};
    CHECK(((Fn_lb_entry)vt(st, slot_of(iv, "GetDownloadedLeaderboardEntry")))(st, set, 0, entry, details, 4) & 0xff);
    uint64_t who, ugc;
    int32_t rank, score, nd;
    memcpy(&who, entry, 8); memcpy(&rank, entry + 8, 4); memcpy(&score, entry + 12, 4); memcpy(&nd, entry + 16, 4); memcpy(&ugc, entry + 20, 8);
    CHECK(who == 76561198000000042ULL && rank == 1 && score == 100 && nd == 2 && details[0] == 7 && details[1] == 8 && ugc == ~0ULL);
    CHECK(!(((Fn_lb_entry)vt(st, slot_of(iv, "GetDownloadedLeaderboardEntry")))(st, set, 5, entry, details, 4) & 0xff));
    int32_t up[3] = {1, 2, 3};
    h = ((Fn_lb_up)vt(st, slot_of(iv, "UploadLeaderboardScore")))(st, board, 1, 150, up, 3);
    CHECK(call_taken(h, 1106, res, sizeof res));
    int32_t up_score, rank_new, rank_prev;
    memcpy(&up_score, res + 16, 4); memcpy(&rank_new, res + 24, 4); memcpy(&rank_prev, res + 28, 4);
    CHECK(res[0] == 1 && up_score == 150 && res[20] == 1 && rank_new == 2 && rank_prev == 5);
    h = ((Fn_lb_call0)vt(st, slot_of(iv, "GetNumberOfCurrentPlayers")))(st);
    CHECK(call_taken(h, 1107, res, sizeof res));
    int32_t players;
    memcpy(&players, res + 4, 4);
    CHECK(res[0] == 1 && players == 777);
    h = ((Fn_lb_call1)vt(st, slot_of(iv, "RequestGlobalStats")))(st, 3);
    CHECK(call_taken(h, 1112, res, sizeof res));
    int32_t gres;
    memcpy(&gres, res + 8, 4);
    CHECK(gres == 1);
    int64_t total = 0;
    CHECK(((Fn_lb_gs)vt(st, slot_of(iv, "GetGlobalStat")))(st, "kills", &total) & 0xff);
    CHECK(total == 1000);
    double acc = 0;
    CHECK(((Fn_lb_gs)vt(st, slot_of(iv, "GetGlobalStat_2")))(st, "accuracy", &acc) & 0xff);
    CHECK(acc == 0.25);
    CHECK(!(((Fn_lb_gs)vt(st, slot_of(iv, "GetGlobalStat")))(st, "unknown", &total) & 0xff));
    int64_t hist[8] = {0};
    CHECK(((Fn_lb_hist)vt(st, slot_of(iv, "GetGlobalStatHistory")))(st, "kills", hist, sizeof hist) == 3);
    CHECK(hist[0] == 30 && hist[1] == 20 && hist[2] == 10);
    h = ((Fn_lb_call0)vt(st, slot_of(iv, "RequestGlobalAchievementPercentages")))(st);
    CHECK(call_taken(h, 1110, res, sizeof res));
    char nm[64];
    float pct = 0;
    uint8_t got = 9;
    CHECK(((Fn_lb_most)vt(st, slot_of(iv, "GetMostAchievedAchievementInfo")))(st, nm, sizeof nm, &pct, &got) == 0);
    CHECK(!strcmp(nm, "ACH_FIRST") && pct == 12.5f && got == 1);
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_enc_req)(void *, const void *, int);
typedef uint64_t (SHIM_TC *Fn_enc_get)(void *, void *, int, uint32_t *);
typedef uint64_t (SHIM_TC *Fn_store_url)(void *, const char *);
static int t_storeurl(void) {
    shim_init();
    const char *iv = "SteamUser023";
    void *client = create("SteamClient023");
    void *u = ((Fn_p_iip)vt(client, 5))(client, 1, 1, iv);
    CHECK(u);
    uint64_t h = ((Fn_store_url)vt(u, slot_of(iv, "RequestStoreAuthURL")))(u, "https://store.steampowered.com/");
    uint8_t res[512];
    CHECK(call_taken(h, 165, res, sizeof res));
    CHECK(!strncmp((char *)res, "https://store.steampowered.com/itemshop/setoverlay?cookie=", 58));
    h = ((Fn_store_url)vt(u, slot_of(iv, "RequestStoreAuthURL")))(u, "");
    CHECK(!call_taken(h, 165, res, sizeof res));
    return 1;
}

static int t_appticket(void) {
    shim_init();
    const char *iv = "SteamUser023";
    void *client = create("SteamClient023");
    CHECK(client);
    void *u = ((Fn_p_iip)vt(client, 5))(client, 1, 1, iv);
    CHECK(u);
    uint32_t n = 0;
    uint8_t buf[64];
    CHECK(!(((Fn_enc_get)vt(u, slot_of(iv, "GetEncryptedAppTicket")))(u, buf, sizeof buf, &n) & 0xff));
    uint64_t h = ((Fn_enc_req)vt(u, slot_of(iv, "RequestEncryptedAppTicket")))(u, "hello", 5);
    uint8_t res[8];
    CHECK(call_taken(h, 154, res, sizeof res));
    int32_t eresult;
    memcpy(&eresult, res, 4);
    CHECK(eresult == 1);
    uint8_t tiny[4];
    CHECK(!(((Fn_enc_get)vt(u, slot_of(iv, "GetEncryptedAppTicket")))(u, tiny, sizeof tiny, &n) & 0xff) && n == 6);
    CHECK((((Fn_enc_get)vt(u, slot_of(iv, "GetEncryptedAppTicket")))(u, buf, sizeof buf, &n) & 0xff) && n == 6 && !memcmp(buf, "TICKET", 6));
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_lob_create)(void *, int, int);
typedef uint64_t (SHIM_TC *Fn_lob_id)(void *, uint64_t);
typedef uint64_t (SHIM_TC *Fn_lob_skv)(void *, uint64_t, const char *, const char *);
typedef void (SHIM_TC *Fn_lob_member_set)(void *, uint64_t, const char *, const char *);
typedef const char *(SHIM_TC *Fn_lob_get)(void *, uint64_t, const char *);
typedef const char *(SHIM_TC *Fn_lob_mget)(void *, uint64_t, uint64_t, const char *);
typedef int (SHIM_TC *Fn_lob_count)(void *, uint64_t);
typedef uint64_t *(SHIM_TC *Fn_lob_owner)(void *, uint64_t *, uint64_t);
typedef uint64_t *(SHIM_TC *Fn_lob_midx)(void *, uint64_t *, uint64_t, int);
typedef uint64_t *(SHIM_TC *Fn_lob_idx)(void *, uint64_t *, int);
typedef uint64_t (SHIM_TC *Fn_lob_chat)(void *, uint64_t, const void *, int);
typedef int (SHIM_TC *Fn_lob_chat_get)(void *, uint64_t, int, uint64_t *, void *, int, int *);
typedef uint64_t (SHIM_TC *Fn_lob_list)(void *);
typedef void (SHIM_TC *Fn_lob_sfilter)(void *, const char *, const char *, int);
typedef void (SHIM_TC *Fn_lob_nfilter)(void *, const char *, int, int);
typedef void (SHIM_TC *Fn_lob_ifilter)(void *, int);
typedef void (SHIM_TC *Fn_lob_near)(void *, const char *, int);
typedef void (SHIM_TC *Fn_lob_leave)(void *, uint64_t);

static int wait_callback(int id, uint8_t *data, int cap) {
    for (int k = 0; k < g_n_stash; k++) {
        if (g_stash[k].id != id) continue;
        int n = g_stash[k].size < cap ? g_stash[k].size : cap;
        if (data) memcpy(data, g_stash[k].data, (size_t)n);
        memmove(&g_stash[k], &g_stash[k + 1], (size_t)(--g_n_stash - k) * sizeof g_stash[0]);
        return 1;
    }
    for (int i = 0; i < 80; i++) {
        ShimCallbackMsg m;
        while (shim_next_callback(&m)) {
            if (m.id == id) {
                int n = m.size < cap ? m.size : cap;
                if (data) memcpy(data, m.param, (size_t)n);
                shim_free_last_callback();
                return 1;
            }
            stash_callback(&m);
            shim_free_last_callback();
        }
        Sleep(50);
    }
    return 0;
}

static int t_lobbies_net(void) {
    shim_init();
    const char *iv = "SteamMatchMaking009";
    uint64_t me = 76561198000000001ULL;
    void *client = create("SteamClient023");
    CHECK(client);
    void *mm = ((Fn_p_iip)vt(client, 10))(client, 1, 1, iv);
    CHECK(mm);
    uint8_t cb[64];
    uint64_t h = ((Fn_lob_create)vt(mm, slot_of(iv, "CreateLobby")))(mm, 2, 4);
    CHECK(call_taken(h, 513, cb, sizeof cb));
    int32_t eresult;
    uint64_t lobby;
    memcpy(&eresult, cb, 4);
    memcpy(&lobby, cb + 8, 8);
    CHECK(eresult == 1 && lobby == 0x1860000000001234ULL);
    CHECK(wait_callback(504, cb, sizeof cb));
    uint64_t owner;
    ((Fn_lob_owner)vt(mm, slot_of(iv, "GetLobbyOwner")))(mm, &owner, lobby);
    CHECK(owner == me && ((Fn_lob_count)vt(mm, slot_of(iv, "GetNumLobbyMembers")))(mm, lobby) == 1);
    CHECK(((Fn_lob_skv)vt(mm, slot_of(iv, "SetLobbyData")))(mm, lobby, "map", "dust") & 0xff);
    CHECK(wait_callback(505, cb, sizeof cb));
    CHECK(!strcmp(((Fn_lob_get)vt(mm, slot_of(iv, "GetLobbyData")))(mm, lobby, "MAP"), "dust"));
    CHECK(((Fn_lob_count)vt(mm, slot_of(iv, "GetLobbyDataCount")))(mm, lobby) == 1);
    ((Fn_lob_member_set)vt(mm, slot_of(iv, "SetLobbyMemberData")))(mm, lobby, "ready", "1");
    CHECK(wait_callback(505, cb, sizeof cb));
    CHECK(!strcmp(((Fn_lob_mget)vt(mm, slot_of(iv, "GetLobbyMemberData")))(mm, lobby, me, "ready"), "1"));
    CHECK(((Fn_lob_chat)vt(mm, slot_of(iv, "SendLobbyChatMsg")))(mm, lobby, "hello", 5) & 0xff);
    CHECK(wait_callback(507, cb, sizeof cb));
    uint8_t text[16] = {0};
    uint64_t speaker = 0;
    int ctype = 0;
    uint32_t chat_id;
    memcpy(&chat_id, cb + 20, 4);
    CHECK(((Fn_lob_chat_get)vt(mm, slot_of(iv, "GetLobbyChatEntry")))(mm, lobby, (int)chat_id, &speaker, text, sizeof text, &ctype) == 5);
    CHECK(!memcmp(text, "hello", 5) && speaker == me && ctype == 1);
    CHECK(wait_callback(506, cb, sizeof cb));
    uint64_t other_user;
    uint32_t state;
    memcpy(&other_user, cb + 8, 8);
    memcpy(&state, cb + 24, 4);
    CHECK(other_user == 76561198000000042ULL && state == 1);
    CHECK(((Fn_lob_count)vt(mm, slot_of(iv, "GetNumLobbyMembers")))(mm, lobby) == 2);
    uint64_t second;
    ((Fn_lob_midx)vt(mm, slot_of(iv, "GetLobbyMemberByIndex")))(mm, &second, lobby, 1);
    CHECK(second == other_user);
    CHECK(Steam_BConnected(g_user, g_pipe) == 1 && Steam_BLoggedOn(g_user, g_pipe) == 1);
    ((Fn_lob_sfilter)vt(mm, slot_of(iv, "AddRequestLobbyListStringFilter")))(mm, "map", "dust", 0);
    ((Fn_lob_nfilter)vt(mm, slot_of(iv, "AddRequestLobbyListNumericalFilter")))(mm, "lv", 5, 2);
    ((Fn_lob_ifilter)vt(mm, slot_of(iv, "AddRequestLobbyListFilterSlotsAvailable")))(mm, 2);
    ((Fn_lob_near)vt(mm, slot_of(iv, "AddRequestLobbyListNearValueFilter")))(mm, "skill", 10);
    ((Fn_lob_ifilter)vt(mm, slot_of(iv, "AddRequestLobbyListDistanceFilter")))(mm, 3);
    h = ((Fn_lob_list)vt(mm, slot_of(iv, "RequestLobbyList")))(mm);
    CHECK(call_taken(h, 510, cb, sizeof cb));
    uint32_t found;
    memcpy(&found, cb, 4);
    CHECK(found == 1);
    uint64_t listed;
    ((Fn_lob_idx)vt(mm, slot_of(iv, "GetLobbyByIndex")))(mm, &listed, 0);
    CHECK(listed == 0x1860000000005678ULL);
    CHECK(!strcmp(((Fn_lob_get)vt(mm, slot_of(iv, "GetLobbyData")))(mm, listed, "name"), "Beto's Room"));
    h = ((Fn_lob_id)vt(mm, slot_of(iv, "JoinLobby")))(mm, listed);
    CHECK(call_taken(h, 504, cb, sizeof cb));
    int32_t response;
    memcpy(&response, cb + 16, 4);
    CHECK(response == 1 && ((Fn_lob_count)vt(mm, slot_of(iv, "GetNumLobbyMembers")))(mm, listed) == 2);
    ((Fn_lob_owner)vt(mm, slot_of(iv, "GetLobbyOwner")))(mm, &owner, listed);
    CHECK(owner == 76561198000000043ULL);
    h = ((Fn_lob_id)vt(mm, slot_of(iv, "JoinLobby")))(mm, 0x1860000000009999ULL);
    CHECK(call_taken(h, 504, cb, sizeof cb));
    memcpy(&response, cb + 16, 4);
    CHECK(response == 2);
    CHECK(wait_callback(503, cb, sizeof cb));
    uint64_t inviter, invited_lobby;
    memcpy(&inviter, cb, 8);
    memcpy(&invited_lobby, cb + 8, 8);
    CHECK(inviter == 76561198000000043ULL && invited_lobby == 0x0186000000007777ULL);
    CHECK(wait_callback(512, cb, sizeof cb));
    uint64_t kicked_from;
    memcpy(&kicked_from, cb, 8);
    CHECK(kicked_from == listed);
    CHECK(((Fn_lob_count)vt(mm, slot_of(iv, "GetNumLobbyMembers")))(mm, listed) == 0);
    ((Fn_lob_leave)vt(mm, slot_of(iv, "LeaveLobby")))(mm, lobby);
    CHECK(!strcmp(((Fn_lob_get)vt(mm, slot_of(iv, "GetLobbyData")))(mm, lobby, "map"), ""));
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_inv_all)(void *, uint32_t *);
typedef uint64_t (SHIM_TC *Fn_inv_ids)(void *, uint32_t *, const uint64_t *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_inv_consume)(void *, uint32_t *, uint64_t, uint32_t);
typedef uint64_t (SHIM_TC *Fn_inv_promo)(void *, uint32_t *, int32_t);
typedef int (SHIM_TC *Fn_inv_status)(void *, int32_t);
typedef uint64_t (SHIM_TC *Fn_inv_items)(void *, int32_t, void *, uint32_t *);
typedef uint64_t (SHIM_TC *Fn_inv_prop)(void *, int32_t, uint32_t, const char *, char *, uint32_t *);
typedef uint64_t (SHIM_TC *Fn_inv_defprop)(void *, int32_t, const char *, char *, uint32_t *);
typedef uint64_t (SHIM_TC *Fn_inv_defids)(void *, int32_t *, uint32_t *);
typedef uint64_t (SHIM_TC *Fn_inv_elig_req)(void *, uint64_t);
typedef uint64_t (SHIM_TC *Fn_inv_elig)(void *, uint64_t, int32_t *, uint32_t *);
typedef void (SHIM_TC *Fn_inv_destroy)(void *, int32_t);
typedef uint64_t (SHIM_TC *Fn_inv_prices)(void *);
typedef uint32_t (SHIM_TC *Fn_inv_nprices)(void *);
typedef uint64_t (SHIM_TC *Fn_inv_price)(void *, int32_t, uint64_t *, uint64_t *);
typedef uint64_t (SHIM_TC *Fn_inv_buy)(void *, const int32_t *, const uint32_t *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_inv_load)(void *);
typedef uint64_t (SHIM_TC *Fn_inv_prices_list)(void *, int32_t *, uint64_t *, uint64_t *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_inv_start)(void *);
typedef uint64_t (SHIM_TC *Fn_inv_setstr)(void *, uint64_t, uint64_t, const char *, const char *);
typedef uint64_t (SHIM_TC *Fn_inv_setint)(void *, uint64_t, uint64_t, const char *, int64_t);
typedef uint64_t (SHIM_TC *Fn_inv_rm)(void *, uint64_t, uint64_t, const char *);
typedef uint64_t (SHIM_TC *Fn_inv_submit)(void *, uint64_t, uint32_t *);
typedef uint64_t (SHIM_TC *Fn_inv_drop)(void *, uint32_t *, int32_t);
typedef uint64_t (SHIM_TC *Fn_inv_xfer)(void *, uint32_t *, uint64_t, uint32_t, uint64_t);
typedef uint64_t (SHIM_TC *Fn_inv_inspect)(void *, uint32_t *, const char *);

static int inv_wait(int32_t h) {
    uint8_t cb[16];
    if (!wait_callback(4700, cb, sizeof cb)) return 0;
    int32_t got, status;
    memcpy(&got, cb, 4);
    memcpy(&status, cb + 4, 4);
    return got == h && status == 1;
}

static int t_inventory(void) {
    shim_init();
    const char *iv = "STEAMINVENTORY_INTERFACE_V003";
    void *client = create("SteamClient023");
    CHECK(client);
    void *inv = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamInventory")))(client, 1, 1, iv);
    CHECK(inv);
    uint32_t h = 0;
    CHECK(((Fn_inv_all)vt(inv, slot_of(iv, "GetAllItems")))(inv, &h) & 0xff);
    CHECK(h != 0 && inv_wait((int32_t)h));
    CHECK(((Fn_inv_status)vt(inv, slot_of(iv, "GetResultStatus")))(inv, (int32_t)h) == 1);
    uint32_t n = 0;
    CHECK(((Fn_inv_items)vt(inv, slot_of(iv, "GetResultItems")))(inv, (int32_t)h, NULL, &n) & 0xff);
    CHECK(n == 2);
    uint8_t items[32];
    CHECK(((Fn_inv_items)vt(inv, slot_of(iv, "GetResultItems")))(inv, (int32_t)h, items, &n) & 0xff);
    uint64_t id0, id1;
    int32_t def0, def1;
    uint16_t q0, f1;
    memcpy(&id0, items, 8); memcpy(&def0, items + 8, 4); memcpy(&q0, items + 12, 2);
    memcpy(&id1, items + 16, 8); memcpy(&def1, items + 24, 4); memcpy(&f1, items + 30, 2);
    CHECK(id0 == 111 && def0 == 100 && q0 == 2 && id1 == 222 && def1 == 200 && f1 == 1);
    char text[64];
    uint32_t size = sizeof text;
    CHECK(((Fn_inv_prop)vt(inv, slot_of(iv, "GetResultItemProperty")))(inv, (int32_t)h, 0, "name", text, &size) & 0xff);
    CHECK(!strcmp(text, "Sword") && size == 6);
    uint32_t tiny = 2;
    char two[2];
    CHECK(!(((Fn_inv_prop)vt(inv, slot_of(iv, "GetResultItemProperty")))(inv, (int32_t)h, 0, "name", two, &tiny) & 0xff) && tiny == 6);
    uint32_t h2 = 0;
    CHECK(((Fn_inv_consume)vt(inv, slot_of(iv, "ConsumeItem")))(inv, &h2, 111, 1) & 0xff);
    CHECK(inv_wait((int32_t)h2));
    uint64_t want[1] = { 222 };
    uint32_t h3 = 0;
    CHECK(((Fn_inv_ids)vt(inv, slot_of(iv, "GetItemsByID")))(inv, &h3, want, 1) & 0xff);
    CHECK(inv_wait((int32_t)h3));
    n = 0;
    CHECK(((Fn_inv_items)vt(inv, slot_of(iv, "GetResultItems")))(inv, (int32_t)h3, NULL, &n) & 0xff && n == 1);
    uint32_t h4 = 0;
    CHECK(((Fn_inv_promo)vt(inv, slot_of(iv, "AddPromoItem")))(inv, &h4, 300) & 0xff);
    CHECK(inv_wait((int32_t)h4));
    {
        uint64_t uh = ((Fn_inv_start)vt(inv, slot_of(iv, "StartUpdateProperties")))(inv);
        CHECK(uh != 0);
        CHECK(((Fn_inv_setstr)vt(inv, slot_of(iv, "SetProperty")))(inv, uh, 111, "color", "blue") & 0xff);
        CHECK(((Fn_inv_setint)vt(inv, slot_of(iv, "SetProperty_3")))(inv, uh, 111, "level", 7) & 0xff);
        CHECK(((Fn_inv_rm)vt(inv, slot_of(iv, "RemoveProperty")))(inv, uh, 222, "old") & 0xff);
        uint32_t hm = 0;
        CHECK(((Fn_inv_submit)vt(inv, slot_of(iv, "SubmitUpdateProperties")))(inv, uh, &hm) & 0xff);
        CHECK(inv_wait((int32_t)hm));
        uint32_t hd = 0;
        CHECK(((Fn_inv_drop)vt(inv, slot_of(iv, "TriggerItemDrop")))(inv, &hd, 55) & 0xff);
        CHECK(inv_wait((int32_t)hd));
        uint32_t hs = 0, hc = 0, hi = 0;
        CHECK(((Fn_inv_xfer)vt(inv, slot_of(iv, "TransferItemQuantity")))(inv, &hs, 111, 1, ~0ULL) & 0xff);
        CHECK(inv_wait((int32_t)hs));
        CHECK(((Fn_inv_xfer)vt(inv, slot_of(iv, "TransferItemQuantity")))(inv, &hc, 111, 1, 222) & 0xff);
        CHECK(inv_wait((int32_t)hc));
        CHECK(((Fn_inv_inspect)vt(inv, slot_of(iv, "InspectItem")))(inv, &hi, "token") & 0xff);
        CHECK(inv_wait((int32_t)hi));
    }
    {
        uint64_t pc = ((Fn_inv_prices)vt(inv, slot_of(iv, "RequestPrices")))(inv);
        uint8_t pr[16];
        CHECK(call_taken(pc, 4705, pr, sizeof pr));
        int32_t pres;
        memcpy(&pres, pr, 4);
        CHECK(pres == 1 && !strcmp((char *)pr + 4, "BRL"));
        CHECK(((Fn_inv_nprices)vt(inv, slot_of(iv, "GetNumItemsWithPrices")))(inv) == 1);
        uint64_t price = 0, base = 0;
        CHECK(((Fn_inv_price)vt(inv, slot_of(iv, "GetItemPrice")))(inv, 100, &price, &base) & 0xff);
        CHECK(price == 4600 && base == 4600);
        CHECK(!(((Fn_inv_price)vt(inv, slot_of(iv, "GetItemPrice")))(inv, 200, &price, &base) & 0xff));
        int32_t pdefs[1] = { 100 };
        uint32_t pq[1] = { 2 };
        uint64_t sc = ((Fn_inv_buy)vt(inv, slot_of(iv, "StartPurchase")))(inv, pdefs, pq, 1);
        uint8_t sr[24];
        CHECK(call_taken(sc, 4704, sr, sizeof sr));
        int32_t sres;
        uint64_t order, trans;
        memcpy(&sres, sr, 4); memcpy(&order, sr + 8, 8); memcpy(&trans, sr + 16, 8);
        CHECK(sres == 1 && order == 555 && trans == 777);
        uint8_t rr[16];
        CHECK(wait_callback(4700, rr, sizeof rr));
    }
    ((Fn_inv_destroy)vt(inv, slot_of(iv, "DestroyResult")))(inv, (int32_t)h);
    CHECK(((Fn_inv_status)vt(inv, slot_of(iv, "GetResultStatus")))(inv, (int32_t)h) == 2);
    uint64_t call = ((Fn_inv_elig_req)vt(inv, slot_of(iv, "RequestEligiblePromoItemDefinitionsIDs")))(inv, 76561198000000001ULL);
    uint8_t res[32];
    CHECK(call_taken(call, 4703, res, sizeof res));
    int32_t eligible;
    memcpy(&eligible, res + 16, 4);
    CHECK(eligible == 2);
    int32_t ids[4];
    uint32_t cnt = 4;
    CHECK(((Fn_inv_elig)vt(inv, slot_of(iv, "GetEligiblePromoItemDefinitionIDs")))(inv, 76561198000000001ULL, ids, &cnt) & 0xff);
    CHECK(cnt == 2 && ids[0] == 7 && ids[1] == 9);
    cnt = 4;
    CHECK(((Fn_inv_defids)vt(inv, slot_of(iv, "GetItemDefinitionIDs")))(inv, ids, &cnt) & 0xff);
    CHECK(cnt == 2 && ids[0] == 100 && ids[1] == 200);
    size = sizeof text;
    CHECK(((Fn_inv_defprop)vt(inv, slot_of(iv, "GetItemDefinitionProperty")))(inv, 200, "name", text, &size) & 0xff);
    CHECK(!strcmp(text, "Shield"));
    return 1;
}

typedef uint32_t (SHIM_TC *Fn_ugc_u32)(void *);
typedef uint32_t (SHIM_TC *Fn_ugc_items)(void *, uint64_t *, uint32_t);
typedef uint32_t (SHIM_TC *Fn_ugc_state)(void *, uint64_t);
typedef uint64_t (SHIM_TC *Fn_ugc_info)(void *, uint64_t, uint64_t *, char *, uint32_t, uint32_t *);
typedef uint64_t (SHIM_TC *Fn_ugc_dl)(void *, uint64_t, uint8_t);
typedef uint64_t (SHIM_TC *Fn_ugc_call)(void *, uint64_t);

typedef uint64_t (SHIM_TC *Fn_ugc_u32_call)(void *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_ugc_play)(void *, const uint64_t *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_ugc_call0)(void *);

static int t_ugcwrite(void) {
    shim_init();
    const char *iv = "STEAMUGC_INTERFACE_VERSION018";
    void *client = create("SteamClient023");
    CHECK(client);
    void *ugc = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamUGC")))(client, 1, 1, iv);
    CHECK(ugc);
    uint8_t cb[16];
    int32_t result;
    uint64_t items[2] = { 111, 222 };
    uint64_t call = ((Fn_ugc_play)vt(ugc, slot_of(iv, "StartPlaytimeTracking")))(ugc, items, 2);
    CHECK(call != 0 && call_taken(call, 3410, cb, sizeof cb));
    memcpy(&result, cb, 4);
    CHECK(result == 1);
    call = ((Fn_ugc_play)vt(ugc, slot_of(iv, "StopPlaytimeTracking")))(ugc, items, 1);
    CHECK(call != 0 && call_taken(call, 3411, cb, sizeof cb));
    call = ((Fn_ugc_call0)vt(ugc, slot_of(iv, "StopPlaytimeTrackingForAllItems")))(ugc);
    CHECK(call != 0 && call_taken(call, 3411, cb, sizeof cb));
    CHECK(((Fn_ugc_call0)vt(ugc, slot_of(iv, "StopPlaytimeTrackingForAllItems")))(ugc) == 0);
    CHECK(((Fn_ugc_play)vt(ugc, slot_of(iv, "StartPlaytimeTracking")))(ugc, items, 0) == 0);
    CHECK(((Fn_ugc_play)vt(ugc, slot_of(iv, "StartPlaytimeTracking")))(ugc, NULL, 2) == 0);
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_ugc_create)(void *, uint32_t, int);
typedef uint64_t (SHIM_TC *Fn_ugc_start)(void *, uint32_t, uint64_t);
typedef uint64_t (SHIM_TC *Fn_ugc_set_s)(void *, uint64_t, const char *);
typedef Fn_ugc_set_s Fn_set_s_fn;
typedef uint64_t (SHIM_TC *Fn_set_i_fn)(void *, uint64_t, int);
typedef uint64_t (SHIM_TC *Fn_ugc_set_i)(void *, uint64_t, int);
typedef uint64_t (SHIM_TC *Fn_ugc_set_kv)(void *, uint64_t, const char *, const char *);
typedef struct { const char **strings; int32_t n; } ParamStrings;
typedef uint64_t (SHIM_TC *Fn_ugc_set_tags)(void *, uint64_t, const ParamStrings *, uint8_t);
typedef int (SHIM_TC *Fn_ugc_progress)(void *, uint64_t, uint64_t *, uint64_t *);

static int t_ugcpublish(void) {
    shim_init();
    const char *iv = "STEAMUGC_INTERFACE_VERSION018";
    void *client = create("SteamClient023");
    CHECK(client);
    void *ugc = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamUGC")))(client, 1, 1, iv);
    CHECK(ugc);
    uint8_t res[24];
    uint64_t call = ((Fn_ugc_create)vt(ugc, slot_of(iv, "CreateItem")))(ugc, 480, 0);
    CHECK(call != 0 && call_taken(call, 3403, res, 16));
    uint64_t item;
    int32_t result;
    memcpy(&item, res, 8);
    memcpy(&result, res + 8, 4);
    CHECK(result == 1 && item == 555);
    uint64_t h = ((Fn_ugc_start)vt(ugc, slot_of(iv, "StartItemUpdate")))(ugc, 480, item);
    CHECK(h != 0 && h != ~0ULL);
    CHECK(((Fn_ugc_set_s)vt(ugc, slot_of(iv, "SetItemTitle")))(ugc, h, "My mod") & 0xff);
    CHECK(((Fn_ugc_set_s)vt(ugc, slot_of(iv, "SetItemDescription")))(ugc, h, "Descri\"ption") & 0xff);
    CHECK(((Fn_ugc_set_i)vt(ugc, slot_of(iv, "SetItemVisibility")))(ugc, h, 2) & 0xff);
    const char *tags[2] = { "map", "coop" };
    ParamStrings arr = { tags, 2 };
    CHECK(((Fn_ugc_set_tags)vt(ugc, slot_of(iv, "SetItemTags")))(ugc, h, &arr, 0) & 0xff);
    CHECK(((Fn_ugc_set_kv)vt(ugc, slot_of(iv, "AddItemKeyValueTag")))(ugc, h, "mode", "cooperative") & 0xff);
    CHECK(((Fn_ugc_set_s)vt(ugc, slot_of(iv, "SetItemContent")))(ugc, h, "Z:\\tmp\\content") & 0xff);
    {
        typedef uint64_t (SHIM_TC *Fn_prev_file)(void *, uint64_t, const char *, int);
        typedef uint64_t (SHIM_TC *Fn_prev_idx)(void *, uint64_t, uint32_t, const char *);
        typedef uint64_t (SHIM_TC *Fn_idx)(void *, uint64_t, uint32_t);
        typedef uint64_t (SHIM_TC *Fn_ver)(void *, uint64_t, const char *, const char *);
        CHECK(((Fn_prev_file)vt(ugc, slot_of(iv, "AddItemPreviewFile")))(ugc, h, "Z:\\tmp\\a.png", 0) & 0xff);
        CHECK(((Fn_set_s_fn)vt(ugc, slot_of(iv, "AddItemPreviewVideo")))(ugc, h, "dQw4w9WgXcQ") & 0xff);
        CHECK(((Fn_prev_idx)vt(ugc, slot_of(iv, "UpdateItemPreviewFile")))(ugc, h, 2, "Z:\\tmp\\b.png") & 0xff);
        CHECK(((Fn_idx)vt(ugc, slot_of(iv, "RemoveItemPreview")))(ugc, h, 4) & 0xff);
        CHECK(((Fn_set_i_fn)vt(ugc, slot_of(iv, "AddContentDescriptor")))(ugc, h, 5) & 0xff);
        CHECK(((Fn_set_i_fn)vt(ugc, slot_of(iv, "RemoveContentDescriptor")))(ugc, h, 3) & 0xff);
        const char *iv21 = "STEAMUGC_INTERFACE_VERSION021";
        void *ugc21 = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamUGC")))(client, 1, 1, iv21);
        CHECK(ugc21);
        CHECK(((Fn_ver)vt(ugc21, slot_of(iv21, "SetRequiredGameVersions")))(ugc21, h, "1.0", "2.0") & 0xff);
    }
    CHECK(!(((Fn_ugc_set_s)vt(ugc, slot_of(iv, "SetItemTitle")))(ugc, h + 999, "x") & 0xff));
    uint64_t sub = ((Fn_ugc_set_s)vt(ugc, slot_of(iv, "SubmitItemUpdate")))(ugc, h, "first version");
    CHECK(sub != 0);
    uint64_t processed = 0, total = 0;
    int status = ((Fn_ugc_progress)vt(ugc, slot_of(iv, "GetItemUpdateProgress")))(ugc, h, &processed, &total);
    CHECK(status >= 1 && status <= 5);
    CHECK(call_taken(sub, 3404, res, 16));
    memcpy(&result, res, 4);
    memcpy(&item, res + 8, 8);
    CHECK(result == 1 && item == 555 && res[4] == 0);
    CHECK(((Fn_ugc_progress)vt(ugc, slot_of(iv, "GetItemUpdateProgress")))(ugc, h, &processed, &total) == 0);
    call = ((Fn_ugc_call)vt(ugc, slot_of(iv, "DeleteItem")))(ugc, 555);
    CHECK(call != 0 && call_taken(call, 3417, res, 16));
    memcpy(&result, res, 4);
    CHECK(result == 1);
    {
        typedef uint64_t (SHIM_TC *Fn_init_gs)(void *, uint32_t, const char *);
        CHECK(((Fn_init_gs)vt(ugc, slot_of(iv, "BInitWorkshopForGameServer")))(ugc, 480, "Z:\\srv\\workshop") & 0xff);
        CHECK(((Fn_init_gs)vt(ugc, slot_of(iv, "BInitWorkshopForGameServer")))(ugc, 0, NULL) & 0xff);
    }
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_rs_publish)(void *, const char *, const char *, uint32_t, const char *, const char *, int, const ParamStrings *, int);
typedef uint64_t (SHIM_TC *Fn_rs_create)(void *, uint64_t);
typedef uint64_t (SHIM_TC *Fn_rs_set_s)(void *, uint64_t, const char *);
typedef uint64_t (SHIM_TC *Fn_rs_set_i)(void *, uint64_t, int);
typedef uint64_t (SHIM_TC *Fn_rs_set_tags)(void *, uint64_t, const ParamStrings *);
typedef uint64_t (SHIM_TC *Fn_rs_call)(void *, uint64_t);

static int t_legacy_publish(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    const char *rv = "STEAMREMOTESTORAGE_INTERFACE_VERSION016";
    void *rs = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamRemoteStorage")))(client, 1, 1, rv);
    CHECK(rs);
    const char *tags[2] = { "map", "coop" };
    ParamStrings arr = { tags, 2 };
    uint64_t call = ((Fn_rs_publish)vt(rs, slot_of(rv, "PublishWorkshopFile")))(rs, "mod.zip", "cover.png", 480, "My mod", "Description", 2, &arr, 0);
    uint8_t res[24];
    CHECK(call != 0 && call_taken(call, 1309, res, sizeof res));
    int32_t result;
    uint64_t item;
    memcpy(&result, res, 4);
    memcpy(&item, res + 8, 8);
    CHECK(result == 1 && item == 777 && res[16] == 0);
    CHECK(((Fn_rs_publish)vt(rs, slot_of(rv, "PublishWorkshopFile")))(rs, NULL, NULL, 480, "x", "y", 2, NULL, 0) == 0);
    uint64_t h = ((Fn_rs_create)vt(rs, slot_of(rv, "CreatePublishedFileUpdateRequest")))(rs, 777);
    CHECK(h != 0 && h != ~0ULL);
    CHECK(((Fn_rs_set_s)vt(rs, slot_of(rv, "UpdatePublishedFileTitle")))(rs, h, "New title") & 0xff);
    CHECK(((Fn_rs_set_s)vt(rs, slot_of(rv, "UpdatePublishedFileFile")))(rs, h, "mod2.zip") & 0xff);
    CHECK(((Fn_rs_set_i)vt(rs, slot_of(rv, "UpdatePublishedFileVisibility")))(rs, h, 0) & 0xff);
    CHECK(((Fn_rs_set_tags)vt(rs, slot_of(rv, "UpdatePublishedFileTags")))(rs, h, &arr) & 0xff);
    CHECK(((Fn_rs_set_s)vt(rs, slot_of(rv, "UpdatePublishedFileSetChangeDescription")))(rs, h, "v2") & 0xff);
    CHECK(!(((Fn_rs_set_s)vt(rs, slot_of(rv, "UpdatePublishedFileTitle")))(rs, h + 99, "x") & 0xff));
    call = ((Fn_rs_call)vt(rs, slot_of(rv, "CommitPublishedFileUpdate")))(rs, h);
    CHECK(call != 0 && call_taken(call, 1316, res, sizeof res));
    memcpy(&result, res, 4);
    memcpy(&item, res + 8, 8);
    CHECK(result == 1 && item == 777);
    CHECK(((Fn_rs_call)vt(rs, slot_of(rv, "CommitPublishedFileUpdate")))(rs, h) == 0);
    call = ((Fn_rs_call)vt(rs, slot_of(rv, "DeletePublishedFile")))(rs, 777);
    CHECK(call != 0 && call_taken(call, 1311, res, 16));
    memcpy(&result, res, 4);
    memcpy(&item, res + 8, 8);
    CHECK(result == 1 && item == 777);
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_rs_u32)(void *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_rs_id_u32)(void *, uint64_t, uint32_t);
typedef uint64_t (SHIM_TC *Fn_rs_shared)(void *, uint64_t, uint32_t, const ParamStrings *, const ParamStrings *);
typedef uint64_t (SHIM_TC *Fn_rs_workshop)(void *, uint32_t, uint32_t, uint32_t, uint32_t, const ParamStrings *, const ParamStrings *);
typedef uint64_t (SHIM_TC *Fn_rs_video)(void *, uint32_t, const char *, const char *, const char *, uint32_t, const char *, const char *, uint32_t, const ParamStrings *);
typedef uint64_t (SHIM_TC *Fn_rs_dl_to)(void *, uint64_t, const char *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_rs_progress)(void *, uint64_t, int32_t *, int32_t *);
typedef uint64_t (SHIM_TC *Fn_rs_ugcdet)(void *, uint64_t, uint32_t *, char **, int32_t *, uint64_t *);
typedef uint64_t (SHIM_TC *Fn_rs_read)(void *, uint64_t, void *, int32_t, uint32_t, uint32_t);

static int t_legacy_workshop(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    const char *rv = "STEAMREMOTESTORAGE_INTERFACE_VERSION016";
    void *rs = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamRemoteStorage")))(client, 1, 1, rv);
    CHECK(rs);
    static uint8_t res[16384];
    int32_t i32;
    uint32_t u32;
    uint64_t u64;
    const char *tagv[1] = { "map" };
    ParamStrings tags = { tagv, 1 };
    uint64_t call = ((Fn_rs_id_u32)vt(rs, slot_of(rv, "GetPublishedFileDetails")))(rs, 777, 0);
    CHECK(call != 0 && call_taken(call, 1318, res, 9760));
    memcpy(&i32, res, 4); CHECK(i32 == 1);
    memcpy(&u64, res + 8, 8); CHECK(u64 == 777);
    memcpy(&u32, res + 16, 4); CHECK(u32 == 480);
    CHECK(!strcmp((char *)res + 24, "Magic Castle"));
    CHECK(!strcmp((char *)res + 153, "A castle mod"));
    memcpy(&u64, res + 8160, 8); CHECK(u64 == 555);
    memcpy(&u64, res + 8176, 8); CHECK(u64 == 76561198000000042ULL);
    memcpy(&u32, res + 8184, 4); CHECK(u32 == 1700000000u);
    CHECK(!strcmp((char *)res + 8197, "mod,fun"));
    CHECK(!strcmp((char *)res + 9223, "castle.zip"));
    memcpy(&i32, res + 9484, 4); CHECK(i32 == 4096);
    CHECK(!strncmp((char *)res + 9492, "http://127.0.0.1:", 17));

    call = ((Fn_rs_u32)vt(rs, slot_of(rv, "EnumerateUserPublishedFiles")))(rs, 0);
    CHECK(call != 0 && call_taken(call, 1312, res, 416));
    memcpy(&i32, res, 4); CHECK(i32 == 1);
    memcpy(&i32, res + 4, 4); CHECK(i32 == 2);
    memcpy(&i32, res + 8, 4); CHECK(i32 == 7);
    memcpy(&u64, res + 16, 8); CHECK(u64 == 777);
    memcpy(&u64, res + 24, 8); CHECK(u64 == 778);

    call = ((Fn_rs_shared)vt(rs, slot_of(rv, "EnumerateUserSharedWorkshopFiles")))(rs, 76561198000000042ULL, 0, &tags, NULL);
    CHECK(call != 0 && call_taken(call, 1326, res, 416));
    memcpy(&i32, res + 4, 4); CHECK(i32 == 2);

    call = ((Fn_rs_workshop)vt(rs, slot_of(rv, "EnumeratePublishedWorkshopFiles")))(rs, 2, 10, 1, 7, &tags, NULL);
    CHECK(call != 0 && call_taken(call, 1319, res, 624));
    memcpy(&i32, res + 4, 4); CHECK(i32 == 1);
    memcpy(&i32, res + 8, 4); CHECK(i32 == 99);
    memcpy(&u64, res + 16, 8); CHECK(u64 == 700);
    memcpy(&u32, res + 616, 4); CHECK(u32 == 2357570);
    memcpy(&u32, res + 620, 4); CHECK(u32 == 10);

    call = ((Fn_rs_call)vt(rs, slot_of(rv, "GetPublishedItemVoteDetails")))(rs, 777);
    CHECK(call != 0 && call_taken(call, 1320, res, 32));
    memcpy(&i32, res + 16, 4); CHECK(i32 == 30);
    memcpy(&i32, res + 20, 4); CHECK(i32 == 2);
    memcpy(&i32, res + 24, 4); CHECK(i32 == 3);
    call = ((Fn_rs_call)vt(rs, slot_of(rv, "GetUserPublishedItemVoteDetails")))(rs, 777);
    CHECK(call != 0 && call_taken(call, 1325, res, 24));
    memcpy(&i32, res + 16, 4); CHECK(i32 == 1);
    call = ((Fn_rs_id_u32)vt(rs, slot_of(rv, "UpdateUserPublishedItemVote")))(rs, 777, 0);
    CHECK(call != 0 && call_taken(call, 1324, res, 16));
    memcpy(&i32, res, 4); CHECK(i32 == 1);

    call = ((Fn_rs_id_u32)vt(rs, slot_of(rv, "SetUserPublishedFileAction")))(rs, 777, 1);
    CHECK(call != 0 && call_taken(call, 1327, res, 24));
    call = ((Fn_rs_id_u32)vt(rs, slot_of(rv, "EnumeratePublishedFilesByUserAction")))(rs, 1, 0);
    CHECK(call != 0 && call_taken(call, 1328, res, 616));
    memcpy(&i32, res + 8, 4); CHECK(i32 == 1);
    memcpy(&u64, res + 16, 8); CHECK(u64 == 777);

    call = ((Fn_rs_video)vt(rs, slot_of(rv, "PublishVideo")))(rs, 1, "channel", "dQw4w9WgXcQ", "cover.png", 480, "Video", "Desc", 0, &tags);
    CHECK(call != 0 && call_taken(call, 1309, res, 24));
    memcpy(&i32, res, 4); CHECK(i32 == 1);
    memcpy(&u64, res + 8, 8); CHECK(u64 == 888);
    call = ((Fn_rs_video)vt(rs, slot_of(rv, "PublishVideo")))(rs, 0, NULL, "x", NULL, 480, "V", "D", 0, NULL);
    CHECK(call != 0 && call_taken(call, 1309, res, 24));
    memcpy(&i32, res, 4); CHECK(i32 == 8);

    uint32_t app = 0;
    char *name = NULL;
    int32_t size = 0;
    uint64_t owner = 0;
    CHECK(((Fn_rs_ugcdet)vt(rs, slot_of(rv, "GetUGCDetails")))(rs, 555, &app, &name, &size, &owner) & 0xff);
    CHECK(app == 480 && size == 4096 && owner == 76561198000000042ULL && name && !strcmp(name, "castle.zip"));
    CHECK(!(((Fn_rs_ugcdet)vt(rs, slot_of(rv, "GetUGCDetails")))(rs, 123456, &app, &name, &size, &owner) & 0xff));
    call = ((Fn_rs_id_u32)vt(rs, slot_of(rv, "UGCDownload")))(rs, 555, 1);
    CHECK(call != 0 && call_taken(call, 1317, res, 296));
    memcpy(&i32, res, 4); CHECK(i32 == 1);
    memcpy(&u64, res + 8, 8); CHECK(u64 == 555);
    memcpy(&i32, res + 20, 4); CHECK(i32 == 11);
    int32_t got = 0, expected = 0;
    CHECK(((Fn_rs_progress)vt(rs, slot_of(rv, "GetUGCDownloadProgress")))(rs, 555, &got, &expected) & 0xff);
    CHECK(got == 11 && expected == 11);
    char data[16] = {0};
    CHECK((uint32_t)((Fn_rs_read)vt(rs, slot_of(rv, "UGCRead")))(rs, 555, data, 5, 0, 0) == 5 && !memcmp(data, "mod-c", 5));
    CHECK((uint32_t)((Fn_rs_read)vt(rs, slot_of(rv, "UGCRead")))(rs, 555, data, 16, 5, 2) == 6 && !memcmp(data, "ontent", 6));
    CHECK((uint32_t)((Fn_rs_read)vt(rs, slot_of(rv, "UGCRead")))(rs, 999, data, 16, 0, 0) == 0);
    call = ((Fn_rs_id_u32)vt(rs, slot_of(rv, "UGCDownload")))(rs, 999, 1);
    CHECK(call != 0 && call_taken(call, 1317, res, 296));
    memcpy(&i32, res, 4); CHECK(i32 == 9);
    return 1;
}

static int t_extras(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    uint8_t res[512];
    int32_t i32;
    uint64_t u64;
    {
        FILE *f = fopen("extras_sha.txt", "wb");
        CHECK(f);
        fwrite("hello", 1, 5, f);
        fclose(f);
        char full[MAX_PATH];
        CHECK(GetFullPathNameA("extras_sha.txt", sizeof full, full, NULL));
        const char *an = "STEAMAPPS_INTERFACE_VERSION008";
        void *apps = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamApps")))(client, 1, 1, an);
        CHECK(apps);
        typedef uint64_t (SHIM_TC *Fn_details)(void *, const char *);
        uint64_t call = ((Fn_details)vt(apps, slot_of(an, "GetFileDetails")))(apps, full);
        CHECK(call != 0 && call_taken(call, 1023, res, 40));
        memcpy(&i32, res, 4); CHECK(i32 == 1);
        memcpy(&u64, res + 8, 8); CHECK(u64 == 5);
        static const uint8_t sha[20] = { 0xaa, 0xf4, 0xc6, 0x1d, 0xdc, 0xc5, 0xe8, 0xa2, 0xda, 0xbe, 0xde, 0x0f, 0x3b, 0x48, 0x2c, 0xd9, 0xae, 0xa9, 0x43, 0x4d };
        CHECK(!memcmp(res + 16, sha, 20));
        call = ((Fn_details)vt(apps, slot_of(an, "GetFileDetails")))(apps, "nao_existe.bin");
        CHECK(call != 0 && call_taken(call, 1023, res, 40));
        memcpy(&i32, res, 4); CHECK(i32 == 9);
        remove("extras_sha.txt");
    }
    {
        const char *fn = "SteamFriends017";
        void *fr = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamFriends")))(client, 1, 1, fn);
        CHECK(fr);
        typedef uint64_t (SHIM_TC *Fn_id)(void *, uint64_t);
        uint64_t call = ((Fn_id)vt(fr, slot_of(fn, "IsFollowing")))(fr, 76561198000000042ULL);
        {
            bool failed = false;
            CHECK(call != 0 && wait_call_result(call, &failed) && failed);
        }
        call = ((Fn_id)vt(fr, slot_of(fn, "JoinClanChatRoom")))(fr, 103582791429521412ULL);
        CHECK(call != 0 && call_taken(call, 342, res, 16));
        memcpy(&i32, res + 8, 4); CHECK(i32 == 5);
    }
    {
        const char *iv = "STEAMUGC_INTERFACE_VERSION018";
        void *ugc = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamUGC")))(client, 1, 1, iv);
        CHECK(ugc);
        typedef uint64_t (SHIM_TC *Fn_ii)(void *, uint32_t, uint64_t);
        uint64_t call = ((Fn_ii)vt(ugc, slot_of(iv, "AddItemToFavorites")))(ugc, 480, 777);
        CHECK(call != 0 && call_taken(call, 3407, res, 16));
        memcpy(&u64, res, 8); CHECK(u64 == 777);
        memcpy(&i32, res + 8, 4); CHECK(i32 == 2 && res[12] == 1);
        typedef uint64_t (SHIM_TC *Fn_v)(void *);
        call = ((Fn_v)vt(ugc, slot_of(iv, "GetWorkshopEULAStatus")))(ugc);
        CHECK(call != 0 && call_taken(call, 3419, res, 24));
        memcpy(&i32, res, 4); CHECK(i32 == 1 && res[16] == 1);
    }
    {
        const char *rv = "STEAMREMOTESTORAGE_INTERFACE_VERSION016";
        void *rs = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamRemoteStorage")))(client, 1, 1, rv);
        CHECK(rs);
        typedef uint64_t (SHIM_TC *Fn_name)(void *, const char *);
        uint64_t call = ((Fn_name)vt(rs, slot_of(rv, "FileShare")))(rs, "save.dat");
        {
            bool failed = false;
            CHECK(call != 0 && wait_call_result(call, &failed) && failed);
        }
        const char *hv = "STEAMHTMLSURFACE_INTERFACE_VERSION_005";
        void *html = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamHTMLSurface")))(client, 1, 1, hv);
        if (html) {
            typedef uint64_t (SHIM_TC *Fn_v)(void *);
            CHECK(((Fn_v)vt(html, slot_of(hv, "Init")))(html) & 0xff);
            CHECK(((Fn_v)vt(html, slot_of(hv, "Shutdown")))(html) & 0xff);
        }
    }
    return 1;
}

static int t_ugc(void) {
    shim_init();
    const char *iv = "STEAMUGC_INTERFACE_VERSION018";
    void *client = create("SteamClient023");
    CHECK(client);
    void *ugc = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamUGC")))(client, 1, 1, iv);
    CHECK(ugc);
    CHECK(((Fn_ugc_u32)vt(ugc, slot_of(iv, "GetNumSubscribedItems")))(ugc) == 2);
    {
        typedef uint64_t (SHIM_TC *Fn_x_setbeta)(void *, const char *);
        const char *an = "STEAMAPPS_INTERFACE_VERSION008";
        void *apps = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamApps")))(client, 1, 1, an);
        CHECK(apps);
        CHECK(((Fn_x_setbeta)vt(apps, slot_of(an, "SetActiveBeta")))(apps, "beta") & 0xff);
        CHECK(!(((Fn_x_setbeta)vt(apps, slot_of(an, "SetActiveBeta")))(apps, "internal") & 0xff));
    }
    uint64_t ids[4] = {0};
    CHECK(((Fn_ugc_items)vt(ugc, slot_of(iv, "GetSubscribedItems")))(ugc, ids, 4) == 2 && ids[0] == 111 && ids[1] == 222);
    CHECK(((Fn_ugc_state)vt(ugc, slot_of(iv, "GetItemState")))(ugc, 111) == 5);
    CHECK(((Fn_ugc_state)vt(ugc, slot_of(iv, "GetItemState")))(ugc, 222) == 1);
    uint64_t size = 0;
    uint32_t when = 0;
    char folder[260];
    CHECK(((Fn_ugc_info)vt(ugc, slot_of(iv, "GetItemInstallInfo")))(ugc, 111, &size, folder, sizeof folder, &when) & 0xff);
    CHECK(size == 5000 && when == 1700 && !strncmp(folder, "Z:\\", 3) && strstr(folder, "111"));
    CHECK(!(((Fn_ugc_info)vt(ugc, slot_of(iv, "GetItemInstallInfo")))(ugc, 222, &size, folder, sizeof folder, &when) & 0xff));
    CHECK(((Fn_ugc_dl)vt(ugc, slot_of(iv, "DownloadItem")))(ugc, 222, 1) & 0xff);
    uint8_t cb[32];
    CHECK(wait_callback(3406, cb, sizeof cb));
    uint64_t fileid;
    int32_t result;
    memcpy(&fileid, cb + 8, 8);
    memcpy(&result, cb + 16, 4);
    CHECK(fileid == 222 && result == 1);
    uint64_t call = ((Fn_ugc_call)vt(ugc, slot_of(iv, "SubscribeItem")))(ugc, 333);
    CHECK(call_taken(call, 1313, cb, sizeof cb));
    memcpy(&result, cb, 4);
    CHECK(result == 1);
    CHECK(((Fn_ugc_u32)vt(ugc, slot_of(iv, "GetNumSubscribedItems")))(ugc) == 3);
    {
        const char *rv = "STEAMREMOTESTORAGE_INTERFACE_VERSION016";
        void *rs = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamRemoteStorage")))(client, 1, 1, rv);
        CHECK(rs);
        uint64_t c2 = ((Fn_ugc_u32_call)vt(rs, slot_of(rv, "EnumerateUserSubscribedFiles")))(rs, 0);
        uint8_t big[16 + 8 * 50 + 4 * 50];
        CHECK(call_taken(c2, 1314, big, sizeof big));
        int32_t res, got, total;
        memcpy(&res, big, 4); memcpy(&got, big + 4, 4); memcpy(&total, big + 8, 4);
        uint64_t first, third;
        memcpy(&first, big + 16, 8); memcpy(&third, big + 16 + 16, 8);
        CHECK(res == 1 && got == 3 && total == 3 && first == 111 && third == 333);
    }
    return 1;
}

typedef struct { void **vtbl; int responded[8], n_responded, failed[8], n_failed, complete_code, completes; int refreshed; } ListResp;
static void SHIM_TC lr_responded(void *self, void *req, int i) { (void)req; ListResp *r = self; if (r->n_responded < 8) r->responded[r->n_responded++] = i; }
static void SHIM_TC lr_failed(void *self, void *req, int i) { (void)req; ListResp *r = self; if (r->n_failed < 8) r->failed[r->n_failed++] = i; }
static void SHIM_TC lr_complete(void *self, void *req, int code) { (void)req; ListResp *r = self; r->complete_code = code; r->completes++; }
static void *g_list_vtbl[3] = { (void *)lr_responded, (void *)lr_failed, (void *)lr_complete };

typedef struct { void **vtbl; int ok, failed; char name[64]; int ping, players; } PingResp;
static void SHIM_TC pr_ok(void *self, void *item) {
    PingResp *r = self;
    r->ok++;
    snprintf(r->name, sizeof r->name, "%s", (char *)item + 172);
    memcpy(&r->ping, (char *)item + 8, 4);
    memcpy(&r->players, (char *)item + 148, 4);
}
static void SHIM_TC pr_fail(void *self) { ((PingResp *)self)->failed++; }
static void *g_ping_vtbl[2] = { (void *)pr_ok, (void *)pr_fail };

typedef struct { void **vtbl; int n, failed, complete; char first[64]; int score; } PlayersResp;
static void SHIM_TC pl_add(void *self, const char *name, int score, float time) { (void)time; PlayersResp *r = self; if (!r->n++) { snprintf(r->first, sizeof r->first, "%s", name); r->score = score; } }
static void SHIM_TC pl_fail(void *self) { ((PlayersResp *)self)->failed++; }
static void SHIM_TC pl_done(void *self) { ((PlayersResp *)self)->complete++; }
static void *g_players_vtbl[3] = { (void *)pl_add, (void *)pl_fail, (void *)pl_done };

typedef struct { void **vtbl; int n, failed, complete; char key[64], value[64]; } RulesResp;
static void SHIM_TC ru_add(void *self, const char *k, const char *v) { RulesResp *r = self; if (!r->n++) { snprintf(r->key, sizeof r->key, "%s", k); snprintf(r->value, sizeof r->value, "%s", v); } }
static void SHIM_TC ru_fail(void *self) { ((RulesResp *)self)->failed++; }
static void SHIM_TC ru_done(void *self) { ((RulesResp *)self)->complete++; }
static void *g_rules_vtbl[3] = { (void *)ru_add, (void *)ru_fail, (void *)ru_done };

typedef uint64_t (SHIM_TC *Fn_ms_req)(void *, uint32_t, void **, uint32_t, void *);
typedef int (SHIM_TC *Fn_ms_count)(void *, void *);
typedef uint64_t (SHIM_TC *Fn_ms_lan2)(void *, uint32_t, void *);
typedef void *(SHIM_TC *Fn_ms_details)(void *, void *, int);
typedef uint32_t (SHIM_TC *Fn_ms_query)(void *, uint32_t, uint16_t, void *);
typedef void (SHIM_TC *Fn_ms_release)(void *, void *);
typedef int (SHIM_TC *Fn_fav_count)(void *);
typedef uint64_t (SHIM_TC *Fn_fav_get)(void *, int, uint32_t *, uint32_t *, uint16_t *, uint16_t *, uint32_t *, uint32_t *);
typedef int (SHIM_TC *Fn_fav_add)(void *, uint32_t, uint32_t, uint16_t, uint16_t, uint32_t, uint32_t);

static int pump_until(int *flag, int want) {
    for (int i = 0; i < 100 && *flag < want; i++) {
        ShimCallbackMsg m;
        while (shim_next_callback(&m)) shim_free_last_callback();
        if (*flag >= want) break;
        Sleep(50);
    }
    return *flag >= want;
}

static int t_servers(void) {
    shim_init();
    const char *iv = "SteamMatchMakingServers002";
    uint32_t port = (uint32_t)atoi(getenv("SELFTEST_SERVER_PORT"));
    void *client = create("SteamClient023");
    CHECK(client);
    void *ms = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamMatchmakingServers")))(client, 1, 1, iv);
    CHECK(ms);
    char filter[512];
    memset(filter, 0, sizeof filter);
    strcpy(filter, "map");
    strcpy(filter + 256, "de_test");
    void *filters[1] = { filter };
    ListResp lr;
    memset(&lr, 0, sizeof lr);
    lr.vtbl = g_list_vtbl;
    uint64_t h = ((Fn_ms_req)vt(ms, slot_of(iv, "RequestInternetServerList")))(ms, 440, filters, 1, &lr);
    CHECK(h != 0);
    CHECK(pump_until(&lr.completes, 1));
    CHECK(lr.complete_code == 0);
    CHECK(((Fn_ms_count)vt(ms, slot_of(iv, "GetServerCount")))(ms, (void *)(uintptr_t)h) == 2);
    CHECK(lr.n_responded == 1 && lr.n_failed == 1);
    uint8_t *item = ((Fn_ms_details)vt(ms, slot_of(iv, "GetServerDetails")))(ms, (void *)(uintptr_t)h, lr.responded[0]);
    CHECK(item);
    uint16_t qp, cp;
    uint32_t ip, ping, players, max_players, app;
    memcpy(&cp, item, 2); memcpy(&qp, item + 2, 2); memcpy(&ip, item + 4, 4); memcpy(&ping, item + 8, 4);
    memcpy(&app, item + 144, 4); memcpy(&players, item + 148, 4); memcpy(&max_players, item + 152, 4);
    CHECK(ip == 0x7F000001u && qp == port && cp == 27030 && ping >= 1 && item[12] == 1);
    CHECK(!strcmp((char *)item + 172, "Test Server") && !strcmp((char *)item + 46, "de_test") && !strcmp((char *)item + 14, "tf"));
    CHECK(app == 440 && players == 5 && max_players == 24 && item[160] == 0 && item[161] == 1);
    CHECK(!strcmp((char *)item + 236, "alltalk,test"));
    uint64_t sid;
    memcpy(&sid, item + 364, 8);
    CHECK(sid == 85568392926520363ULL);
    ((Fn_ms_release)vt(ms, slot_of(iv, "ReleaseRequest")))(ms, (void *)(uintptr_t)h);
    PingResp pr;
    memset(&pr, 0, sizeof pr);
    pr.vtbl = g_ping_vtbl;
    CHECK(((Fn_ms_query)vt(ms, slot_of(iv, "PingServer")))(ms, 0x7F000001u, (uint16_t)port, &pr) != (uint32_t)-1);
    CHECK(pump_until(&pr.ok, 1) && !strcmp(pr.name, "Test Server") && pr.ping >= 1 && pr.players == 5);
    PlayersResp pl;
    memset(&pl, 0, sizeof pl);
    pl.vtbl = g_players_vtbl;
    CHECK(((Fn_ms_query)vt(ms, slot_of(iv, "PlayerDetails")))(ms, 0x7F000001u, (uint16_t)port, &pl) != (uint32_t)-1);
    CHECK(pump_until(&pl.complete, 1) && pl.n == 2 && !strcmp(pl.first, "Ana") && pl.score == 7);
    RulesResp ru;
    memset(&ru, 0, sizeof ru);
    ru.vtbl = g_rules_vtbl;
    CHECK(((Fn_ms_query)vt(ms, slot_of(iv, "ServerRules")))(ms, 0x7F000001u, (uint16_t)port, &ru) != (uint32_t)-1);
    CHECK(pump_until(&ru.complete, 1) && ru.n == 2 && !strcmp(ru.key, "mp_friendlyfire") && !strcmp(ru.value, "0"));
    ListResp fav, hist, fr;
    memset(&fav, 0, sizeof fav); memset(&hist, 0, sizeof hist); memset(&fr, 0, sizeof fr);
    fav.vtbl = hist.vtbl = fr.vtbl = g_list_vtbl;
    uint64_t hf = ((Fn_ms_req)vt(ms, slot_of(iv, "RequestFavoritesServerList")))(ms, 440, NULL, 0, &fav);
    CHECK(hf && pump_until(&fav.completes, 1) && fav.complete_code == 0 && fav.n_responded == 1);
    uint64_t hh = ((Fn_ms_req)vt(ms, slot_of(iv, "RequestHistoryServerList")))(ms, 440, NULL, 0, &hist);
    CHECK(hh && pump_until(&hist.completes, 1) && hist.n_failed == 1 && hist.n_responded == 0);
    uint64_t hr = ((Fn_ms_req)vt(ms, slot_of(iv, "RequestFriendsServerList")))(ms, 440, NULL, 0, &fr);
    CHECK(hr && pump_until(&fr.completes, 1) && fr.n_responded == 1);
    uint8_t *fitem = ((Fn_ms_details)vt(ms, slot_of(iv, "GetServerDetails")))(ms, (void *)(uintptr_t)hr, fr.responded[0]);
    CHECK(fitem && !strcmp((char *)fitem + 172, "Test Server"));
    {
        const char *mv = "SteamMatchMaking009";
        void *mm = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamMatchmaking")))(client, 1, 1, mv);
        CHECK(mm);
        CHECK(((Fn_fav_count)vt(mm, slot_of(mv, "GetFavoriteGameCount")))(mm) == 2);
        uint32_t fapp, fip, fflags, fwhen;
        uint16_t fconn, fquery;
        CHECK(((Fn_fav_get)vt(mm, slot_of(mv, "GetFavoriteGame")))(mm, 0, &fapp, &fip, &fconn, &fquery, &fflags, &fwhen) & 0xff);
        CHECK(fapp == 440 && fip == 0x7F000001u && fconn == port && fflags == 1 && fwhen == 5);
        CHECK(((Fn_fav_add)vt(mm, slot_of(mv, "AddFavoriteGame")))(mm, 440, 0x7F000001u, 27031, 27031, 1, 123) >= 0);
        CHECK(((Fn_fav_count)vt(mm, slot_of(mv, "GetFavoriteGameCount")))(mm) == 3);
        Sleep(800);
    }
    PingResp dead;
    memset(&dead, 0, sizeof dead);
    dead.vtbl = g_ping_vtbl;
    CHECK(((Fn_ms_query)vt(ms, slot_of(iv, "PingServer")))(ms, 0x7F000001u, 1, &dead) != (uint32_t)-1);
    CHECK(pump_until(&dead.failed, 1) && dead.ok == 0);
    return 1;
}

static int t_servers_offline(void) {
    CHECK(setup());
    const char *iv = "SteamMatchMakingServers002";
    void *ms = ((Fn_p_iip)vt(g_client, 12))(g_client, g_user, g_pipe, iv);
    CHECK(ms);
    ListResp net, lan, fav;
    memset(&net, 0, sizeof net); memset(&lan, 0, sizeof lan); memset(&fav, 0, sizeof fav);
    net.vtbl = lan.vtbl = fav.vtbl = g_list_vtbl;
    uint64_t hn = ((Fn_ms_req)vt(ms, slot_of(iv, "RequestInternetServerList")))(ms, 500, NULL, 0, &net);
    CHECK(hn != 0 && net.completes == 0);
    uint64_t hl = ((Fn_ms_lan2)vt(ms, slot_of(iv, "RequestLANServerList")))(ms, 500, &lan);
    CHECK(hl != 0);
    uint64_t hf = ((Fn_ms_req)vt(ms, slot_of(iv, "RequestFavoritesServerList")))(ms, 500, NULL, 0, &fav);
    CHECK(hf != 0);
    CHECK(pump_until(&net.completes, 1) && net.complete_code == 1);
    CHECK(pump_until(&fav.completes, 1) && fav.complete_code == 2);
    CHECK(pump_until(&lan.completes, 1) && lan.complete_code == 0 && ((Fn_ms_count)vt(ms, slot_of(iv, "GetServerCount")))(ms, (void *)(uintptr_t)hl) == 0);
    ((Fn_ms_release)vt(ms, slot_of(iv, "ReleaseRequest")))(ms, (void *)(uintptr_t)hn);
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_uq_all)(void *, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t);
typedef uint64_t (SHIM_TC *Fn_uq_user)(void *, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t, uint32_t);
typedef uint64_t (SHIM_TC *Fn_uq_ids)(void *, const uint64_t *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_uq_h)(void *, uint64_t);
typedef uint64_t (SHIM_TC *Fn_uq_hs)(void *, uint64_t, const char *);
typedef uint64_t (SHIM_TC *Fn_uq_hb)(void *, uint64_t, uint8_t);
typedef uint64_t (SHIM_TC *Fn_uq_res)(void *, uint64_t, uint32_t, void *);
typedef uint32_t (SHIM_TC *Fn_uq_n)(void *, uint64_t, uint32_t);
typedef uint64_t (SHIM_TC *Fn_uq_str)(void *, uint64_t, uint32_t, uint32_t, char *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_uq_url)(void *, uint64_t, uint32_t, char *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_uq_ch)(void *, uint64_t, uint32_t, uint64_t *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_uq_stat)(void *, uint64_t, uint32_t, uint32_t, uint64_t *);
typedef uint64_t (SHIM_TC *Fn_uq_kv)(void *, uint64_t, uint32_t, const char *, char *, uint32_t);

typedef uint64_t (SHIM_TC *Fn_gs_init)(void *, uint32_t, uint16_t, uint16_t, uint32_t, uint32_t, const char *);
typedef void (SHIM_TC *Fn_gs_str)(void *, const char *);
typedef void (SHIM_TC *Fn_gs_int)(void *, int);
typedef void (SHIM_TC *Fn_gs_kv)(void *, const char *, const char *);
typedef void (SHIM_TC *Fn_gs_v)(void *);
typedef uint64_t (SHIM_TC *Fn_gs_b)(void *);
typedef uint64_t *(SHIM_TC *Fn_gs_sid)(void *, uint64_t *);
typedef uint64_t (SHIM_TC *Fn_gs_upd)(void *, uint64_t, const char *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_gs_pkt)(void *, const void *, int, uint32_t, uint16_t);
typedef int (SHIM_TC *Fn_gs_out)(void *, void *, int, uint32_t *, uint16_t *);
typedef uint32_t (SHIM_TC *Fn_gs_lic)(void *, uint64_t, uint32_t);

typedef uint32_t (SHIM_TC *Fn_web_ticket)(void *, const char *);
static int t_live(void) {
    shim_init();
    printf("live: steamid=%llu app=%u\n", (unsigned long long)shim_snapshot()->steamid, shim_snapshot()->has_ticket ? shim_snapshot()->ticket.appid : 0);
    void *client = create("SteamClient023");
    CHECK(client);
    const char *uv = "STEAMUSERSTATS_INTERFACE_VERSION012";
    void *st = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamUserStats")))(client, 1, 1, uv);
    CHECK(st);
    uint8_t res[64];
    uint64_t h = ((Fn_lb_call0)vt(st, slot_of(uv, "GetNumberOfCurrentPlayers")))(st);
    CHECK(call_taken(h, 1107, res, sizeof res));
    int32_t players;
    memcpy(&players, res + 4, 4);
    printf("live: players=%d success=%d\n", players, res[0]);
    CHECK(res[0] == 1 && players > 0);
    h = ((Fn_lb_find)vt(st, slot_of(uv, "FindOrCreateLeaderboard")))(st, "Quickest Win", 1, 2);
    CHECK(call_taken(h, 1104, res, sizeof res));
    uint64_t board;
    memcpy(&board, res, 8);
    printf("live: leaderboard=%llu found=%d name=%s count=%d\n", (unsigned long long)board, res[8],
           ((Fn_lb_str)vt(st, slot_of(uv, "GetLeaderboardName")))(st, board), ((Fn_lb_int)vt(st, slot_of(uv, "GetLeaderboardEntryCount")))(st, board));
    CHECK(res[8] == 1);
    h = ((Fn_lb_dl)vt(st, slot_of(uv, "DownloadLeaderboardEntries")))(st, board, 0, 1, 3);
    CHECK(call_taken(h, 1105, res, sizeof res));
    uint64_t set;
    int32_t count;
    memcpy(&set, res + 8, 8);
    memcpy(&count, res + 16, 4);
    for (int i = 0; i < count; i++) {
        uint8_t entry[32];
        int32_t details[4];
        CHECK(((Fn_lb_entry)vt(st, slot_of(uv, "GetDownloadedLeaderboardEntry")))(st, set, i, entry, details, 4) & 0xff);
        uint64_t who;
        int32_t rank, score;
        memcpy(&who, entry, 8); memcpy(&rank, entry + 8, 4); memcpy(&score, entry + 12, 4);
        printf("live: entry %d steamid=%llu rank=%d score=%d\n", i, (unsigned long long)who, rank, score);
    }
    CHECK(count == 3);
    const char *mv = "SteamMatchMaking009";
    void *mm = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamMatchmaking")))(client, 1, 1, mv);
    CHECK(mm);
    h = ((Fn_lob_create)vt(mm, slot_of(mv, "CreateLobby")))(mm, 2, 4);
    CHECK(call_taken(h, 513, res, sizeof res));
    uint64_t lobby;
    int32_t eresult;
    memcpy(&eresult, res, 4);
    memcpy(&lobby, res + 8, 8);
    printf("live: lobby=%llx eresult=%d\n", (unsigned long long)lobby, eresult);
    CHECK(eresult == 1 && lobby);
    CHECK(((Fn_lob_skv)vt(mm, slot_of(mv, "SetLobbyData")))(mm, lobby, "shimprobe", "ok") & 0xff);
    CHECK(wait_callback(505, res, sizeof res));
    printf("live: lobby data=%s\n", ((Fn_lob_get)vt(mm, slot_of(mv, "GetLobbyData")))(mm, lobby, "shimprobe"));
    ((Fn_lob_member_set)vt(mm, slot_of(mv, "SetLobbyMemberData")))(mm, lobby, "mine", "1");
    CHECK(wait_callback(505, res, sizeof res));
    printf("live: member data=%s\n", ((Fn_lob_mget)vt(mm, slot_of(mv, "GetLobbyMemberData")))(mm, lobby, shim_snapshot()->steamid, "mine"));
    CHECK(((Fn_lob_chat)vt(mm, slot_of(mv, "SendLobbyChatMsg")))(mm, lobby, "oi", 3) & 0xff);
    CHECK(wait_callback(507, res, sizeof res));
    printf("live: chat ok\n");
    ((Fn_lob_sfilter)vt(mm, slot_of(mv, "AddRequestLobbyListStringFilter")))(mm, "shimprobe", "ok", 0);
    h = ((Fn_lob_list)vt(mm, slot_of(mv, "RequestLobbyList")))(mm);
    CHECK(call_taken(h, 510, res, sizeof res));
    uint32_t found;
    memcpy(&found, res, 4);
    printf("live: lobby search found=%u\n", found);
    CHECK(found >= 1);
    ((Fn_lob_leave)vt(mm, slot_of(mv, "LeaveLobby")))(mm, lobby);
    {
        const char *tv = "SteamUser022";
        void *tu = ((Fn_p_iip)vt(client, 5))(client, 1, 1, tv);
        uint8_t tk[2048];
        uint32_t tn = 0;
        uint32_t th = ((Fn_auth3)vt(tu, slot_of(tv, "GetAuthSessionTicket")))(tu, tk, sizeof tk, &tn);
        CHECK(th != 0 && tn > 100);
        int verdict = ((Fn_us_begin)vt(tu, slot_of(tv, "BeginAuthSession")))(tu, tk, (int)tn, shim_snapshot()->steamid);
        printf("live: ticket of %u bytes, offline verification=%d\n", tn, verdict);
        CHECK(verdict == 0);
        tk[tn - 10] ^= 1;
        CHECK(((Fn_us_begin)vt(tu, slot_of(tv, "BeginAuthSession")))(tu, tk, (int)tn, shim_snapshot()->steamid) == 1);
    }
    {
        const char *gv = "SteamGameServer013";
        void *gsv = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamGameServer")))(client, 1, 1, gv);
        ((Fn_gs_init)vt(gsv, slot_of(gv, "InitGameServer")))(gsv, 0, 27015, 27016, 3, 480, "1.0.0.0");
        ((Fn_gs_v)vt(gsv, slot_of(gv, "LogOnAnonymous")))(gsv);
        uint8_t gcb[16];
        CHECK(wait_callback(115, gcb, sizeof gcb));
        uint64_t gsid = 0;
        ((Fn_gs_sid)vt(gsv, slot_of(gv, "GetSteamID")))(gsv, &gsid);
        printf("live: game server SteamID=%llu (type %u)\n", (unsigned long long)gsid, (unsigned)((gsid >> 52) & 0xF));
        CHECK(gsid != 0 && ((gsid >> 52) & 0xF) == 4);
        ((Fn_gs_str)vt(gsv, slot_of(gv, "SetServerName")))(gsv, "AureliaPlus live check");
        ((Fn_gs_str)vt(gsv, slot_of(gv, "SetMapName")))(gsv, "live_map");
        ((Fn_gs_str)vt(gsv, slot_of(gv, "SetModDir")))(gsv, "spacewar");
        ((Fn_gs_int)vt(gsv, slot_of(gv, "SetMaxPlayerCount")))(gsv, 4);
        ((Fn_gs_v)vt(gsv, slot_of(gv, "ForceHeartbeat")))(gsv);
        Sleep(6000);
        const char *msv = "SteamMatchMakingServers002";
        void *lms = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamMatchmakingServers")))(client, 1, 1, msv);
        ListResp ll;
        memset(&ll, 0, sizeof ll);
        ll.vtbl = g_list_vtbl;
        char lflt[512];
        memset(lflt, 0, sizeof lflt);
        strcpy(lflt, "map");
        strcpy(lflt + 256, "live_map");
        void *lfilters[1] = { lflt };
        uint64_t lh = ((Fn_ms_req)vt(lms, slot_of(msv, "RequestInternetServerList")))(lms, 480, lfilters, 1, &ll);
        CHECK(lh && pump_until(&ll.completes, 1));
        int lt = ((Fn_ms_count)vt(lms, slot_of(msv, "GetServerCount")))(lms, (void *)(uintptr_t)lh), mine = 0;
        for (int i = 0; i < lt; i++) {
            uint8_t *it = ((Fn_ms_details)vt(lms, slot_of(msv, "GetServerDetails")))(lms, (void *)(uintptr_t)lh, i);
            uint64_t isid;
            memcpy(&isid, it + 364, 8);
            if (isid == gsid) { mine = 1; printf("live: my server in the master list: %s, map %s\n", it + 172, it + 46); }
        }
        printf("live: master list with %d servers; mine appears: %d\n", lt, mine);
        CHECK(mine);
        ((Fn_gs_v)vt(gsv, slot_of(gv, "LogOff")))(gsv);
    }
    {
        const char *wv = "SteamUser023";
        void *wu = ((Fn_p_iip)vt(client, 5))(client, 1, 1, wv);
        uint32_t wh = ((Fn_web_ticket)vt(wu, slot_of(wv, "GetAuthTicketForWebApi")))(wu, "aurelia-live");
        static uint8_t wcb[4096];
        CHECK(wh != 0 && wait_callback(168, wcb, sizeof wcb));
        int32_t wres, wn;
        memcpy(&wres, wcb + 4, 4); memcpy(&wn, wcb + 8, 4);
        uint32_t wtype = 0;
        if (wn > 60) memcpy(&wtype, wcb + 12 + 4 + 20 + 4 + 4, 4);
        printf("live: Web API ticket result=%d bytes=%d header type=%u verification=%d\n", wres, wn, wtype, shim_verify_auth_ticket(wcb + 12, (uint32_t)wn, 480, shim_snapshot()->steamid, NULL));
        CHECK(wres == 1 && wtype == 5 && shim_verify_auth_ticket(wcb + 12, (uint32_t)wn, 480, shim_snapshot()->steamid, NULL) == 0);
    }
    const char *suv = "SteamUser023";
    void *u = ((Fn_p_iip)vt(client, 5))(client, 1, 1, suv);
    h = ((Fn_enc_req)vt(u, slot_of(suv, "RequestEncryptedAppTicket")))(u, "x", 1);
    CHECK(call_taken(h, 154, res, sizeof res));
    memcpy(&eresult, res, 4);
    uint32_t n = 0;
    uint8_t buf[2048];
    ((Fn_enc_get)vt(u, slot_of(suv, "GetEncryptedAppTicket")))(u, buf, sizeof buf, &n);
    printf("live: encrypted ticket eresult=%d bytes=%u\n", eresult, n);
    CHECK(eresult == 1 && n > 0);
    const char *sv = "SteamMatchMakingServers002";
    void *ms = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamMatchmakingServers")))(client, 1, 1, sv);
    ListResp lr;
    memset(&lr, 0, sizeof lr);
    lr.vtbl = g_list_vtbl;
    uint64_t hl = ((Fn_ms_req)vt(ms, slot_of(sv, "RequestInternetServerList")))(ms, 440, NULL, 0, &lr);
    CHECK(hl && pump_until(&lr.completes, 1));
    int total = ((Fn_ms_count)vt(ms, slot_of(sv, "GetServerCount")))(ms, (void *)(uintptr_t)hl);
    printf("live: servers listed=%d responded=%d failed=%d code=%d\n", total, lr.n_responded, lr.n_failed, lr.complete_code);
    if (lr.n_responded) {
        uint8_t *item = ((Fn_ms_details)vt(ms, slot_of(sv, "GetServerDetails")))(ms, (void *)(uintptr_t)hl, lr.responded[0]);
        uint32_t ping, pl, mx;
        memcpy(&ping, item + 8, 4); memcpy(&pl, item + 148, 4); memcpy(&mx, item + 152, 4);
        printf("live: server %s map=%s ping=%u players=%u/%u\n", item + 172, item + 46, ping, pl, mx);
    }
    CHECK(total > 0);
    {
        const char *fv = "SteamMatchMaking009";
        void *fm = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamMatchmaking")))(client, 1, 1, fv);
        printf("live: favorites+history=%d\n", ((Fn_fav_count)vt(fm, slot_of(fv, "GetFavoriteGameCount")))(fm));
        ListResp fl;
        memset(&fl, 0, sizeof fl);
        fl.vtbl = g_list_vtbl;
        uint64_t hf = ((Fn_ms_req)vt(ms, slot_of(sv, "RequestFriendsServerList")))(ms, 0, NULL, 0, &fl);
        CHECK(hf && pump_until(&fl.completes, 1));
        printf("live: friends' servers=%d code=%d\n", ((Fn_ms_count)vt(ms, slot_of(sv, "GetServerCount")))(ms, (void *)(uintptr_t)hf), fl.complete_code);
    }
    {
        const char *gv = "STEAMUGC_INTERFACE_VERSION018";
        void *g = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamUGC")))(client, 1, 1, gv);
        uint64_t qh = ((Fn_uq_all)vt(g, slot_of(gv, "CreateQueryAllUGCRequest")))(g, 0, 0, 480, 480, 1);
        uint64_t qc = ((Fn_uq_h)vt(g, slot_of(gv, "SendQueryUGCRequest")))(g, qh);
        uint8_t qd[24];
        CHECK(call_taken(qc, 3401, qd, sizeof qd));
        uint32_t qn, qt;
        int32_t qr;
        memcpy(&qr, qd + 8, 4); memcpy(&qn, qd + 12, 4); memcpy(&qt, qd + 16, 4);
        printf("live: workshop result=%d num=%u total=%u\n", qr, qn, qt);
        CHECK(qr == 1 && qn > 0);
        static uint8_t det[9784];
        CHECK(((Fn_uq_res)vt(g, slot_of(gv, "GetQueryUGCResult")))(g, qh, 0, det) & 0xff);
        uint64_t qid;
        uint32_t qvu;
        memcpy(&qid, det, 8); memcpy(&qvu, det + 9756, 4);
        printf("live: workshop item %llu title=%s votes_up=%u\n", (unsigned long long)qid, det + 24, qvu);
    }
    return 1;
}

static int t_ugcquery(void) {
    shim_init();
    const char *iv = "STEAMUGC_INTERFACE_VERSION018";
    void *client = create("SteamClient023");
    CHECK(client);
    void *ugc = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamUGC")))(client, 1, 1, iv);
    CHECK(ugc);
    uint64_t q = ((Fn_uq_all)vt(ugc, slot_of(iv, "CreateQueryAllUGCRequest")))(ugc, 11, 0, 480, 480, 1);
    CHECK(q != ~0ULL && q != 0);
    CHECK(((Fn_uq_hs)vt(ugc, slot_of(iv, "AddRequiredTag")))(ugc, q, "mod") & 0xff);
    CHECK(((Fn_uq_hs)vt(ugc, slot_of(iv, "SetSearchText")))(ugc, q, "castle") & 0xff);
    CHECK(((Fn_uq_hb)vt(ugc, slot_of(iv, "SetReturnKeyValueTags")))(ugc, q, 1) & 0xff);
    CHECK(((Fn_uq_hb)vt(ugc, slot_of(iv, "SetReturnMetadata")))(ugc, q, 1) & 0xff);
    CHECK(((Fn_uq_hb)vt(ugc, slot_of(iv, "SetReturnChildren")))(ugc, q, 1) & 0xff);
    uint64_t call = ((Fn_uq_h)vt(ugc, slot_of(iv, "SendQueryUGCRequest")))(ugc, q);
    uint8_t done[24];
    CHECK(call_taken(call, 3401, done, sizeof done));
    uint32_t num, total;
    int32_t result;
    memcpy(&result, done + 8, 4); memcpy(&num, done + 12, 4); memcpy(&total, done + 16, 4);
    CHECK(result == 1 && num == 2 && total == 99);
    static uint8_t d[9784];
    CHECK(((Fn_uq_res)vt(ugc, slot_of(iv, "GetQueryUGCResult")))(ugc, q, 0, d) & 0xff);
    uint64_t fileid, owner, hfile;
    uint32_t created, up, down, children;
    int32_t fsize;
    float score;
    memcpy(&fileid, d, 8); memcpy(&owner, d + 8160, 8); memcpy(&created, d + 8168, 4); memcpy(&hfile, d + 9216, 8);
    memcpy(&fsize, d + 9492, 4); memcpy(&up, d + 9756, 4); memcpy(&down, d + 9760, 4); memcpy(&score, d + 9764, 4); memcpy(&children, d + 9768, 4);
    CHECK(fileid == 777 && owner == 76561198000000042ULL && created == 1700000000u && hfile == 555 && fsize == 4096 && up == 30 && down == 2 && score == 0.9f && children == 2);
    CHECK(!strcmp((char *)d + 24, "Magic Castle") && !strcmp((char *)d + 153, "A castle mod") && !strcmp((char *)d + 8187, "mod,fun") && !strcmp((char *)d + 9232, "castle.zip"));
    CHECK(((Fn_uq_n)vt(ugc, slot_of(iv, "GetQueryUGCNumTags")))(ugc, q, 0) == 2);
    char text[64];
    CHECK(((Fn_uq_str)vt(ugc, slot_of(iv, "GetQueryUGCTag")))(ugc, q, 0, 1, text, sizeof text) & 0xff);
    CHECK(!strcmp(text, "fun"));
    CHECK(((Fn_uq_str)vt(ugc, slot_of(iv, "GetQueryUGCTagDisplayName")))(ugc, q, 0, 0, text, sizeof text) & 0xff);
    CHECK(!strcmp(text, "Mod"));
    CHECK(((Fn_uq_url)vt(ugc, slot_of(iv, "GetQueryUGCPreviewURL")))(ugc, q, 0, text, sizeof text) & 0xff);
    CHECK(!strcmp(text, "http://p/1.png"));
    CHECK(((Fn_uq_url)vt(ugc, slot_of(iv, "GetQueryUGCMetadata")))(ugc, q, 0, text, sizeof text) & 0xff);
    CHECK(!strcmp(text, "meta!"));
    uint64_t kids[4] = {0};
    CHECK(((Fn_uq_ch)vt(ugc, slot_of(iv, "GetQueryUGCChildren")))(ugc, q, 0, kids, 4) & 0xff);
    CHECK(kids[0] == 11 && kids[1] == 12);
    uint64_t stat = 0;
    CHECK(((Fn_uq_stat)vt(ugc, slot_of(iv, "GetQueryUGCStatistic")))(ugc, q, 0, 0, &stat) & 0xff);
    CHECK(stat == 304);
    CHECK(((Fn_uq_kv)vt(ugc, slot_of(iv, "GetQueryUGCKeyValueTag_2")))(ugc, q, 0, "version", text, sizeof text) & 0xff);
    CHECK(!strcmp(text, "1.2"));
    CHECK(!(((Fn_uq_res)vt(ugc, slot_of(iv, "GetQueryUGCResult")))(ugc, q, 5, d) & 0xff));
    CHECK(((Fn_uq_h)vt(ugc, slot_of(iv, "ReleaseQueryUGCRequest")))(ugc, q) & 0xff);
    uint64_t ids[2] = { 777, 778 };
    uint64_t qd = ((Fn_uq_ids)vt(ugc, slot_of(iv, "CreateQueryUGCDetailsRequest")))(ugc, ids, 2);
    CHECK(qd != ~0ULL);
    call = ((Fn_uq_h)vt(ugc, slot_of(iv, "SendQueryUGCRequest")))(ugc, qd);
    CHECK(call_taken(call, 3401, done, sizeof done));
    memcpy(&num, done + 12, 4);
    CHECK(num == 2);
    uint64_t qu = ((Fn_uq_user)vt(ugc, slot_of(iv, "CreateQueryUserUGCRequest")))(ugc, 0, 6, 0, 0, 480, 480, 1);
    call = ((Fn_uq_h)vt(ugc, slot_of(iv, "SendQueryUGCRequest")))(ugc, qu);
    CHECK(call_taken(call, 3401, done, sizeof done));
    memcpy(&num, done + 12, 4);
    CHECK(num == 1);
    uint64_t qv = ((Fn_uq_user)vt(ugc, slot_of(iv, "CreateQueryUserUGCRequest")))(ugc, 0, 2, 0, 0, 480, 480, 1);
    call = ((Fn_uq_h)vt(ugc, slot_of(iv, "SendQueryUGCRequest")))(ugc, qv);
    CHECK(call_taken(call, 3401, done, sizeof done));
    memcpy(&result, done + 8, 4); memcpy(&num, done + 12, 4);
    CHECK(result == 1 && num == 0);
    const char *iv20 = "STEAMUGC_INTERFACE_VERSION020";
    void *ugc20 = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamUGC")))(client, 1, 1, iv20);
    CHECK(ugc20);
    uint64_t q20 = ((Fn_uq_ids)vt(ugc20, slot_of(iv20, "CreateQueryUGCDetailsRequest")))(ugc20, ids, 2);
    call = ((Fn_uq_h)vt(ugc20, slot_of(iv20, "SendQueryUGCRequest")))(ugc20, q20);
    CHECK(call_taken(call, 3401, done, sizeof done));
    memset(d, 0xAB, sizeof d);
    CHECK(((Fn_uq_res)vt(ugc20, slot_of(iv20, "GetQueryUGCResult")))(ugc20, q20, 0, d) & 0xff);
    uint64_t total_size;
    memcpy(&total_size, d + 9776, 8);
    CHECK(total_size == 4096);
    return 1;
}

static const void *find_bytes(const void *h, size_t hn, const void *n, size_t nn) {
    const uint8_t *p = h;
    for (size_t i = 0; i + nn <= hn; i++) if (!memcmp(p + i, n, nn)) return p + i;
    return NULL;
}

static int t_gameserver(void) {
    shim_init();
    const char *iv = "SteamGameServer013";
    void *client = create("SteamClient023");
    CHECK(client);
    void *gs = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamGameServer")))(client, 1, 1, iv);
    CHECK(gs);
    CHECK(((Fn_gs_init)vt(gs, slot_of(iv, "InitGameServer")))(gs, 0x7F000001u, 27015, 27016, 3, 480, "1.0.0.0") & 0xff);
    ((Fn_gs_str)vt(gs, slot_of(iv, "SetServerName")))(gs, "Test Server GS");
    ((Fn_gs_str)vt(gs, slot_of(iv, "SetMapName")))(gs, "map_one");
    ((Fn_gs_str)vt(gs, slot_of(iv, "SetModDir")))(gs, "spacewar");
    ((Fn_gs_str)vt(gs, slot_of(iv, "SetGameDescription")))(gs, "Spacewar");
    ((Fn_gs_str)vt(gs, slot_of(iv, "SetGameTags")))(gs, "ffa,test");
    ((Fn_gs_int)vt(gs, slot_of(iv, "SetMaxPlayerCount")))(gs, 8);
    ((Fn_gs_kv)vt(gs, slot_of(iv, "SetKeyValue")))(gs, "mp_friendlyfire", "0");
    ((Fn_gs_kv)vt(gs, slot_of(iv, "SetKeyValue")))(gs, "sv_cheats", "0");
    CHECK(((Fn_gs_upd)vt(gs, slot_of(iv, "BUpdateUserData")))(gs, 76561198000000042ULL, "Ana", 7) & 0xff);
    CHECK(!(((Fn_gs_b)vt(gs, slot_of(iv, "BLoggedOn")))(gs) & 0xff));
    ((Fn_gs_v)vt(gs, slot_of(iv, "LogOnAnonymous")))(gs);
    uint8_t cb[16];
    CHECK(wait_callback(101, cb, sizeof cb) && wait_callback(115, cb, sizeof cb));
    CHECK(((Fn_gs_b)vt(gs, slot_of(iv, "BLoggedOn")))(gs) & 0xff);
    uint64_t sid = 0;
    ((Fn_gs_sid)vt(gs, slot_of(iv, "GetSteamID")))(gs, &sid);
    CHECK(sid == 90297557588031489ULL);
    uint8_t info_q[25] = { 0xFF, 0xFF, 0xFF, 0xFF, 'T' };
    memcpy(info_q + 5, "Source Engine Query", 20);
    CHECK(((Fn_gs_pkt)vt(gs, slot_of(iv, "HandleIncomingPacket")))(gs, info_q, sizeof info_q, 0x01020304u, 5555) & 0xff);
    uint8_t pkt[1400];
    uint32_t pip = 0;
    uint16_t pport = 0;
    int n = ((Fn_gs_out)vt(gs, slot_of(iv, "GetNextOutgoingPacket")))(gs, pkt, sizeof pkt, &pip, &pport);
    CHECK(n > 10 && pip == 0x01020304u && pport == 5555 && pkt[4] == 'I');
    CHECK(!strcmp((char *)pkt + 6, "Test Server GS") && find_bytes(pkt, (size_t)n, "map_one", 7) && find_bytes(pkt, (size_t)n, "ffa,test", 8));
    CHECK(((Fn_gs_out)vt(gs, slot_of(iv, "GetNextOutgoingPacket")))(gs, pkt, sizeof pkt, &pip, &pport) == 0);
    uint8_t pl_q[9] = { 0xFF, 0xFF, 0xFF, 0xFF, 'U', 0xFF, 0xFF, 0xFF, 0xFF };
    CHECK(((Fn_gs_pkt)vt(gs, slot_of(iv, "HandleIncomingPacket")))(gs, pl_q, sizeof pl_q, 0x01020304u, 5555) & 0xff);
    n = ((Fn_gs_out)vt(gs, slot_of(iv, "GetNextOutgoingPacket")))(gs, pkt, sizeof pkt, &pip, &pport);
    CHECK(n == 9 && pkt[4] == 'A');
    memcpy(pl_q + 5, pkt + 5, 4);
    CHECK(((Fn_gs_pkt)vt(gs, slot_of(iv, "HandleIncomingPacket")))(gs, pl_q, sizeof pl_q, 0x01020304u, 5555) & 0xff);
    n = ((Fn_gs_out)vt(gs, slot_of(iv, "GetNextOutgoingPacket")))(gs, pkt, sizeof pkt, &pip, &pport);
    CHECK(n > 10 && pkt[4] == 'D' && pkt[5] == 1 && !strcmp((char *)pkt + 7, "Ana"));
    uint8_t ru_q[9] = { 0xFF, 0xFF, 0xFF, 0xFF, 'V', 0xFF, 0xFF, 0xFF, 0xFF };
    CHECK(((Fn_gs_pkt)vt(gs, slot_of(iv, "HandleIncomingPacket")))(gs, ru_q, sizeof ru_q, 0x01020304u, 5555) & 0xff);
    n = ((Fn_gs_out)vt(gs, slot_of(iv, "GetNextOutgoingPacket")))(gs, pkt, sizeof pkt, &pip, &pport);
    memcpy(ru_q + 5, pkt + 5, 4);
    CHECK(((Fn_gs_pkt)vt(gs, slot_of(iv, "HandleIncomingPacket")))(gs, ru_q, sizeof ru_q, 0x01020304u, 5555) & 0xff);
    n = ((Fn_gs_out)vt(gs, slot_of(iv, "GetNextOutgoingPacket")))(gs, pkt, sizeof pkt, &pip, &pport);
    uint16_t rules;
    memcpy(&rules, pkt + 5, 2);
    CHECK(pkt[4] == 'E' && rules == 2 && !strcmp((char *)pkt + 7, "mp_friendlyfire"));
    uint8_t game_own[8] = { 1, 2, 3, 4, 5, 6, 7, 8 };
    CHECK(!(((Fn_gs_pkt)vt(gs, slot_of(iv, "HandleIncomingPacket")))(gs, game_own, sizeof game_own, 0x01020304u, 5555) & 0xff));
    CHECK(((Fn_gs_lic)vt(gs, slot_of(iv, "UserHasLicenseForApp")))(gs, 76561198000000042ULL, 480) == 2);
    ((Fn_gs_v)vt(gs, slot_of(iv, "LogOff")))(gs);
    CHECK(wait_callback(103, cb, sizeof cb));
    CHECK(!(((Fn_gs_b)vt(gs, slot_of(iv, "BLoggedOn")))(gs) & 0xff));
    return 1;
}

static int t_webticket(void) {
    shim_init();
    const char *iv = "SteamUser023";
    void *client = create("SteamClient023");
    CHECK(client);
    void *u = ((Fn_p_iip)vt(client, 5))(client, 1, 1, iv);
    CHECK(u);
    uint32_t h = ((Fn_web_ticket)vt(u, slot_of(iv, "GetAuthTicketForWebApi")))(u, "my-service");
    CHECK(h != 0);
    static uint8_t cb[4096];
    CHECK(wait_callback(168, cb, sizeof cb));
    uint32_t got;
    int32_t result, n;
    memcpy(&got, cb, 4); memcpy(&result, cb + 4, 4); memcpy(&n, cb + 8, 4);
    CHECK(got == h && result == 1 && n == 18 && !memcmp(cb + 12, "TICKET-FROM-BRIDGE", 18));
    CHECK(((Fn_web_ticket)vt(u, slot_of(iv, "GetAuthTicketForWebApi")))(u, NULL) != 0);
    return 1;
}

static int t_livebuy(void) {
    shim_init();
    const char *iv = "STEAMINVENTORY_INTERFACE_V003";
    void *client = create("SteamClient023");
    void *inv = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamInventory")))(client, 1, 1, iv);
    CHECK(inv);
    uint8_t cb[64];
    CHECK(((Fn_inv_load)vt(inv, slot_of(iv, "LoadItemDefinitions")))(inv) & 0xff);
    CHECK(wait_callback(4702, cb, sizeof cb));
    uint64_t pc = ((Fn_inv_prices)vt(inv, slot_of(iv, "RequestPrices")))(inv);
    CHECK(call_taken(pc, 4705, cb, sizeof cb));
    printf("live: account currency=%s items with price=%u\n", (char *)cb + 4, ((Fn_inv_nprices)vt(inv, slot_of(iv, "GetNumItemsWithPrices")))(inv));
    int32_t defs[64];
    uint64_t prices[64], base[64];
    uint32_t n = ((Fn_inv_nprices)vt(inv, slot_of(iv, "GetNumItemsWithPrices")))(inv);
    if (n > 64) n = 64;
    CHECK(((Fn_inv_prices_list)vt(inv, slot_of(iv, "GetItemsWithPrices")))(inv, defs, prices, base, n) & 0xff);
    for (uint32_t i = 0; i < n; i++) {
        char name[128] = "";
        uint32_t sz = sizeof name;
        ((Fn_inv_defprop)vt(inv, slot_of(iv, "GetItemDefinitionProperty")))(inv, defs[i], "name", name, &sz);
        printf("live: def %d \"%s\" price %llu cents\n", defs[i], name, (unsigned long long)prices[i]);
    }
    const char *buy = getenv("LIVE_BUY");
    if (buy && atoi(buy) > 0) {
        int32_t d[1] = { atoi(buy) };
        uint32_t q[1] = { 1 };
        uint64_t sc = ((Fn_inv_buy)vt(inv, slot_of(iv, "StartPurchase")))(inv, d, q, 1);
        uint8_t sr[24];
        CHECK(call_taken(sc, 4704, sr, sizeof sr));
        int32_t sres;
        uint64_t order, trans;
        memcpy(&sres, sr, 4); memcpy(&order, sr + 8, 8); memcpy(&trans, sr + 16, 8);
        printf("live: purchase started result=%d order=%llu transaction=%llu\n", sres, (unsigned long long)order, (unsigned long long)trans);
        CHECK(sres == 1);
        printf("live: waiting for the payment approval (up to 4 min)...\n");
        fflush(stdout);
        uint8_t rr[16];
        for (int i = 0; i < 240; i++) {
            ShimCallbackMsg m;
            bool got = false;
            while (shim_next_callback(&m)) { if (m.id == 4700) got = true; shim_free_last_callback(); }
            if (got) { printf("live: purchase completed, items received\n"); return 1; }
            Sleep(1000);
        }
        (void)rr;
        printf("live: no approval within the time limit\n");
    }
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_p2p_send)(void *, uint64_t, const void *, uint32_t, int, int);
typedef uint64_t (SHIM_TC *Fn_p2p_avail)(void *, uint32_t *, int);
typedef uint64_t (SHIM_TC *Fn_p2p_read)(void *, void *, uint32_t, uint32_t *, uint64_t *, int);
typedef uint64_t (SHIM_TC *Fn_p2p_accept)(void *, uint64_t);
typedef uint64_t (SHIM_TC *Fn_p2p_state)(void *, uint64_t, uint8_t *);

static int p2p_wait_packet(void *nw, const char *iv, int channel, void *buf, uint32_t cap, uint32_t *n, uint64_t *from) {
    for (int i = 0; i < 400; i++) {
        ShimCallbackMsg m;
        while (shim_next_callback(&m)) shim_free_last_callback();
        uint32_t sz = 0;
        if (((Fn_p2p_avail)vt(nw, slot_of(iv, "IsP2PPacketAvailable")))(nw, &sz, channel) & 0xff)
            return ((Fn_p2p_read)vt(nw, slot_of(iv, "ReadP2PPacket")))(nw, buf, cap, n, from, channel) & 0xff;
        Sleep(50);
    }
    return 0;
}

static int t_p2p(bool initiator) {
    shim_init();
    const char *iv = "SteamNetworking006";
    uint64_t me = shim_snapshot()->steamid, peer = initiator ? 76561198000000222ULL : 76561198000000111ULL;
    void *client = create("SteamClient023");
    void *nw = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamNetworking")))(client, 1, 1, iv);
    CHECK(nw);
    uint8_t buf[256];
    uint32_t n = 0;
    uint64_t from = 0;
    printf("p2p: eu=%llu\n", (unsigned long long)me);
    if (initiator) {
        CHECK(((Fn_p2p_send)vt(nw, slot_of(iv, "SendP2PPacket")))(nw, peer, "ping-1", 7, 2, 0) & 0xff);
        CHECK(((Fn_p2p_send)vt(nw, slot_of(iv, "SendP2PPacket")))(nw, peer, "alpha", 6, 0, 5) & 0xff);
        CHECK(p2p_wait_packet(nw, iv, 0, buf, sizeof buf, &n, &from));
        CHECK(from == peer && n == 7 && !memcmp(buf, "pong-1", 7));
        uint8_t st[20];
        CHECK(((Fn_p2p_state)vt(nw, slot_of(iv, "GetP2PSessionState")))(nw, peer, st) & 0xff);
        CHECK(st[0] == 1);
        for (int i = 0; i < 30; i++) {
            char msg[16];
            snprintf(msg, sizeof msg, "seq-%02d", i);
            CHECK(((Fn_p2p_send)vt(nw, slot_of(iv, "SendP2PPacket")))(nw, peer, msg, 7, 2, 1) & 0xff);
        }
        CHECK(p2p_wait_packet(nw, iv, 2, buf, sizeof buf, &n, &from) && !memcmp(buf, "done", 5));
        return 1;
    }
    uint8_t cb[16];
    int got = 0;
    for (int k = 0; k < 8 && !got; k++) got = wait_callback(1202, cb, sizeof cb);
    CHECK(got);
    uint64_t asker;
    memcpy(&asker, cb, 8);
    CHECK(asker == peer);
    CHECK(((Fn_p2p_accept)vt(nw, slot_of(iv, "AcceptP2PSessionWithUser")))(nw, asker) & 0xff);
    CHECK(p2p_wait_packet(nw, iv, 0, buf, sizeof buf, &n, &from));
    CHECK(from == peer && n == 7 && !memcmp(buf, "ping-1", 7));
    CHECK(p2p_wait_packet(nw, iv, 5, buf, sizeof buf, &n, &from) && !memcmp(buf, "alpha", 6));
    CHECK(((Fn_p2p_send)vt(nw, slot_of(iv, "SendP2PPacket")))(nw, peer, "pong-1", 7, 2, 0) & 0xff);
    for (int i = 0; i < 30; i++) {
        char want[16];
        snprintf(want, sizeof want, "seq-%02d", i);
        CHECK(p2p_wait_packet(nw, iv, 1, buf, sizeof buf, &n, &from));
        CHECK(!memcmp(buf, want, 7));
    }
    CHECK(((Fn_p2p_send)vt(nw, slot_of(iv, "SendP2PPacket")))(nw, peer, "done", 5, 2, 2) & 0xff);
    Sleep(800);
    return 1;
}
static int t_p2p_a(void) { return t_p2p(true); }
static int t_p2p_b(void) { return t_p2p(false); }

typedef int64_t t_i64a __attribute__((aligned(8)));
typedef struct TMsg { void *data; int size; uint32_t conn; uint8_t identity[136]; t_i64a conn_user, time_received, number; void *free_data; void (*release)(struct TMsg *); int channel, flags; t_i64a user_data; uint16_t lane, pad; } TMsg;
typedef uint32_t (SHIM_TC *Fn_s_listen)(void *, int, int, const void *);
typedef uint32_t (SHIM_TC *Fn_s_connect)(void *, const void *, int, int, const void *);
typedef int (SHIM_TC *Fn_s_accept)(void *, uint32_t);
typedef int (SHIM_TC *Fn_s_send)(void *, uint32_t, const void *, uint32_t, int, int64_t *);
typedef int (SHIM_TC *Fn_s_recv)(void *, uint32_t, TMsg **, int);
typedef uint32_t (SHIM_TC *Fn_s_mkgroup)(void *);
typedef uint64_t (SHIM_TC *Fn_s_setgroup)(void *, uint32_t, uint32_t);
typedef uint64_t (SHIM_TC *Fn_s_close)(void *, uint32_t, int, const char *, uint8_t);
typedef uint64_t (SHIM_TC *Fn_s_info)(void *, uint32_t, uint8_t *);
typedef uint64_t (SHIM_TC *Fn_s_pair)(void *, uint32_t *, uint32_t *, uint8_t, const void *, const void *);
typedef int (SHIM_TC *Fn_m_send)(void *, const void *, const void *, uint32_t, int, int);
typedef int (SHIM_TC *Fn_m_recv)(void *, int, TMsg **, int);
typedef uint64_t (SHIM_TC *Fn_m_accept)(void *, const void *);
typedef TMsg *(SHIM_TC *Fn_nu_alloc)(void *, int);

static void make_identity(uint8_t *id, uint64_t sid) { memset(id, 0, 136); int t = 16, n = 8; memcpy(id, &t, 4); memcpy(id + 4, &n, 4); memcpy(id + 8, &sid, 8); }

static uint32_t wait_status(uint32_t want_conn, int want_state, uint8_t *cb) {
    for (int k = 0; k < 40; k++) {
        if (!wait_callback(1221, cb, 712)) continue;
        uint32_t conn;
        int st;
        memcpy(&conn, cb, 4);
        memcpy(&st, cb + 8 + 176, 4);
        if (st == want_state && (!want_conn || conn == want_conn)) return conn;
    }
    return 0;
}

static int sock_wait_msg(void *ns, const char *iv, uint32_t conn, uint32_t group, TMsg **out) {
    for (int i = 0; i < 400; i++) {
        int n = group ? ((Fn_s_recv)vt(ns, slot_of(iv, "ReceiveMessagesOnPollGroup")))(ns, group, out, 1)
                      : ((Fn_s_recv)vt(ns, slot_of(iv, "ReceiveMessagesOnConnection")))(ns, conn, out, 1);
        if (n > 0) return 1;
        Sleep(50);
    }
    return 0;
}

static int t_nsockets(bool initiator) {
    shim_init();
    const char *iv = "SteamNetworkingSockets012";
    void *client = create("SteamClient023");
    void *ns = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, iv);
    CHECK(ns);
    uint8_t cb[712], id[136], info[696];
    uint8_t big[5000];
    for (int i = 0; i < (int)sizeof big; i++) big[i] = (uint8_t)(i * 7 + 3);
    TMsg *m = NULL;
    if (initiator) {
        make_identity(id, 76561198000000222ULL);
        uint32_t c = ((Fn_s_connect)vt(ns, slot_of(iv, "ConnectP2P")))(ns, id, 7, 0, NULL);
        CHECK(c);
        CHECK(wait_status(c, 3, cb) == c);
        CHECK(((Fn_s_info)vt(ns, slot_of(iv, "GetConnectionInfo")))(ns, c, info) & 0xff);
        int st;
        memcpy(&st, info + 176, 4);
        CHECK(st == 3);
        int64_t no = 0;
        CHECK(((Fn_s_send)vt(ns, slot_of(iv, "SendMessageToConnection")))(ns, c, big, sizeof big, 8, &no) == 1 && no == 1);
        for (int i = 0; i < 20; i++) {
            char msg[16];
            snprintf(msg, sizeof msg, "small-%02d", i);
            CHECK(((Fn_s_send)vt(ns, slot_of(iv, "SendMessageToConnection")))(ns, c, msg, 9, 8, NULL) == 1);
        }
        CHECK(sock_wait_msg(ns, iv, c, 0, &m));
        CHECK(m->size == 5 && !memcmp(m->data, "echo!", 5) && m->conn == c);
        m->release(m);
        CHECK(((Fn_s_close)vt(ns, slot_of(iv, "CloseConnection")))(ns, c, 1000, "end", 0) & 0xff);
        CHECK(((Fn_s_send)vt(ns, slot_of(iv, "SendMessageToConnection")))(ns, c, "x", 1, 8, NULL) != 1);
        Sleep(800);
        return 1;
    }
    uint32_t lis = ((Fn_s_listen)vt(ns, slot_of(iv, "CreateListenSocketP2P")))(ns, 7, 0, NULL);
    CHECK(lis);
    uint32_t grp = ((Fn_s_mkgroup)vt(ns, slot_of(iv, "CreatePollGroup")))(ns);
    CHECK(grp);
    uint32_t c = wait_status(0, 1, cb);
    CHECK(c);
    uint32_t l;
    memcpy(&l, cb + 8 + 144, 4);
    CHECK(l == lis);
    CHECK(((Fn_s_accept)vt(ns, slot_of(iv, "AcceptConnection")))(ns, c) == 1);
    CHECK(((Fn_s_setgroup)vt(ns, slot_of(iv, "SetConnectionPollGroup")))(ns, c, grp) & 0xff);
    CHECK(sock_wait_msg(ns, iv, c, grp, &m));
    CHECK(m->size == (int)sizeof big && !memcmp(m->data, big, sizeof big));
    m->release(m);
    for (int i = 0; i < 20; i++) {
        char want[16];
        snprintf(want, sizeof want, "small-%02d", i);
        CHECK(sock_wait_msg(ns, iv, c, grp, &m));
        CHECK(m->size == 9 && !memcmp(m->data, want, 9) && m->number == i + 2);
        m->release(m);
    }
    CHECK(((Fn_s_send)vt(ns, slot_of(iv, "SendMessageToConnection")))(ns, c, "echo!", 5, 8, NULL) == 1);
    CHECK(wait_status(c, 4, cb) == c);
    int reason;
    memcpy(&reason, cb + 8 + 180, 4);
    CHECK(reason == 1000);
    CHECK(((Fn_s_close)vt(ns, slot_of(iv, "CloseConnection")))(ns, c, 0, NULL, 0) & 0xff);
    return 1;
}
static int t_sockets_a(void) { return t_nsockets(true); }
static int t_sockets_b(void) { return t_nsockets(false); }

static int t_messages(bool initiator) {
    shim_init();
    const char *iv = "SteamNetworkingMessages002";
    void *client = create("SteamClient023");
    void *nm = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, iv);
    CHECK(nm);
    uint8_t id[136], cb[136];
    uint8_t big[3000];
    for (int i = 0; i < (int)sizeof big; i++) big[i] = (uint8_t)(i * 5 + 1);
    TMsg *m = NULL;
    if (initiator) {
        make_identity(id, 76561198000000222ULL);
        CHECK(((Fn_m_send)vt(nm, slot_of(iv, "SendMessageToUser")))(nm, id, "hello", 6, 8, 3) == 1);
        CHECK(((Fn_m_send)vt(nm, slot_of(iv, "SendMessageToUser")))(nm, id, big, sizeof big, 8, 3) == 1);
        int got = 0;
        for (int i = 0; i < 400 && !got; i++) { got = ((Fn_m_recv)vt(nm, slot_of(iv, "ReceiveMessagesOnChannel")))(nm, 4, &m, 1); if (!got) Sleep(50); }
        CHECK(got == 1 && m->size == 6 && !memcmp(m->data, "reply", 6) && m->channel == 4);
        uint64_t from;
        memcpy(&from, m->identity + 8, 8);
        CHECK(from == 76561198000000222ULL);
        m->release(m);
        Sleep(500);
        return 1;
    }
    int ok = 0;
    for (int k = 0; k < 8 && !ok; k++) ok = wait_callback(1251, cb, sizeof cb);
    CHECK(ok);
    uint64_t asker;
    memcpy(&asker, cb + 8, 8);
    CHECK(asker == 76561198000000111ULL);
    CHECK(((Fn_m_accept)vt(nm, slot_of(iv, "AcceptSessionWithUser")))(nm, cb) & 0xff);
    int got = 0;
    for (int i = 0; i < 400 && !got; i++) { got = ((Fn_m_recv)vt(nm, slot_of(iv, "ReceiveMessagesOnChannel")))(nm, 3, &m, 1); if (!got) Sleep(50); }
    CHECK(got == 1 && m->size == 6 && !memcmp(m->data, "hello", 6) && m->channel == 3);
    m->release(m);
    got = 0;
    for (int i = 0; i < 400 && !got; i++) { got = ((Fn_m_recv)vt(nm, slot_of(iv, "ReceiveMessagesOnChannel")))(nm, 3, &m, 1); if (!got) Sleep(50); }
    CHECK(got == 1 && m->size == (int)sizeof big && !memcmp(m->data, big, sizeof big));
    m->release(m);
    make_identity(id, asker);
    CHECK(((Fn_m_send)vt(nm, slot_of(iv, "SendMessageToUser")))(nm, id, "reply", 6, 8, 4) == 1);
    Sleep(800);
    return 1;
}
static int t_messages_a(void) { return t_messages(true); }
static int t_messages_b(void) { return t_messages(false); }

typedef uint64_t (SHIM_TC *Fn_s_addr)(void *, uint32_t, uint8_t *);
typedef uint32_t (SHIM_TC *Fn_s_cip)(void *, const void *, int, const void *);

static int t_ipsockets(void) {
    shim_init();
    const char *iv = "SteamNetworkingSockets012";
    void *client = create("SteamClient023");
    void *ns = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, iv);
    CHECK(ns);
    uint8_t addr[18] = {0}, cb[712];
    uint32_t lis = ((Fn_ns_listen)vt(ns, slot_of(iv, "CreateListenSocketIP")))(ns, addr, 0, NULL);
    CHECK(lis);
    CHECK(((Fn_s_addr)vt(ns, slot_of(iv, "GetListenSocketAddress")))(ns, lis, addr) & 0xff);
    uint16_t port;
    memcpy(&port, addr + 16, 2);
    CHECK(port != 0);
    memset(addr, 0, 16);
    addr[10] = addr[11] = 0xFF; addr[12] = 127; addr[15] = 1;
    uint32_t c = ((Fn_s_cip)vt(ns, slot_of(iv, "ConnectByIPAddress")))(ns, addr, 0, NULL);
    CHECK(c);
    uint32_t srv = wait_status(0, 1, cb);
    if (srv == c) srv = wait_status(0, 1, cb);
    CHECK(srv && srv != c);
    uint32_t l;
    memcpy(&l, cb + 8 + 144, 4);
    CHECK(l == lis);
    CHECK(((Fn_s_accept)vt(ns, slot_of(iv, "AcceptConnection")))(ns, srv) == 1);
    CHECK(wait_status(c, 3, cb) == c);
    CHECK(((Fn_s_send)vt(ns, slot_of(iv, "SendMessageToConnection")))(ns, c, "hello", 6, 8, NULL) == 1);
    TMsg *m = NULL;
    CHECK(sock_wait_msg(ns, iv, srv, 0, &m));
    CHECK(m->size == 6 && !memcmp(m->data, "hello", 6));
    m->release(m);
    CHECK(((Fn_s_send)vt(ns, slot_of(iv, "SendMessageToConnection")))(ns, srv, "back", 5, 8, NULL) == 1);
    CHECK(sock_wait_msg(ns, iv, c, 0, &m));
    CHECK(m->size == 5 && !memcmp(m->data, "back", 5));
    m->release(m);
    CHECK(((Fn_s_close)vt(ns, slot_of(iv, "CloseConnection")))(ns, c, 1000, NULL, 0) & 0xff);
    CHECK(wait_status(srv, 4, cb) == srv);
    CHECK(((Fn_s_close)vt(ns, slot_of(iv, "CloseListenSocket")))(ns, lis, 0, NULL, 0) & 0xff);
    return 1;
}

static int t_socketpair(void) {
    shim_init();
    const char *iv = "SteamNetworkingSockets012";
    void *client = create("SteamClient023");
    void *ns = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, iv);
    CHECK(ns);
    uint32_t a = 0, b = 0;
    CHECK(((Fn_s_pair)vt(ns, slot_of(iv, "CreateSocketPair")))(ns, &a, &b, 1, NULL, NULL) & 0xff);
    CHECK(a && b && a != b);
    CHECK(((Fn_s_send)vt(ns, slot_of(iv, "SendMessageToConnection")))(ns, a, "ping", 5, 8, NULL) == 1);
    TMsg *m = NULL;
    CHECK(((Fn_s_recv)vt(ns, slot_of(iv, "ReceiveMessagesOnConnection")))(ns, b, &m, 4) == 1);
    CHECK(m->size == 5 && !memcmp(m->data, "ping", 5) && m->conn == b);
    m->release(m);
    CHECK(((Fn_s_recv)vt(ns, slot_of(iv, "ReceiveMessagesOnConnection")))(ns, b, &m, 4) == 0);
    uint8_t info[696];
    CHECK(((Fn_s_info)vt(ns, slot_of(iv, "GetConnectionInfo")))(ns, a, info) & 0xff);
    int st;
    memcpy(&st, info + 176, 4);
    CHECK(st == 3);
    CHECK(((Fn_s_close)vt(ns, slot_of(iv, "CloseConnection")))(ns, a, 5, NULL, 0) & 0xff);
    CHECK(((Fn_s_info)vt(ns, slot_of(iv, "GetConnectionInfo")))(ns, b, info) & 0xff);
    memcpy(&st, info + 176, 4);
    CHECK(st == 4);
    CHECK(!(((Fn_s_info)vt(ns, slot_of(iv, "GetConnectionInfo")))(ns, a, info) & 0xff));
        const char *uv = "SteamNetworkingUtils003";
    void *nu = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, uv);
    CHECK(nu);
    TMsg *am = ((Fn_nu_alloc)vt(nu, slot_of(uv, "AllocateMessage")))(nu, 64);
    CHECK(am && am->size == 64 && am->data);
    am->release(am);
    return 1;
}

typedef void (SHIM_TC *Fn_ov_s)(void *, const char *);
typedef void (SHIM_TC *Fn_ov_web)(void *, const char *, int);
typedef void (SHIM_TC *Fn_ov_store)(void *, uint32_t, int);
typedef void (SHIM_TC *Fn_ov_user)(void *, const char *, uint64_t);
typedef void (SHIM_TC *Fn_ov_lobby)(void *, uint64_t);
typedef uint8_t (SHIM_TC *Fn_ov_enabled)(void *);

static int t_overlay_requests(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    const char *fv = "SteamFriends017", *uv = "SteamUtils010";
    void *fr = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, fv);
    void *ut = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, uv);
    CHECK(fr && ut);
    CHECK(!(((Fn_ov_enabled)vt(ut, sl(uv, "IsOverlayEnabled")))(ut) & 0xff));
    ((Fn_ov_store)vt(fr, sl(fv, "ActivateGameOverlayToStore")))(fr, 1234, 0);
    static char url[5001];
    memset(url, 'a', 5000);
    ((Fn_ov_web)vt(fr, sl(fv, "ActivateGameOverlayToWebPage")))(fr, url, 0);
    ((Fn_ov_s)vt(fr, sl(fv, "ActivateGameOverlay")))(fr, "Achievements");
    ((Fn_ov_user)vt(fr, sl(fv, "ActivateGameOverlayToUser")))(fr, "chat", 76561198000000042ULL);
    ((Fn_ov_lobby)vt(fr, sl(fv, "ActivateGameOverlayInviteDialog")))(fr, 109775240000000001ULL);
    ((Fn_ov_s)vt(fr, sl(fv, "ActivateGameOverlayInviteDialogConnectString")))(fr, "+connect 1.2.3.4");
    ((Fn_ov_lobby)vt(fr, sl(fv, "ActivateGameOverlayRemotePlayTogetherInviteDialog")))(fr, 109775240000000002ULL);
    ((Fn_ov_lobby)vt(fr, sl(fv, "SetPlayedWith")))(fr, 76561198000000077ULL);
    CHECK(((int (SHIM_TC *)(void *))vt(fr, sl(fv, "GetCoplayFriendCount")))(fr) == 1);
    {
        uint64_t who = 0;
        ((uint64_t *(SHIM_TC *)(void *, uint64_t *, int))vt(fr, sl(fv, "GetCoplayFriend")))(fr, &who, 0);
        CHECK(who == 76561198000000077ULL);
        CHECK(((uint32_t (SHIM_TC *)(void *, uint64_t))vt(fr, sl(fv, "GetFriendCoplayTime")))(fr, 76561198000000077ULL) != 0);
        CHECK(((uint32_t (SHIM_TC *)(void *, uint64_t))vt(fr, sl(fv, "GetFriendCoplayTime")))(fr, 76561198000000099ULL) == 0);
    }
    ((Fn_ov_web)vt(fr, sl(fv, "ActivateGameOverlayToWebPage")))(fr, "https://example.test/modal", 1);
    {
        const char *ov = "SteamFriends013";
        void *old = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, ov);
        CHECK(old);
        ((void (SHIM_TC *)(void *, const char *))vt(old, sl(ov, "ActivateGameOverlayToWebPage")))(old, "https://example.test/old");
    }
    Sleep(600);
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_show_text)(void *, int, int, const char *, uint32_t, const char *);
typedef uint32_t (SHIM_TC *Fn_text_len)(void *);
typedef uint64_t (SHIM_TC *Fn_text_get)(void *, char *, uint32_t);

static int t_gamepad_text(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    const char *uv = "SteamUtils010";
    void *ut = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, uv);
    CHECK(ut);
    CHECK(((Fn_text_len)vt(ut, sl(uv, "GetEnteredGamepadTextLength")))(ut) == 1);
    CHECK(((Fn_show_text)vt(ut, sl(uv, "ShowGamepadTextInput")))(ut, 0, 1, "Your name", 16, "John") & 0xff);
    uint8_t cb[16] = {0};
    CHECK(wait_callback(714, cb, sizeof cb));
    uint32_t len;
    memcpy(&len, cb + 4, 4);
    CHECK(cb[0] == 1 && len == 4);
    CHECK(((Fn_text_len)vt(ut, sl(uv, "GetEnteredGamepadTextLength")))(ut) == 4);
    char text[16] = {0};
    CHECK(((Fn_text_get)vt(ut, sl(uv, "GetEnteredGamepadTextInput")))(ut, text, sizeof text) & 0xff);
    CHECK(!strcmp(text, "Ana"));
    return 1;
}

typedef int (SHIM_TC *Fn_i0)(void *);
typedef uint64_t *(SHIM_TC *Fn_coplay)(void *, uint64_t *, int);
typedef uint32_t (SHIM_TC *Fn_coplay_of)(void *, uint64_t);
typedef uint64_t (SHIM_TC *Fn_b0)(void *);
typedef const char *(SHIM_TC *Fn_change)(void *, int, int *, int *);

static int t_overlay_data(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    const char *gen = "GetISteamGenericInterface";
    const char *fv = "SteamFriends017", *mv = "SteamMatchMaking005", *rv = "STEAMREMOTESTORAGE_INTERFACE_VERSION016";
    void *fr = ((Fn_p_iip)vt(client, sl("SteamClient023", gen)))(client, 1, 1, fv);
    void *mm = ((Fn_p_iip)vt(client, sl("SteamClient023", gen)))(client, 1, 1, mv);
    void *rs = ((Fn_p_iip)vt(client, sl("SteamClient023", gen)))(client, 1, 1, rv);
    CHECK(fr && mm && rs);
    CHECK(((Fn_i0)vt(fr, sl(fv, "GetCoplayFriendCount")))(fr) == 2);
    uint64_t who = 0;
    ((Fn_coplay)vt(fr, sl(fv, "GetCoplayFriend")))(fr, &who, 0);
    CHECK(who == 76561198000000011ULL);
    CHECK(((Fn_coplay_of)vt(fr, sl(fv, "GetFriendCoplayGame")))(fr, 76561198000000011ULL) == 480);
    CHECK(((Fn_coplay_of)vt(fr, sl(fv, "GetFriendCoplayTime")))(fr, 76561198000000022ULL) == 1700000000u);
    CHECK(((Fn_coplay_of)vt(fr, sl(fv, "GetFriendCoplayGame")))(fr, 99) == 0);
    CHECK(((Fn_b0)vt(mm, sl(mv, "RequestFriendsLobbies")))(mm) & 0xff);
    Sleep(500);
    CHECK(((Fn_i0)vt(rs, sl(rv, "GetLocalFileChangeCount")))(rs) == 2);
    int change = 0, path = 0;
    const char *name = ((Fn_change)vt(rs, sl(rv, "GetLocalFileChange")))(rs, 1, &change, &path);
    CHECK(!strcmp(name, "profile/ana.cfg") && change == 1 && path == 2);
    change = path = 9;
    name = ((Fn_change)vt(rs, sl(rv, "GetLocalFileChange")))(rs, 5, &change, &path);
    CHECK(name && !*name && change == 9 && path == 9);
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_show_floating)(void *, int, int, int, int, int);

static WCHAR g_typed[32];
static int g_ntyped;

static LRESULT CALLBACK typed_proc(HWND w, UINT m, WPARAM wp, LPARAM lp) {
    if (m == WM_CHAR && g_ntyped < 31) g_typed[g_ntyped++] = (WCHAR)wp;
    return DefWindowProcW(w, m, wp, lp);
}

static int t_gamepad_floating(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    const char *uv = "SteamUtils010";
    void *ut = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, uv);
    CHECK(ut);
    WNDCLASSW wc = {0};
    wc.lpfnWndProc = typed_proc;
    wc.hInstance = GetModuleHandleW(NULL);
    wc.lpszClassName = L"SelftestGame";
    RegisterClassW(&wc);
    HWND win = CreateWindowW(L"SelftestGame", L"game", WS_OVERLAPPEDWINDOW | WS_VISIBLE, 0, 0, 200, 100, NULL, NULL, wc.hInstance, NULL);
    if (win) { SetForegroundWindow(win); SetFocus(win); }
    CHECK(((Fn_show_floating)vt(ut, sl(uv, "ShowFloatingGamepadTextInput")))(ut, 0, 10, 20, 300, 100) & 0xff);
    uint8_t cb[8] = {0};
    int dismissed = 0;
    for (int i = 0; i < 40 && !dismissed; i++) {
        MSG m;
        while (PeekMessageW(&m, NULL, 0, 0, PM_REMOVE)) { TranslateMessage(&m); DispatchMessageW(&m); }
        dismissed = wait_callback(738, cb, sizeof cb);
        if (!dismissed) Sleep(50);
    }
    CHECK(dismissed);
    for (int i = 0; i < 20; i++) { MSG m; while (PeekMessageW(&m, NULL, 0, 0, PM_REMOVE)) { TranslateMessage(&m); DispatchMessageW(&m); } Sleep(20); }
    if (win) CHECK(g_ntyped == 2 && g_typed[0] == L'A' && g_typed[1] == L'b');
    else printf("(no display: typing not checked)\n");
    return 1;
}

static int t_overlay_capable(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    const char *uv = "SteamUtils010";
    void *ut = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, uv);
    CHECK(ut);
    CHECK(((Fn_ov_enabled)vt(ut, sl(uv, "IsOverlayEnabled")))(ut) & 0xff);
    return 1;
}

static int t_overlay_shown(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    const char *fv = "SteamFriends017";
    void *fr = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, fv);
    CHECK(fr);
    ((Fn_ov_store)vt(fr, sl(fv, "ActivateGameOverlayToStore")))(fr, 1234, 0);
    uint8_t cb[16] = {0};
    CHECK(wait_callback(331, cb, sizeof cb));
    CHECK(cb[0] == 1);
    CHECK(wait_callback(331, cb, sizeof cb));
    CHECK(cb[0] == 0);
    return 1;
}

static int t_overlay_join(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    const char *fv = "SteamFriends017";
    void *fr = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, fv);
    CHECK(fr);
    ((Fn_ov_store)vt(fr, sl(fv, "ActivateGameOverlayToStore")))(fr, 1234, 0);
    uint8_t cb[16] = {0};
    CHECK(wait_callback(333, cb, sizeof cb));
    uint64_t lobby, friend_id;
    memcpy(&lobby, cb, 8);
    memcpy(&friend_id, cb + 8, 8);
    CHECK(lobby == 109775240000000001ULL && friend_id == 76561198000000002ULL);
    return 1;
}

typedef uint32_t (SHIM_TC *Fn_dlc_installed)(void *, uint32_t);
typedef uint32_t (SHIM_TC *Fn_dlc_count)(void *);

static int t_dlc_updates(void) {
    shim_init();
    const char *iv = "STEAMAPPS_INTERFACE_VERSION008";
    void *client = create("SteamClient023");
    CHECK(client);
    void *apps = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, iv);
    CHECK(apps);
    CHECK(!(((Fn_dlc_installed)vt(apps, sl(iv, "BIsDlcInstalled")))(apps, 777) & 0xff));
    CHECK(((Fn_dlc_count)vt(apps, sl(iv, "GetDLCCount")))(apps) == 1);
    {
        uint32_t id = 0; uint8_t avail = 9; char name[64] = "";
        CHECK((((Fn_dlc)vt(apps, sl(iv, "BGetDLCDataByIndex")))(apps, 0, &id, &avail, name, sizeof name) & 0xff) == 1);
        CHECK(id == 777 && avail == 0 && !strcmp(name, "Extra Pack"));
    }
    uint8_t cb[8] = {0};
    CHECK(wait_callback(1005, cb, sizeof cb));
    uint32_t appid;
    memcpy(&appid, cb, 4);
    CHECK(appid == 777);
    CHECK(((Fn_dlc_installed)vt(apps, sl(iv, "BIsDlcInstalled")))(apps, 777) & 0xff);
    CHECK(((Fn_dlc_count)vt(apps, sl(iv, "GetDLCCount")))(apps) == 1);
    return 1;
}

static int t_microtxn_capture(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    void *user = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, "SteamUser020");
    CHECK(user);
    uint8_t cb[24] = {0};
    CHECK(wait_callback(152, cb, sizeof cb));
    uint32_t appid;
    uint64_t order;
    memcpy(&appid, cb, 4);
    memcpy(&order, cb + 8, 8);
    CHECK(appid == 2357570 && order == 506252350422904163ULL && cb[16] == 0);
    return 1;
}

static int t_offline(void) {
    shim_init();
    static uint8_t cb[64];
    CHECK(shim_session_online() == 0);
    CHECK(Steam_BLoggedOn(g_user, g_pipe) == 0 && Steam_BConnected(g_user, g_pipe) == 0);
    CHECK(wait_callback(102, cb, sizeof cb));
    int32_t result;
    memcpy(&result, cb, 4);
    CHECK(result == 3 && cb[4] == 1);
    CHECK(wait_callback(101, cb, sizeof cb));
    CHECK(shim_session_online() == 1);
    CHECK(Steam_BLoggedOn(g_user, g_pipe) == 1 && Steam_BConnected(g_user, g_pipe) == 1);
    return 1;
}

typedef uint32_t (SHIM_TC *Fn_u_u)(void *, uint32_t);
typedef uint32_t (SHIM_TC *Fn_license)(void *, uint64_t, uint32_t);
static int t_account_queries(void) {
    shim_init();
    const char *iv = "STEAMAPPS_INTERFACE_VERSION008";
    void *client = create("SteamClient023");
    CHECK(client);
    void *apps = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, iv);
    CHECK(apps);
    Fn_u_u earliest = (Fn_u_u)vt(apps, sl(iv, "GetEarliestPurchaseUnixTime"));
    CHECK(earliest(apps, 550) == 1300000000u);
    CHECK(earliest(apps, 440) == 1400000000u);
    CHECK(earliest(apps, 999) == 0);
    const char *uv = "SteamUser023";
    void *user = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, uv);
    CHECK(user);
    uint64_t me = 0;
    ((Fn_gs_sid)vt(user, sl(uv, "GetSteamID")))(user, &me);
    Fn_license lic = (Fn_license)vt(user, sl(uv, "UserHasLicenseForApp"));
    CHECK(lic(user, me, 2357570) == 0);
    CHECK(lic(user, me, 777) == 0);
    CHECK(lic(user, me, 888) == 1);
    CHECK(lic(user, 76561198000000042ULL, 480) == 2);
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_pf5)(void *, const char *, const char *, uint32_t, const char *, const char *, int, const ParamStrings *);
typedef uint64_t (SHIM_TC *Fn_pw5)(void *, const char *, const char *, uint32_t, const char *, const char *, const ParamStrings *);
typedef struct { uint64_t item; const char *file, *preview, *title, *desc; int32_t vis; const ParamStrings *tags; uint8_t uf, up, ut, ud, uv, utags; } UpdReq5;
typedef uint64_t (SHIM_TC *Fn_upd5)(void *, UpdReq5);
typedef uint64_t (SHIM_TC *Fn_video6)(void *, uint32_t, const char *, const char *, const char *, uint32_t, const char *, const char *);
typedef uint64_t (SHIM_TC *Fn_beta9)(void *, int32_t, uint32_t *, uint32_t *, char *, int32_t, char *, int32_t, uint32_t *);
static int t_legacy_signatures(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    const char *tags[2] = { "map", "coop" };
    ParamStrings arr = { tags, 2 };
    uint8_t res[24];
    int32_t result;
    uint64_t item;
    const char *r5 = "STEAMREMOTESTORAGE_INTERFACE_VERSION005";
    void *rs5 = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamRemoteStorage")))(client, 1, 1, r5);
    CHECK(rs5);
    uint64_t call = ((Fn_pf5)vt(rs5, slot_of(r5, "PublishFile")))(rs5, "save.dat", "cover.png", 480, "My file", "Description", 2, &arr);
    CHECK(call != 0 && call_taken(call, 1309, res, sizeof res));
    memcpy(&result, res, 4); memcpy(&item, res + 8, 8);
    CHECK(result == 1 && item == 777);
    call = ((Fn_pw5)vt(rs5, slot_of(r5, "PublishWorkshopFile")))(rs5, "mod.zip", "cover.png", 480, "My mod", "Desc", &arr);
    CHECK(call != 0 && call_taken(call, 1309, res, sizeof res));
    memcpy(&result, res, 4);
    CHECK(result == 1);
    UpdReq5 rq = { 777, "mod2.zip", NULL, "New", NULL, 0, NULL, 1, 0, 1, 0, 0, 0 };
    call = ((Fn_upd5)vt(rs5, slot_of(r5, "UpdatePublishedFile")))(rs5, rq);
    CHECK(call != 0 && call_taken(call, 1316, res, sizeof res));
    memcpy(&result, res, 4); memcpy(&item, res + 8, 8);
    CHECK(result == 1 && item == 777);
    const char *r6 = "STEAMREMOTESTORAGE_INTERFACE_VERSION006";
    void *rs6 = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamRemoteStorage")))(client, 1, 1, r6);
    CHECK(rs6);
    call = ((Fn_video6)vt(rs6, slot_of(r6, "PublishVideo")))(rs6, 1, "channel", "dQw4w9WgXcQ", "cover.png", 480, "Video", "Desc");
    CHECK(call != 0 && call_taken(call, 1309, res, sizeof res));
    memcpy(&result, res, 4); memcpy(&item, res + 8, 8);
    CHECK(result == 1 && item == 888);
    const char *a9 = "STEAMAPPS_INTERFACE_VERSION009";
    void *apps = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, a9);
    CHECK(apps);
    uint32_t flags = 0, build = 0, updated = 0xdeadbeef;
    char name[32] = "", desc[64] = "";
    CHECK((((Fn_beta9)vt(apps, slot_of(a9, "GetBetaInfo")))(apps, 1, &flags, &build, name, sizeof name, desc, sizeof desc, &updated)) & 0xff);
    CHECK(!strcmp(name, "beta") && build == 99 && updated == 1700000000u && (flags & 2));
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_c_id)(void *, uint64_t);
typedef uint64_t (SHIM_TC *Fn_c_u32)(void *, uint32_t);
typedef uint64_t (SHIM_TC *Fn_c_str)(void *, const char *);
typedef int32_t (SHIM_TC *Fn_c_idret)(void *, uint64_t);
typedef uint64_t *(SHIM_TC *Fn_c_owner)(void *, uint64_t *, uint64_t);
typedef uint64_t *(SHIM_TC *Fn_c_member)(void *, uint64_t *, uint64_t, int32_t);
typedef uint64_t (SHIM_TC *Fn_c_say)(void *, uint64_t, const char *);
typedef int32_t (SHIM_TC *Fn_c_getmsg)(void *, uint64_t, int32_t, char *, int32_t, int32_t *, uint64_t *);
typedef struct { uint32_t type; uint64_t id; } BeaconLoc;
typedef uint64_t (SHIM_TC *Fn_c_create)(void *, uint32_t, const BeaconLoc *, const char *, const char *);
typedef uint64_t (SHIM_TC *Fn_c_slots)(void *, uint64_t, uint32_t);
typedef uint64_t (SHIM_TC *Fn_c_fav)(void *, uint32_t, uint64_t);
typedef uint64_t (SHIM_TC *Fn_c_instant)(void *, const char *, const char *, const char *, uint32_t, float, int);
typedef void (SHIM_TC *Fn_c_load)(void *, uint32_t, const char *, const char *);
typedef void (SHIM_TC *Fn_c_allow)(void *, uint32_t, uint8_t);
typedef uint64_t (SHIM_TC *Fn_c_create_browser)(void *, const char *, const char *);
typedef uint64_t (SHIM_TC *Fn_c_call0)(void *);
static int t_community(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    uint8_t res[512];
    int32_t i32;
    uint64_t u64;
    const char *fv = "SteamFriends017";
    void *fr = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, fv);
    CHECK(fr);
    uint64_t call = ((Fn_c_id)vt(fr, sl(fv, "GetFollowerCount")))(fr, 76561198000000042ULL);
    CHECK(call != 0 && call_taken(call, 344, res, 24));
    memcpy(&i32, res, 4); CHECK(i32 == 1);
    memcpy(&u64, res + 8, 8); CHECK(u64 == 76561198000000042ULL);
    memcpy(&i32, res + 16, 4); CHECK(i32 == 42);
    call = ((Fn_c_id)vt(fr, sl(fv, "IsFollowing")))(fr, 76561198000000042ULL);
    CHECK(call != 0 && call_taken(call, 345, res, 24));
    CHECK(res[16] == 1);
    call = ((Fn_c_u32)vt(fr, sl(fv, "EnumerateFollowingList")))(fr, 0);
    CHECK(call != 0 && call_taken(call, 346, res, 416));
    memcpy(&u64, res + 8, 8); CHECK(u64 == 76561198000000001ULL);
    memcpy(&u64, res + 16, 8); CHECK(u64 == 76561198000000002ULL);
    memcpy(&i32, res + 408, 4); CHECK(i32 == 2);
    memcpy(&i32, res + 412, 4); CHECK(i32 == 5);

    call = ((Fn_c_id)vt(fr, sl(fv, "RequestClanOfficerList")))(fr, 103582791429521408ULL);
    CHECK(call != 0 && call_taken(call, 335, res, 16));
    memcpy(&i32, res + 8, 4); CHECK(i32 == 3 && res[12] == 1);
    CHECK(((Fn_c_idret)vt(fr, sl(fv, "GetClanOfficerCount")))(fr, 103582791429521408ULL) == 3);

    call = ((Fn_c_id)vt(fr, sl(fv, "JoinClanChatRoom")))(fr, 103582791429521408ULL);
    CHECK(call != 0 && call_taken(call, 342, res, 16));
    memcpy(&i32, res + 8, 4); CHECK(i32 == 1);
    CHECK(((Fn_c_idret)vt(fr, sl(fv, "GetClanChatMemberCount")))(fr, 103582791429521408ULL) == 3);
    uint64_t owner = 0, member = 0;
    ((Fn_c_owner)vt(fr, sl(fv, "GetClanOwner")))(fr, &owner, 103582791429521408ULL);
    CHECK(owner == 76561198000000100ULL);
    ((Fn_c_member)vt(fr, sl(fv, "GetChatMemberByIndex")))(fr, &member, 103582791429521408ULL, 1);
    CHECK(member == 76561198000000002ULL);
    CHECK(((Fn_c_say)vt(fr, sl(fv, "SendClanChatMessage")))(fr, 103582791429521408ULL, "hello group") & 0xff);
    CHECK(wait_callback(338, res, 24));
    int32_t msg_id;
    memcpy(&u64, res + 8, 8); CHECK(u64 == 76561198000000002ULL);
    memcpy(&msg_id, res + 16, 4);
    char text[64] = "";
    int32_t type = 0;
    uint64_t chatter = 0;
    CHECK(((Fn_c_getmsg)vt(fr, sl(fv, "GetClanChatMessage")))(fr, 103582791429521408ULL, msg_id, text, sizeof text, &type, &chatter) > 0);
    CHECK(!strcmp(text, "welcome") && chatter == 76561198000000002ULL);
    CHECK(((Fn_c_idret)vt(fr, sl(fv, "LeaveClanChatRoom")))(fr, 103582791429521408ULL) & 0xff);

    const char *rv = "STEAMREMOTESTORAGE_INTERFACE_VERSION016";
    void *rs = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamRemoteStorage")))(client, 1, 1, rv);
    CHECK(rs);
    call = ((Fn_c_str)vt(rs, sl(rv, "FileShare")))(rs, "save.dat");
    CHECK(call != 0 && call_taken(call, 1307, res, 280));
    memcpy(&i32, res, 4); CHECK(i32 == 1);
    memcpy(&u64, res + 8, 8); CHECK(u64 == 9999);
    CHECK(!strcmp((char *)res + 16, "save.dat"));

    const char *pv = "SteamParties002";
    void *parties = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, pv);
    CHECK(parties);
    BeaconLoc loc = { 1, 7 };
    call = ((Fn_c_create)vt(parties, sl(pv, "CreateBeacon")))(parties, 4, &loc, "+connect 1.2.3.4", "meta");
    CHECK(call != 0 && call_taken(call, 5302, res, 16));
    memcpy(&i32, res, 4); CHECK(i32 == 1);
    memcpy(&u64, res + 8, 8); CHECK(u64 == 555);
    CHECK(((Fn_c_call0)vt(parties, sl(pv, "GetNumActiveBeacons")))(parties) == 1);
    call = ((Fn_c_slots)vt(parties, sl(pv, "ChangeNumOpenSlots")))(parties, 555, 2);
    CHECK(call != 0 && call_taken(call, 5304, res, 4));
    memcpy(&i32, res, 4); CHECK(i32 == 1);
    call = ((Fn_c_id)vt(parties, sl(pv, "JoinParty")))(parties, 777);
    CHECK(call != 0 && call_taken(call, 5301, res, 280));
    memcpy(&i32, res, 4); CHECK(i32 == 1);
    CHECK(!strcmp((char *)res + 24, "+connect 5.6.7.8"));

    const char *uv = "STEAMUGC_INTERFACE_VERSION020";
    void *ugc = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamUGC")))(client, 1, 1, uv);
    CHECK(ugc);
    call = ((Fn_c_fav)vt(ugc, sl(uv, "AddItemToFavorites")))(ugc, 480, 123456);
    CHECK(call != 0 && call_taken(call, 3407, res, 16));
    memcpy(&u64, res, 8); CHECK(u64 == 123456);
    memcpy(&i32, res + 8, 4); CHECK(i32 == 1 && res[12] == 1);
    const char *suv = "SteamUser023";
    void *user = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, suv);
    CHECK(user);
    call = ((Fn_c_call0)vt(user, sl(suv, "GetMarketEligibility")))(user);
    CHECK(call != 0 && call_taken(call, 166, res, 20));
    CHECK(res[0] == 0);
    memcpy(&i32, res + 4, 4); CHECK(i32 == 128);
    memcpy(&i32, res + 12, 4); CHECK(i32 == 15);

    const char *tv = "STEAMTIMELINE_INTERFACE_V004";
    void *tl = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, tv);
    CHECK(tl);
    CHECK(((Fn_c_instant)vt(tl, sl(tv, "AddInstantaneousTimelineEvent")))(tl, "Ace", "5 kills", "trophy", 3, -2.5f, 1) != 0);

    const char *hv = "STEAMHTMLSURFACE_INTERFACE_VERSION_005";
    void *html = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, hv);
    CHECK(html);
    CHECK(((Fn_c_call0)vt(html, sl(hv, "Init")))(html) & 0xff);
    call = ((Fn_c_create_browser)vt(html, sl(hv, "CreateBrowser")))(html, "agent", NULL);
    CHECK(call != 0 && call_taken(call, 4501, res, 4));
    uint32_t browser;
    memcpy(&browser, res, 4);
    CHECK(browser != 0);
    ((Fn_c_load)vt(html, sl(hv, "LoadURL")))(html, browser, "https://store.steampowered.com/app/480", "");
    CHECK(wait_callback(4503, res, 40));
    ((Fn_c_allow)vt(html, sl(hv, "AllowStartRequest")))(html, browser, 1);
    CHECK(wait_callback(4506, res, 24));
    Sleep(800);
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_o_get)(void *, uint64_t, const char *, void *);
typedef uint64_t (SHIM_TC *Fn_o_ach)(void *, uint64_t, const char *, uint8_t *);
typedef uint64_t (SHIM_TC *Fn_o_seti)(void *, uint64_t, const char *, int32_t);
typedef uint64_t (SHIM_TC *Fn_o_setf)(void *, uint64_t, const char *, float);
typedef uint64_t (SHIM_TC *Fn_o_setach)(void *, uint64_t, const char *);
static int t_other_stats(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    const uint64_t player = 76561198000000042ULL;
    uint8_t res[64];
    int32_t i32;
    const char *uv = "STEAMUSERSTATS_INTERFACE_VERSION012";
    void *us = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamUserStats")))(client, 1, 1, uv);
    CHECK(us);
    uint64_t call = ((Fn_c_id)vt(us, sl(uv, "RequestUserStats")))(us, player);
    CHECK(call != 0 && call_taken(call, 1101, res, 24));
    memcpy(&i32, res + 8, 4); CHECK(i32 == 1);
    int32_t kills = 0;
    CHECK(((Fn_o_get)vt(us, sl(uv, "GetUserStat")))(us, player, "kills", &kills) & 0xff);
    CHECK(kills == 42);
    uint8_t got = 9;
    CHECK(((Fn_o_ach)vt(us, sl(uv, "GetUserAchievement")))(us, player, "ACH_ONE", &got) & 0xff);
    CHECK(got == 1);
    CHECK(((Fn_o_ach)vt(us, sl(uv, "GetUserAchievement")))(us, player, "ACH_TWO", &got) & 0xff);
    CHECK(got == 0);
    CHECK(!(((Fn_o_get)vt(us, sl(uv, "GetUserStat")))(us, 76561198000000099ULL, "kills", &kills) & 0xff));

    const char *gv = "SteamGameServerStats001";
    void *gss = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamGenericInterface")))(client, 1, 1, gv);
    CHECK(gss);
    call = ((Fn_c_id)vt(gss, sl(gv, "RequestUserStats")))(gss, 76561198000000077ULL);
    CHECK(call != 0 && call_taken(call, 1800, res, 16));
    memcpy(&i32, res, 4); CHECK(i32 == 1);
    CHECK(((Fn_o_seti)vt(gss, sl(gv, "SetUserStat")))(gss, 76561198000000077ULL, "kills", 50) & 0xff);
    CHECK(((Fn_o_setach)vt(gss, sl(gv, "SetUserAchievement")))(gss, 76561198000000077ULL, "ACH_TWO") & 0xff);
    CHECK(!(((Fn_o_setach)vt(gss, sl(gv, "SetUserAchievement")))(gss, 76561198000000077ULL, "NAO_EXISTE") & 0xff));
    call = ((Fn_c_id)vt(gss, sl(gv, "StoreUserStats")))(gss, 76561198000000077ULL);
    CHECK(call != 0 && call_taken(call, 1801, res, 16));
    memcpy(&i32, res, 4); CHECK(i32 == 1);
    return 1;
}

typedef uint64_t (SHIM_TC *Fn_l_write)(void *, const char *, const void *, int32_t);
typedef uint64_t (SHIM_TC *Fn_l_name)(void *, const char *);
typedef uint64_t (SHIM_TC *Fn_l_pub)(void *, const char *, const char *, uint32_t, const char *, const char *, int, const ParamStrings *, int);
typedef uint64_t (SHIM_TC *Fn_l_del)(void *, uint64_t);
static int live_step(const char *what, int ok) { printf("LIVE %s: %s\n", what, ok ? "ok" : "FAILED"); fflush(stdout); return ok; }
static int t_live_workshop(void) {
    shim_init();
    g_wait_iters = 1800;
    const uint32_t live_app = getenv("LIVE_APP") ? (uint32_t)atoi(getenv("LIVE_APP")) : 480;
    void *client = create("SteamClient023");
    CHECK(client);
    int all = 1;
    uint8_t res[64];
    int32_t result;
    uint64_t item = 0, call;
    const char *payload = "Aurelia test content (deleted afterwards)";
    if (getenv("LIVE_VARIANT") && strcmp(getenv("LIVE_VARIANT"), "legacy") != 0) goto ugc_part;
    const char *rv = "STEAMREMOTESTORAGE_INTERFACE_VERSION016";
    void *rs = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamRemoteStorage")))(client, 1, 1, rv);
    CHECK(rs);
    all &= live_step("FileWrite", (((Fn_l_write)vt(rs, slot_of(rv, "FileWrite")))(rs, "aurelia-live-test.txt", payload, (int32_t)strlen(payload)) & 0xff) == 1);
    const char *tags[1] = { "test" };
    ParamStrings arr = { tags, 1 };
    call = ((Fn_l_pub)vt(rs, slot_of(rv, "PublishWorkshopFile")))(rs, "aurelia-live-test.txt", NULL, live_app, "Aurelia legacy test", "Test item, deleted afterwards", 2, &arr, 0);
    all &= live_step("PublishWorkshopFile (legacy)", call != 0 && call_taken(call, 1309, res, 24));
    memcpy(&result, res, 4); memcpy(&item, res + 8, 8);
    printf("LIVE   eresult=%d item=%llu\n", result, (unsigned long long)item);
    all &= live_step("result of the legacy publish", result == 1 && item != 0 && item != ~0ULL);
    if (item && result == 1) {
        uint64_t h = ((Fn_rs_create)vt(rs, slot_of(rv, "CreatePublishedFileUpdateRequest")))(rs, item);
        ((Fn_rs_set_s)vt(rs, slot_of(rv, "UpdatePublishedFileTitle")))(rs, h, "Aurelia legacy test v2");
        call = ((Fn_rs_call)vt(rs, slot_of(rv, "CommitPublishedFileUpdate")))(rs, h);
        all &= live_step("update (legacy)", call != 0 && call_taken(call, 1316, res, 24));
        memcpy(&result, res, 4);
        all &= live_step("result of the legacy update", result == 1);
        call = ((Fn_l_del)vt(rs, slot_of(rv, "DeletePublishedFile")))(rs, item);
        all &= live_step("delete (legacy)", call != 0 && call_taken(call, 1311, res, 16));
        memcpy(&result, res, 4);
        all &= live_step("result of the legacy delete", result == 1);
    }
    if (!getenv("LIVE_KEEP")) ((Fn_l_name)vt(rs, slot_of(rv, "FileDelete")))(rs, "aurelia-live-test.txt");
    if (getenv("LIVE_VARIANT") && !strcmp(getenv("LIVE_VARIANT"), "legacy")) return all;
ugc_part:;
    const char *iv = "STEAMUGC_INTERFACE_VERSION021";
    void *ugc = ((Fn_p_iip)vt(client, slot_of("SteamClient023", "GetISteamUGC")))(client, 1, 1, iv);
    CHECK(ugc);
    CreateDirectoryA("Z:\\tmp\\aurelia-live-ws", NULL);
    HANDLE f = CreateFileA("Z:\\tmp\\aurelia-live-ws\\mod.txt", GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    if (f != INVALID_HANDLE_VALUE) { DWORD w; WriteFile(f, payload, (DWORD)strlen(payload), &w, NULL); CloseHandle(f); }
    call = ((Fn_ugc_create)vt(ugc, slot_of(iv, "CreateItem")))(ugc, live_app, 0);
    all &= live_step("CreateItem", call != 0 && call_taken(call, 3403, res, 16));
    uint64_t new_item;
    memcpy(&new_item, res, 8); memcpy(&result, res + 8, 4);
    printf("LIVE   eresult=%d item=%llu\n", result, (unsigned long long)new_item);
    all &= live_step("result of CreateItem", result == 1 && new_item != 0);
    if (new_item && result == 1) {
        typedef uint64_t (SHIM_TC *Fn_ver)(void *, uint64_t, const char *, const char *);
        uint64_t h = ((Fn_ugc_start)vt(ugc, slot_of(iv, "StartItemUpdate")))(ugc, live_app, new_item);
        ((Fn_ugc_set_s)vt(ugc, slot_of(iv, "SetItemTitle")))(ugc, h, "Aurelia UGC test");
        ((Fn_ugc_set_s)vt(ugc, slot_of(iv, "SetItemDescription")))(ugc, h, "Test item with video preview and versions, deleted afterwards");
        ((Fn_ugc_set_i)vt(ugc, slot_of(iv, "SetItemVisibility")))(ugc, h, 2);
        ((Fn_ugc_set_s)vt(ugc, slot_of(iv, "SetItemContent")))(ugc, h, "Z:\\tmp\\aurelia-live-ws");
        const char *variant = getenv("LIVE_VARIANT");
        if (!variant || !strcmp(variant, "all") || !strcmp(variant, "video")) ((Fn_ugc_set_s)vt(ugc, slot_of(iv, "AddItemPreviewVideo")))(ugc, h, "dQw4w9WgXcQ");
        if (!variant || !strcmp(variant, "all") || !strcmp(variant, "versions")) ((Fn_ver)vt(ugc, slot_of(iv, "SetRequiredGameVersions")))(ugc, h, "1.0", "2.0");
        uint64_t sub = ((Fn_ugc_set_s)vt(ugc, slot_of(iv, "SubmitItemUpdate")))(ugc, h, "live test");
        all &= live_step("SubmitItemUpdate (video preview and versions)", sub != 0 && call_taken(sub, 3404, res, 16));
        memcpy(&result, res, 4);
        printf("LIVE   eresult=%d\n", result);
        all &= live_step("result of SubmitItemUpdate", result == 1);
        call = ((Fn_ugc_call)vt(ugc, slot_of(iv, "DeleteItem")))(ugc, new_item);
        all &= live_step("DeleteItem", call != 0 && call_taken(call, 3417, res, 16));
        memcpy(&result, res, 4);
        all &= live_step("result of DeleteItem", result == 1);
    }
    return all;
}

typedef uint64_t (SHIM_TC *Fn_d_app)(void *, uint64_t, uint32_t);
static int t_app_dependencies(void) {
    shim_init();
    void *client = create("SteamClient023");
    CHECK(client);
    const char *iv = "STEAMUGC_INTERFACE_VERSION021";
    void *ugc = ((Fn_p_iip)vt(client, sl("SteamClient023", "GetISteamUGC")))(client, 1, 1, iv);
    CHECK(ugc);
    uint8_t res[160];
    int32_t i32;
    uint64_t call = ((Fn_d_app)vt(ugc, sl(iv, "AddAppDependency")))(ugc, 777, 550);
    CHECK(call != 0 && call_taken(call, 3414, res, 24));
    memcpy(&i32, res, 4); CHECK(i32 == 1);
    call = ((Fn_c_id)vt(ugc, sl(iv, "GetAppDependencies")))(ugc, 777);
    CHECK(call != 0 && call_taken(call, 3416, res, 152));
    uint32_t app, n, total;
    memcpy(&app, res + 16, 4); memcpy(&n, res + 144, 4); memcpy(&total, res + 148, 4);
    CHECK(app == 550 && n == 1 && total == 1);
    call = ((Fn_d_app)vt(ugc, sl(iv, "RemoveAppDependency")))(ugc, 777, 550);
    CHECK(call != 0 && call_taken(call, 3415, res, 24));
    return 1;
}

typedef struct { const char *name; int (*fn)(void); } NetCase;
static const NetCase NET_CASES[] = {
    {"leaderboards", t_leaderboards},
    {"appticket", t_appticket},
    {"storeurl", t_storeurl},
    {"lobbies", t_lobbies_net},
    {"inventory", t_inventory},
    {"ugc", t_ugc},
    {"ugcwrite", t_ugcwrite},
    {"legacy_publish", t_legacy_publish},
    {"ugcpublish", t_ugcpublish},
    {"ugcquery", t_ugcquery},
    {"legacy_workshop", t_legacy_workshop},
    {"gameserver", t_gameserver},
    {"webticket", t_webticket},
    {"offline", t_offline},
    {"account_queries", t_account_queries},
    {"legacy_signatures", t_legacy_signatures},
    {"community", t_community},
    {"other_stats", t_other_stats},
    {"app_dependencies", t_app_dependencies},
    {"live_workshop", t_live_workshop},
    {"overlay_requests", t_overlay_requests},
    {"overlay_capable", t_overlay_capable},
    {"overlay_shown", t_overlay_shown},
    {"overlay_join", t_overlay_join},
    {"overlay_data", t_overlay_data},
    {"gamepad_text", t_gamepad_text},
    {"gamepad_floating", t_gamepad_floating},
    {"dlc_updates", t_dlc_updates},
    {"microtxn_capture", t_microtxn_capture},
    {"p2p_a", t_p2p_a},
    {"p2p_b", t_p2p_b},
    {"sockets_a", t_sockets_a},
    {"sockets_b", t_sockets_b},
    {"messages_a", t_messages_a},
    {"messages_b", t_messages_b},
    {"socketpair", t_socketpair},
    {"ipsockets", t_ipsockets},
    {"servers", t_servers},
    {"live", t_live},
    {"livebuy", t_livebuy},
    {"cm", t_cm_relay},
};

static int run_net_case(const char *name) {
    for (size_t i = 0; i < sizeof NET_CASES / sizeof *NET_CASES; i++) {
        if (strcmp(NET_CASES[i].name, name)) continue;
        int ok = NET_CASES[i].fn();
        printf("%s net_%s\n", ok ? "PASS" : "FAIL", name);
        return !ok;
    }
    printf("FAIL unknown net case %s\n", name);
    return 1;
}

int main(void) {
    if (getenv("SELFTEST_NET")) return run_net_case(getenv("SELFTEST_NET"));
    if (getenv("SELFTEST_HTTP")) {
        int ok = t_http();
        printf("%s http_requests_complete_through_winhttp\n", ok ? "PASS" : "FAIL");
        return !ok;
    }
    if (getenv("SELFTEST_CRASH")) return t_crash();
    if (getenv("SELFTEST_PRESENCE")) {
        int ok = t_rich_presence();
        printf("%s rich_presence_goes_to_the_bridge\n", ok ? "PASS" : "FAIL");
        return !ok;
    }
    if (getenv("SELFTEST_STATS")) {
        int ok = t_user_stats();
        printf("%s user_stats_roundtrip\n", ok ? "PASS" : "FAIL");
        return !ok;
    }
    if (getenv("SELFTEST_BRIDGE_REFUSE")) {
        int ok = t_bridge_refuses();
        printf("%s bridge_refusal_does_not_fall_back\n", ok ? "PASS" : "FAIL");
        return !ok;
    }
    if (getenv("SELFTEST_TICKET_OFFLINE")) {
        int ok = t_ticket_offline();
        printf("%s ticket_offline_uses_the_snapshot_reserve\n", ok ? "PASS" : "FAIL");
        return !ok;
    }
    if (getenv("SELFTEST_BRIDGE")) {
        int ok = t_bridge_ticket();
        printf("%s ticket_is_created_by_the_bridge\n", ok ? "PASS" : "FAIL");
        return !ok;
    }
    struct { const char *name; int (*fn)(void); } tests[] = {
        {"frame_encoding_matches_rust_protocol", t_frame_encoding},
        {"call_frames_match_the_rust_protocol_and_fail_fast_offline", t_call_encoding},
        {"snapshot_roundtrip_matches_fixture", t_snapshot},
        {"create_interface_unknown_name_returns_null_and_logs", t_unknown_interface},
        {"user_get_steamid_matches_snapshot", t_user},
        {"bridge_down_keeps_cached_snapshot_and_does_not_block", t_bridge_down},
        {"apps_subscription_follows_snapshot", t_apps},
        {"utils_reports_appid_and_country", t_utils},
        {"auth_ticket_has_inferred_layout", t_auth_ticket},
        {"run_frame_stubs_are_silent_noops", t_runframe_noops},
        {"overwatch_first_calls_are_implemented", t_overwatch_first_calls},
        {"callback_queue_answers_ticket_and_stats_requests", t_callbacks},
        {"ticket_file_override_replaces_the_built_ticket", t_ticket_override},
        {"auth_ticket_identity_argument_is_logged", t_auth_identity_logged},
        {"auth_ticket_works_for_steamuser022_without_identity", t_auth_ticket_v022},
        {"trace_logs_implemented_calls_when_enabled", t_trace},
        {"overwatch_post_login_calls_are_implemented", t_overwatch_post_login_calls},
        {"friends_come_from_the_snapshot", t_friends},
        {"networking_utils_clock_relay_and_unavailable_sockets", t_networking_utils},
        {"input_without_controllers_answers_inactive_data", t_input},
        {"friend_game_played_variants", t_friend_game_variants},
        {"screenshots_are_written_as_png_and_tracked", t_screenshots},
        {"utils_apps_applist_and_client_extras", t_utils_apps_client_extras},
        {"user_connection_auth_session_and_gameserver_local_state", t_user_and_gameserver},
        {"unimplemented_methods_answer_by_return_kind", t_default_behavior},
        {"loopback_sockets_connect_inside_the_process", t_sockets},
        {"player_count_and_clans_answer_without_inventing_data", t_players_and_clans},
        {"matchmaking_servers_without_a_bridge_fail_or_come_up_empty", t_servers_offline},
        {"remote_storage_files_live_in_the_cloud_dir", t_remote_storage},
        {"call_results_complete_through_703_and_get_api_call_result", t_call_results},
        {"legacy_steamclient_exports_follow_the_real_dll", t_legacy_exports},
        {"extras_answer_with_the_real_result_types", t_extras},
        {"protobuf_wire_format_round_trips", t_protobuf},
    };
    int failed = 0;
    const char *only = getenv("SELFTEST_ONLY");
    for (size_t i = 0; i < sizeof tests / sizeof *tests; i++) {
        if (only && !strstr(tests[i].name, only)) continue;
        int ok = tests[i].fn();
        printf("%s %s\n", ok ? "PASS" : "FAIL", tests[i].name);
        failed += !ok;
    }
    return failed ? 1 : 0;
}
