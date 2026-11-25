#ifndef SHIM_H
#define SHIM_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "ifaces.h"

#ifdef _WIN64
#define SHIM_TC
#else
#define SHIM_TC __attribute__((thiscall))
#endif

typedef struct { uint32_t appid; char *name; bool installed; } ShimApp;
typedef struct { uint32_t appid; uint8_t *ownership; size_t ownership_len; uint8_t *token; size_t token_len; } ShimTicket;
typedef struct { uint32_t appid; bool owned, installed; char name[96]; } ShimDlc;
typedef struct { char *name, *description; uint32_t build_id, time_updated; bool priv; } ShimBeta;
typedef struct { uint32_t appid; char *install_dir; char *cloud_dir; uint64_t cloud_quota; uint64_t build_id; char *beta_name; ShimDlc *dlc; size_t n_dlc; uint32_t *depots; size_t n_depots; ShimBeta *betas; size_t n_betas; uint32_t purchase_time; } ShimAppInfo;
typedef struct { uint64_t steamid; char *persona; uint32_t state, game_appid; } ShimFriend;
typedef struct {
    int schema;
    uint64_t steamid;
    char persona[128];
    char country[8];
    ShimApp *apps;
    size_t n_apps;
    bool has_ticket;
    ShimTicket ticket;
    bool has_app;
    ShimAppInfo app;
    ShimFriend *friends;
    size_t n_friends;
    bool online;
} Snapshot;

bool snapshot_load(const char *json, size_t len, Snapshot *out);
void snapshot_free(Snapshot *s);

typedef struct { uint8_t tag; const uint8_t *body; size_t body_len; } BridgeFrame;
size_t bridge_encode_hello(uint8_t *out, size_t cap, const uint8_t token[32]);
size_t bridge_encode_snapshot_req(uint8_t *out, size_t cap);
int bridge_decode(const uint8_t *buf, size_t len, BridgeFrame *f, size_t *used);
bool bridge_connect(const char *host, uint16_t port, const uint8_t token[32], int timeout_ms);
size_t bridge_encode_call(uint8_t *out, size_t cap, uint32_t seq, uint16_t iface, uint16_t slot, const uint8_t *payload, size_t len);
bool bridge_call(uint16_t iface, uint16_t slot, const uint8_t *payload, size_t len, int32_t *status,
                 uint8_t *reply, size_t reply_cap, size_t *reply_len, int timeout_ms);
uint32_t shim_ipc_call_count(void);
bool bridge_call_fresh(uint16_t iface, uint16_t slot, const uint8_t *payload, size_t len, int32_t *status,
                       uint8_t *reply, size_t reply_cap, size_t *reply_len, int timeout_ms);
bool bridge_fetch_snapshot(char **json, size_t *len, int timeout_ms);
bool bridge_reachable(void);
bool shim_connection_online(void);
void shim_poll_connection(void);
bool shim_session_online(void);
bool shim_app_owned(uint32_t appid);
void shim_set_local_ip_binding(uint32_t ip_host_order, uint16_t port);
bool shim_bind_local_udp(uintptr_t sock, uint16_t port, bool use_binding_port);
uint32_t shim_local_ip_binding(void);
uint16_t shim_local_port_binding(void);
void shim_gs_set_spawn_count(uint32_t count);
void bridge_close(void);

typedef struct { uint8_t *p; size_t len, cap; bool oom; } PbW;
void pbw_free(PbW *w);
void pbw_varint(PbW *w, uint32_t field, uint64_t v);
void pbw_fixed32(PbW *w, uint32_t field, uint32_t v);
void pbw_fixed64(PbW *w, uint32_t field, uint64_t v);
void pbw_bytes(PbW *w, uint32_t field, const void *data, size_t len);
void pbw_str(PbW *w, uint32_t field, const char *s);
void pbw_msg(PbW *w, uint32_t field, const PbW *inner);
void pbw_append(PbW *w, const void *data, size_t len);
typedef struct { const uint8_t *p, *end; bool bad; } PbR;
typedef struct { uint32_t field; int wt; uint64_t v; const uint8_t *data; size_t len; } PbField;
void pbr_init(PbR *r, const uint8_t *p, size_t len);
bool pbr_next(PbR *r, PbField *f);

#ifdef SHIM_WITH_GNS
int shim_gns_start(unsigned appid);
void shim_gns_start_async(unsigned appid);
void shim_gns_kill(void);
int shim_gns_use(void);
void shim_gns_pump(void);
unsigned shim_gns_listen_ip(const void *addr, int n, const void *opts);
unsigned shim_gns_connect_ip(const void *addr, int n, const void *opts);
unsigned shim_gns_listen_p2p(int vport, int n, const void *opts);
unsigned shim_gns_connect_p2p(const void *identity, int vport, int n, const void *opts);
int shim_gns_accept(unsigned conn);
int shim_gns_close_conn(unsigned conn, int reason, const char *debug, int linger);
int shim_gns_close_listen(unsigned l);
int shim_gns_set_user_data(unsigned conn, int64_t v);
int64_t shim_gns_get_user_data(unsigned conn);
void shim_gns_set_name(unsigned conn, const char *name);
int shim_gns_get_name(unsigned conn, char *out, int cap);
int shim_gns_send(unsigned conn, const void *data, unsigned cb, int flags, int64_t *number);
void shim_gns_send_messages(int n, void *const *msgs, int64_t *results);
int shim_gns_flush(unsigned conn);
int shim_gns_receive(unsigned conn, void **out, int max);
int shim_gns_receive_group(unsigned group, void **out, int max);
int shim_gns_get_info(unsigned conn, void *info);
int shim_gns_realtime(unsigned conn, void *status, int lanes, void *lane_status);
int shim_gns_detailed(unsigned conn, char *out, int cap);
int shim_gns_listen_address(unsigned l, void *addr);
int shim_gns_socket_pair(unsigned *a, unsigned *b, int loopback, const void *ida, const void *idb);
int shim_gns_get_identity(void *id);
int shim_gns_init_auth(void);
int shim_gns_auth_status(void *out);
unsigned shim_gns_create_group(void);
int shim_gns_destroy_group(unsigned g);
int shim_gns_set_group(unsigned conn, unsigned group);
void shim_gns_run_callbacks(void);
int shim_gns_msg_send(const void *identity, const void *data, unsigned cb, int flags, int channel);
int shim_gns_msg_receive(int channel, void **out, int max);
int shim_gns_msg_accept(const void *identity);
int shim_gns_msg_close_session(const void *identity);
int shim_gns_msg_close_channel(const void *identity, int channel);
int shim_gns_msg_session_info(const void *identity, void *info, void *status);
int shim_gns_relay_status(void *out);
void shim_gns_init_relay(void);
void *shim_gns_allocate_message(int cb);
void *shim_gns_native(const char *version);
#define GNS_DELEGATE(expr) do { if (shim_gns_use()) return (expr); } while (0)
#define GNS_DELEGATE_VOID(stmt) do { if (shim_gns_use()) { stmt; return; } } while (0)
#else
#define GNS_DELEGATE(expr) do { } while (0)
#define GNS_DELEGATE_VOID(stmt) do { } while (0)
static inline void shim_gns_pump(void) {}
static inline void *shim_gns_native(const char *version) { (void)version; return NULL; }
#endif

typedef struct { int32_t emsg; int32_t eresult; uint8_t *body; size_t len; } CmReply;
enum { CM_EMSG_SERVICE_CALL = 151 };
bool shim_cm_call(int32_t emsg, bool protobuf, const char *job, const uint8_t *body, size_t len, int wait_ms, CmReply *reply);
void shim_cm_reply_free(CmReply *r);
bool shim_cm_send(int32_t emsg, bool protobuf, const uint8_t *body, size_t len);
bool shim_cm_gs_send(int32_t emsg, const uint8_t *body, size_t len);
bool shim_cm_service(const char *method, const uint8_t *body, size_t len, int wait_ms, CmReply *reply);
void shim_cm_post(int32_t emsg, PbW *body);
bool shim_cm_flush(int wait_ms);
typedef void (*ShimPushFn)(const uint8_t *body, size_t len);
bool shim_cm_on_push(int32_t emsg, ShimPushFn fn);
bool shim_cm_subscribe(int32_t emsg);
bool shim_cm_poll(int32_t *emsg, uint8_t **body, size_t *len);
typedef bool (*CmParse)(const CmReply *reply, void *ctx, void *out, int *size);
void shim_cm_async(uint64_t handle, int callback_id, int32_t emsg, bool service, const char *job, PbW *body, int wait_ms, CmParse parse, void *ctx);

typedef struct { uint32_t appid, ip; uint16_t conn_port, query_port; uint32_t flags, last_played; } ShimServerEntry;
int shim_serverlist_get(ShimServerEntry *out, int max, uint32_t flag, uint32_t appid);

int shim_verify_auth_ticket(const void *ticket, uint32_t cb, uint32_t appid, uint64_t expect_steamid, uint64_t *owner_out);

typedef struct P2PMsg { uint64_t peer; uint32_t stream; uint8_t flags; int len; uint8_t *data; struct P2PMsg *next; } P2PMsg;
typedef void (*P2PEventFn)(int ev, uint64_t peer, int arg);
void p2p_set_event(P2PEventFn fn);
void p2p_start(void);
bool p2p_send(uint64_t peer, uint32_t stream, uint8_t flags, const void *data, int len);
P2PMsg *p2p_take(uint64_t peer, uint32_t lo, uint32_t hi);
int p2p_peek(uint64_t peer, uint32_t lo, uint32_t hi, uint64_t *from);
void p2p_free(P2PMsg *m);
int p2p_state(uint64_t peer);
void p2p_close(uint64_t peer);
uint16_t p2p_local_port(void);
int p2p_listen_ip(uint16_t port);
void p2p_unlisten_ip(uint16_t port);
uint64_t p2p_connect_ip(uint32_t ip_host_order, uint16_t port);
bool p2p_is_direct(uint64_t peer);
void p2p_direct_addr(uint64_t peer, uint32_t *ip, uint16_t *port);
void p2p_interface_requested(const char *name);
void userstats_preload(void);
void account_preload(void);
void serverlists_preload(void);

size_t shim_win_path(const char *path, char *out, size_t cap);
void shim_log(const char *fmt, ...);
void shim_trace(const char *fmt, ...);
void shim_install_call_trace(void);
void register_images(void);
void shim_prefetch_own_avatar(void);
int shim_is_policy_stub(const void *fn);
bool shim_load_snapshot_json(const char *json, size_t len);
const Snapshot *shim_snapshot(void);
uint32_t shim_process_appid(void);
void shim_init(void);
void *CreateInterface(const char *name, int *return_code);
void shim_register_family(const char *prefix, const char *method, void *fn);
void shim_register_family_ab(const char *prefix, const char *method, int argbytes, void *fn);

#pragma pack(push, 8)
typedef struct { int32_t user; int32_t id; uint8_t *param; int32_t size; } ShimCallbackMsg;
#pragma pack(pop)
void shim_post_callback(int id, const void *data, int size);
uint64_t shim_current_lobby(void);
void shim_post_callback_after_result(int id, const void *data, int size);
void shim_flush_after_result(void);
bool shim_next_callback(ShimCallbackMsg *out);
void shim_free_last_callback(void);
typedef void (*ShimDeferred)(void *a, void *b, int c);
void shim_defer(ShimDeferred fn, void *a, void *b, int c);

void shim_callresult_fail_handle(uint64_t handle);
uint64_t shim_callresult_post(int callback_id, const void *data, int size);
uint64_t shim_callresult_reserve(void);
void shim_callresult_complete(uint64_t handle, int callback_id, const void *data, int size);
bool shim_callresult_completed(uint64_t handle, bool *failed);
bool shim_callresult_take(uint64_t handle, void *buf, int cap, int expected_callback, bool *failed);

void register_policies(void);
void register_client(void);
void register_user(void);
void register_apps(void);
void register_utils(void);
void register_misc(void);
void register_userstats(void);
void register_leaderboards(void);
void register_appticket(void);
void register_inventory(void);
void register_ugc(void);
void register_ugcquery(void);
void register_ugcwrite(void);
void register_remotepublish(void);
void register_serverlists(void);
void register_remotestorage(void);
void register_matchmakingservers(void);
void register_lobbies(void);
void register_gameserver(void);
void *shim_gs_handle(void);
uint64_t shim_gs_logged_on(void), shim_gs_steamid(void), shim_gs_secure(void);
void shim_gs_logon(void), shim_gs_logoff(void);
uint64_t shim_gs_set_server_type(int32_t appid, uint32_t flags, uint32_t ip, uint16_t port, const char *game_dir, const char *version);
uint64_t shim_gs_update_status(int players, int max_players, int bots, const char *name, const char *map);
uint64_t shim_gs_user_connect(uint32_t user_id, uint32_t ip, const void *cookie, uint32_t cb);
uint64_t shim_gs_user_remove(uint32_t user_id);
int shim_user_initiate_game_connection(void *blob, int cb_max, uint32_t ip, uint32_t port);
void shim_user_terminate_game_connection(uint32_t ip, uint32_t port);
void register_applist(void);
void register_screenshots(void);
void register_input(void);
void register_http(void);
void register_networkingutils(void);
void register_sockets(void);
void register_overlay(void);
void register_extras(void);
void register_community(void);
void register_otherstats(void);
void shim_overlay_web_page(const char *url);
void shim_dlc_apply(const uint8_t *buf, size_t n);
void shim_overlay_activated(int active);
void shim_overlay_activated_now(int active);
enum { MTX_PENDING = 0, MTX_APPROVED = 1, MTX_DENIED = 2 };
int shim_microtxn_wait(uint64_t order, int timeout_ms);
bool shim_microtxn_owns(const char *id);
void shim_overlay_submit(const char *kind, const char *json, uint64_t order);
void shim_microtxn_track(const char *id, uint64_t order);
void shim_microtxn_capture_start(void);
const char *shim_game_language(void);
void shim_overlay_achievement(const char *name);
bool shim_overlay_capable(void);
void shim_overlay_start(void);
void shim_microtxn_answer(const char *id, int approved, int denied_by_steam);
const char *shim_iface_name(void *self);
void *shim_net_allocate_message(int cb);
void register_networking(void);
void register_p2pnet(void);

#endif
