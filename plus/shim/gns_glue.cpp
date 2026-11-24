#include <steam/steamnetworkingsockets.h>
#include <steam/isteamnetworkingutils.h>
#include <steam/isteamnetworkingmessages.h>
#include <steam/steamnetworkingcustomsignaling.h>
#include <windows.h>
#include <winhttp.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

extern "C" {
#include "shim.h"
}

extern "C" void GameNetworkingSockets_SetAppID( unsigned nAppID );
extern "C" void GameNetworkingSockets_TEST_EnableP2PLoopback( bool bEnable );
namespace SteamNetworkingSocketsLib
{
	bool CertStore_AddCertFromBase64( const char *pszBase64, SteamNetworkingErrMsg &errMsg );
	void CertStore_AddKeyRevocation( uint64 key_id );
}

namespace
{
	const int32_t k_EMsgClientP2PConnectionInfo = 5434;
	const int32_t k_EMsgClientNetworkingCertRequest = 5621;
	const int32_t k_EMsgClientNetworkingCertRequestResponse = 5622;

	unsigned g_appid;
	uint64_t g_steamid;
	volatile LONG g_state;

	ISteamNetworkingSockets *Sock() { return SteamNetworkingSockets(); }
	ISteamNetworkingMessages *Msgs() { return SteamNetworkingMessages(); }
	const SteamNetworkingIdentity &Id( const void *p ) { return *(const SteamNetworkingIdentity *)p; }

	void OnConnectionStatus( SteamNetConnectionStatusChangedCallback_t *p ) { shim_post_callback( 1221, p, (int)sizeof *p ); }
	void OnAuthStatus( SteamNetAuthenticationStatus_t *p ) { shim_post_callback( 1222, p, (int)sizeof *p ); }
	void OnRelayStatus( SteamRelayNetworkStatus_t *p ) { shim_post_callback( 1281, p, (int)sizeof *p ); }
	void OnMessagesRequest( SteamNetworkingMessagesSessionRequest_t *p ) { shim_post_callback( 1251, p, (int)sizeof *p ); }
	void OnMessagesFailed( SteamNetworkingMessagesSessionFailed_t *p ) { shim_post_callback( 1252, p, (int)sizeof *p ); }

	void GnsDebug( ESteamNetworkingSocketsDebugOutputType type, const char *msg ) { shim_log( "GNS[%d] %s", (int)type, msg ); }

	uint32_t RendezvousFromConnection( const void *msg, size_t cb )
	{
		PbR r; pbr_init( &r, (const uint8_t *)msg, cb );
		PbField f;
		while ( pbr_next( &r, &f ) ) if ( f.field == 9 && f.wt == 5 ) return (uint32_t)f.v;
		return 0;
	}

	CRITICAL_SECTION g_lockSent;
	uint32_t g_sent[256];
	unsigned g_nSent;
	void RememberSent( uint32_t id )
	{
		if ( !id ) return;
		EnterCriticalSection( &g_lockSent );
		bool known = false;
		for ( unsigned i = 0; i < g_nSent && i < 256; ++i ) known |= g_sent[i] == id;
		if ( !known ) g_sent[g_nSent++ % 256] = id;
		LeaveCriticalSection( &g_lockSent );
	}
	bool SentByUs( uint32_t id )
	{
		if ( !id ) return false;
		EnterCriticalSection( &g_lockSent );
		bool known = false;
		for ( unsigned i = 0; i < g_nSent && i < 256; ++i ) known |= g_sent[i] == id;
		LeaveCriticalSection( &g_lockSent );
		return known;
	}

	bool SendRendezvous( uint64_t dest, const void *msg, int cb )
	{
		if ( !dest || !g_steamid || cb <= 0 ) return false;
		PbW w = {};
		pbw_fixed64( &w, 1, dest );
		pbw_fixed64( &w, 2, g_steamid );
		pbw_varint( &w, 3, g_appid );
		pbw_bytes( &w, 6, msg, (size_t)cb );
		RememberSent( RendezvousFromConnection( msg, (size_t)cb ) );
		shim_cm_post( k_EMsgClientP2PConnectionInfo, &w );
		return true;
	}

	class ShimSignaling : public ISteamNetworkingConnectionSignaling
	{
	public:
		explicit ShimSignaling( uint64_t peer ) : m_peer( peer ) {}
		bool SendSignal( HSteamNetConnection, const SteamNetConnectionInfo_t &info, const void *pMsg, int cbMsg ) override
		{
			uint64_t dest = info.m_identityRemote.GetSteamID64();
			if ( !dest ) dest = m_peer;
			bool ok = SendRendezvous( dest, pMsg, cbMsg );
			shim_log( "GNS outgoing signal to %llu: %d bytes (%s)", (unsigned long long)dest, cbMsg, ok ? "queued" : "dropped" );
			return ok;
		}
		void Release() override { delete this; }
	private:
		uint64_t m_peer;
	};

	ISteamNetworkingConnectionSignaling *CreateSignaling( ISteamNetworkingSockets *, const SteamNetworkingIdentity &peer, int, int )
	{
		uint64_t id = peer.GetSteamID64();
		if ( !id ) { shim_log( "GNS: peer identity is not a SteamID; no signaling" ); return nullptr; }
		return new ShimSignaling( id );
	}

	class RecvContext : public ISteamNetworkingSignalingRecvContext
	{
	public:
		ISteamNetworkingConnectionSignaling *OnConnectRequest( HSteamNetConnection, const SteamNetworkingIdentity &peer, int ) override
		{
			uint64_t id = peer.GetSteamID64();
			return id ? new ShimSignaling( id ) : nullptr;
		}
		void SendRejectionSignal( const SteamNetworkingIdentity &peer, const void *pMsg, int cbMsg ) override
		{
			SendRendezvous( peer.GetSteamID64(), pMsg, cbMsg );
		}
	};

	void OnP2PConnectionInfo( const uint8_t *body, size_t len )
	{
		PbR r; pbr_init( &r, body, len );
		PbField f;
		uint64_t src = 0; const uint8_t *rv = nullptr; size_t rvlen = 0;
		while ( pbr_next( &r, &f ) )
		{
			if ( f.field == 2 && f.wt == 1 ) src = f.v;
			else if ( f.field == 6 && f.wt == 2 ) { rv = f.data; rvlen = f.len; }
		}
		if ( !rv || !rvlen ) { shim_log( "GNS: 5434 without rendezvous (%zu bytes)", len ); return; }
		if ( rvlen >= 4 && !memcmp( rv, "AP2P", 4 ) ) return;
		if ( src == g_steamid && SentByUs( RendezvousFromConnection( rv, rvlen ) ) ) return;
		ISteamNetworkingSockets *sock = SteamNetworkingSockets();
		if ( !sock ) return;
		RecvContext ctx;
		bool ok = sock->ReceivedP2PCustomSignal( rv, (int)rvlen, &ctx );
		shim_log( "GNS incoming signal from %llu: %zu bytes (%s)", (unsigned long long)src, rvlen, ok ? "handled" : "refused" );
	}

	bool HttpGet( const wchar_t *host, const wchar_t *path, std::string &out )
	{
		HINTERNET s = WinHttpOpen( L"aurelia-shim", WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, nullptr, nullptr, 0 );
		if ( !s ) return false;
		WinHttpSetTimeouts( s, 8000, 8000, 8000, 15000 );
		bool ok = false;
		HINTERNET c = WinHttpConnect( s, host, INTERNET_DEFAULT_HTTPS_PORT, 0 );
		HINTERNET q = c ? WinHttpOpenRequest( c, L"GET", path, nullptr, WINHTTP_NO_REFERER, WINHTTP_DEFAULT_ACCEPT_TYPES, WINHTTP_FLAG_SECURE ) : nullptr;
		if ( q && WinHttpSendRequest( q, WINHTTP_NO_ADDITIONAL_HEADERS, 0, nullptr, 0, 0, 0 ) && WinHttpReceiveResponse( q, nullptr ) )
		{
			DWORD status = 0, sz = sizeof status;
			WinHttpQueryHeaders( q, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, nullptr, &status, &sz, nullptr );
			char buf[8192]; DWORD n;
			while ( WinHttpReadData( q, buf, sizeof buf, &n ) && n ) out.append( buf, n );
			ok = status == 200;
		}
		if ( q ) WinHttpCloseHandle( q );
		if ( c ) WinHttpCloseHandle( c );
		WinHttpCloseHandle( s );
		return ok;
	}

	std::vector<std::string> JsonStringArray( const std::string &json, const char *key )
	{
		std::vector<std::string> out;
		size_t p = json.find( std::string( "\"" ) + key + "\"" );
		if ( p == std::string::npos ) return out;
		p = json.find( '[', p );
		size_t end = p == std::string::npos ? p : json.find( ']', p );
		if ( p == std::string::npos || end == std::string::npos ) return out;
		for ( size_t i = p + 1; i < end; )
		{
			size_t a = json.find( '"', i );
			if ( a == std::string::npos || a >= end ) break;
			std::string s;
			size_t b = a + 1;
			for ( ; b < end && json[b] != '"'; ++b ) { if ( json[b] == '\\' && b + 1 < end ) ++b; s += json[b]; }
			out.push_back( s );
			i = b + 1;
		}
		return out;
	}

	void LoadSdrConfig( unsigned appid )
	{
		wchar_t path[96];
		swprintf( path, 96, L"/ISteamApps/GetSDRConfig/v1/?appid=%u", appid );
		std::string json;
		if ( !HttpGet( L"api.steampowered.com", path, json ) ) { shim_log( "GNS: SDR configuration unavailable" ); return; }
		int added = 0, failed = 0;
		for ( const std::string &c : JsonStringArray( json, "certs" ) )
		{
			SteamNetworkingErrMsg err;
			if ( SteamNetworkingSocketsLib::CertStore_AddCertFromBase64( c.c_str(), err ) ) ++added; else { ++failed; shim_log( "GNS: certificate rejected: %s", err ); }
		}
		int revoked = 0;
		for ( const std::string &k : JsonStringArray( json, "revoked_keys" ) ) { SteamNetworkingSocketsLib::CertStore_AddKeyRevocation( strtoull( k.c_str(), nullptr, 10 ) ); ++revoked; }
		shim_log( "GNS: SDR configuration: %d certificates, %d rejected, %d revoked keys", added, failed, revoked );
	}

	bool ProvisionCertificate( ISteamNetworkingSockets *sock )
	{
		SteamNetworkingErrMsg err;
		unsigned char blob[1024]; int cb = sizeof blob;
		if ( !sock->GetCertificateRequest( &cb, blob, err ) ) { shim_log( "GNS: GetCertificateRequest failed: %s", err ); return false; }
		int off = -1;
		for ( int i = 0; i + 34 <= cb; ++i ) if ( blob[i] == 0x12 && blob[i + 1] == 0x20 ) { off = i + 2; break; }
		if ( off < 0 ) { shim_log( "GNS: public key not found in the certificate request" ); return false; }
		PbW w = {};
		pbw_bytes( &w, 2, blob + off, 32 );
		pbw_varint( &w, 3, g_appid );
		CmReply reply = {};
		bool ok = shim_cm_call( k_EMsgClientNetworkingCertRequest, true, nullptr, w.p, w.len, 15000, &reply );
		pbw_free( &w );
		if ( !ok ) { shim_log( "GNS: Steam did not answer the certificate request" ); return false; }
		bool good = reply.emsg == k_EMsgClientNetworkingCertRequestResponse && reply.eresult == 1 && reply.len > 0;
		if ( !good ) shim_log( "GNS: unexpected reply to the certificate request: emsg=%d eresult=%d %zu bytes", reply.emsg, reply.eresult, reply.len );
		else if ( !sock->SetCertificate( reply.body, (int)reply.len, err ) ) { shim_log( "GNS: SetCertificate refused: %s", err ); good = false; }
		shim_cm_reply_free( &reply );
		if ( good ) shim_log( "GNS: Valve certificate installed for app %u", g_appid );
		return good;
	}
}

extern "C" {

int shim_gns_embedded( void ) { return 1; }

int shim_gns_start( unsigned appid )
{
	const Snapshot *s = shim_snapshot();
	if ( !s || !s->steamid ) { shim_log( "GNS: no account snapshot" ); return -1; }
	g_appid = appid ? appid : ( s->has_ticket ? s->ticket.appid : 0 );
	g_steamid = s->steamid;
	InitializeCriticalSection( &g_lockSent );
	GameNetworkingSockets_SetAppID( g_appid );
	SteamNetworkingIdentity me; me.SetSteamID64( g_steamid );
	SteamNetworkingErrMsg err;
	if ( !GameNetworkingSockets_Init( &me, err ) ) { shim_log( "GNS: Init failed: %s", err ); return -2; }
	char test[4];
	if ( GetEnvironmentVariableA( "AURELIA_SHIM_GNS_TEST_NO_LOOPBACK", test, sizeof test ) && test[0] == '1' ) GameNetworkingSockets_TEST_EnableP2PLoopback( false );
	ISteamNetworkingUtils *utils = SteamNetworkingUtils();
	utils->SetDebugOutputFunction( k_ESteamNetworkingSocketsDebugOutputType_Msg, GnsDebug );
	utils->SetGlobalConfigValuePtr( k_ESteamNetworkingConfig_Callback_CreateConnectionSignaling, (void *)CreateSignaling );
	utils->SetGlobalConfigValueString( k_ESteamNetworkingConfig_P2P_STUN_ServerList, "stun.l.google.com:19302,stun.cloudflare.com:3478" );
	utils->SetGlobalCallback_SteamNetConnectionStatusChanged( OnConnectionStatus );
	utils->SetGlobalCallback_SteamNetAuthenticationStatusChanged( OnAuthStatus );
	utils->SetGlobalCallback_SteamRelayNetworkStatusChanged( OnRelayStatus );
	utils->SetGlobalCallback_MessagesSessionRequest( OnMessagesRequest );
	utils->SetGlobalCallback_MessagesSessionFailed( OnMessagesFailed );
	shim_cm_subscribe( k_EMsgClientP2PConnectionInfo );
	shim_cm_on_push( k_EMsgClientP2PConnectionInfo, OnP2PConnectionInfo );
	shim_log( "GNS: ready (identity steamid:%llu, app %u)", (unsigned long long)g_steamid, g_appid );
	return 0;
}

void shim_gns_kill( void ) { GameNetworkingSockets_Kill(); }

static DWORD WINAPI CredentialsLoop( LPVOID )
{
	for ( ;; )
	{
		LoadSdrConfig( g_appid );
		ISteamNetworkingSockets *sock = SteamNetworkingSockets();
		bool ok = sock && ProvisionCertificate( sock );
		Sleep( ok ? 24u * 3600u * 1000u : 60u * 1000u );
	}
}

static DWORD WINAPI StartThread( LPVOID arg )
{
	int rc = shim_gns_start( (unsigned)(uintptr_t)arg );
	InterlockedExchange( &g_state, rc == 0 ? 2 : 3 );
	if ( rc ) { shim_log( "GNS: failed to start (%d); the shim uses its own P2P", rc ); return 0; }
	CredentialsLoop( nullptr );
	return 0;
}

void shim_gns_start_async( unsigned appid )
{
	static volatile LONG once;
	if ( InterlockedExchange( &once, 1 ) ) return;
	InterlockedExchange( &g_state, 1 );
	HANDLE t = CreateThread( nullptr, 0, StartThread, (LPVOID)(uintptr_t)appid, 0, nullptr );
	if ( t ) CloseHandle( t );
}

}

extern "C" {

int shim_gns_use( void )
{
	LONG st = g_state;
	if ( st == 0 ) return 0;
	for ( int i = 0; st == 1 && i < 400; ++i ) { Sleep( 50 ); st = g_state; }
	return st == 2;
}

void shim_gns_pump( void )
{
	if ( g_state != 2 ) return;
	if ( ISteamNetworkingSockets *s = Sock() ) s->RunCallbacks();
}

unsigned shim_gns_listen_ip( const void *addr, int n, const void *opts ) { if ( !addr ) return 0; return Sock()->CreateListenSocketIP( *(const SteamNetworkingIPAddr *)addr, n, (const SteamNetworkingConfigValue_t *)opts ); }
unsigned shim_gns_connect_ip( const void *addr, int n, const void *opts ) { if ( !addr ) return 0; return Sock()->ConnectByIPAddress( *(const SteamNetworkingIPAddr *)addr, n, (const SteamNetworkingConfigValue_t *)opts ); }
unsigned shim_gns_listen_p2p( int vport, int n, const void *opts ) { return Sock()->CreateListenSocketP2P( vport, n, (const SteamNetworkingConfigValue_t *)opts ); }
unsigned shim_gns_connect_p2p( const void *identity, int vport, int n, const void *opts ) { if ( !identity ) return 0; return Sock()->ConnectP2P( Id( identity ), vport, n, (const SteamNetworkingConfigValue_t *)opts ); }
int shim_gns_accept( unsigned conn ) { return (int)Sock()->AcceptConnection( conn ); }
int shim_gns_close_conn( unsigned conn, int reason, const char *debug, int linger ) { return Sock()->CloseConnection( conn, reason, debug, linger != 0 ) ? 1 : 0; }
int shim_gns_close_listen( unsigned l ) { return Sock()->CloseListenSocket( l ) ? 1 : 0; }
int shim_gns_set_user_data( unsigned conn, int64_t v ) { return Sock()->SetConnectionUserData( conn, v ) ? 1 : 0; }
int64_t shim_gns_get_user_data( unsigned conn ) { return Sock()->GetConnectionUserData( conn ); }
void shim_gns_set_name( unsigned conn, const char *name ) { Sock()->SetConnectionName( conn, name ? name : "" ); }
int shim_gns_get_name( unsigned conn, char *out, int cap ) { if ( !out || cap <= 0 ) return 0; return Sock()->GetConnectionName( conn, out, cap ) ? 1 : 0; }
int shim_gns_send( unsigned conn, const void *data, unsigned cb, int flags, int64_t *number ) { if ( !data && cb ) return 2;  return (int)Sock()->SendMessageToConnection( conn, data, cb, flags, (int64 *)number ); }
void shim_gns_send_messages( int n, void *const *msgs, int64_t *results ) { Sock()->SendMessages( n, (SteamNetworkingMessage_t **)msgs, (int64 *)results, true );  }
int shim_gns_flush( unsigned conn ) { return (int)Sock()->FlushMessagesOnConnection( conn ); }
int shim_gns_receive( unsigned conn, void **out, int max ) { if ( !out && max > 0 ) return -1; return Sock()->ReceiveMessagesOnConnection( conn, (SteamNetworkingMessage_t **)out, max ); }
int shim_gns_receive_group( unsigned group, void **out, int max ) { if ( !out && max > 0 ) return -1; return Sock()->ReceiveMessagesOnPollGroup( group, (SteamNetworkingMessage_t **)out, max ); }
int shim_gns_get_info( unsigned conn, void *info ) { if ( !info ) return 0; return Sock()->GetConnectionInfo( conn, (SteamNetConnectionInfo_t *)info ) ? 1 : 0; }
int shim_gns_realtime( unsigned conn, void *status, int lanes, void *lane_status ) { return (int)Sock()->GetConnectionRealTimeStatus( conn, (SteamNetConnectionRealTimeStatus_t *)status, lanes, (SteamNetConnectionRealTimeLaneStatus_t *)lane_status ); }
int shim_gns_detailed( unsigned conn, char *out, int cap ) { if ( !out && cap > 0 ) return -1; return Sock()->GetDetailedConnectionStatus( conn, out, cap ); }
int shim_gns_listen_address( unsigned l, void *addr ) { if ( !addr ) return 0; return Sock()->GetListenSocketAddress( l, (SteamNetworkingIPAddr *)addr ) ? 1 : 0; }
int shim_gns_socket_pair( unsigned *a, unsigned *b, int loopback, const void *ida, const void *idb )
{
	if ( !a || !b ) return 0;
	const SteamNetworkingIdentity *ids[2] = { (const SteamNetworkingIdentity *)ida, (const SteamNetworkingIdentity *)idb };
	return Sock()->CreateSocketPair( (HSteamNetConnection *)a, (HSteamNetConnection *)b, loopback != 0, ids[0], ids[1] ) ? 1 : 0;
}
int shim_gns_get_identity( void *id ) { if ( !id ) return 0; return Sock()->GetIdentity( (SteamNetworkingIdentity *)id ) ? 1 : 0; }
int shim_gns_init_auth( void ) { return (int)Sock()->InitAuthentication(); }
int shim_gns_auth_status( void *out ) { return (int)Sock()->GetAuthenticationStatus( (SteamNetAuthenticationStatus_t *)out ); }
unsigned shim_gns_create_group( void ) { return Sock()->CreatePollGroup(); }
int shim_gns_destroy_group( unsigned g ) { return Sock()->DestroyPollGroup( g ) ? 1 : 0; }
int shim_gns_set_group( unsigned conn, unsigned group ) { return Sock()->SetConnectionPollGroup( conn, group ) ? 1 : 0; }
void shim_gns_run_callbacks( void ) { Sock()->RunCallbacks(); }

int shim_gns_msg_send( const void *identity, const void *data, unsigned cb, int flags, int channel ) { if ( !identity || ( !data && cb ) ) return 2; return (int)Msgs()->SendMessageToUser( Id( identity ), data, cb, flags, channel ); }
int shim_gns_msg_receive( int channel, void **out, int max ) { if ( !out && max > 0 ) return 0; return Msgs()->ReceiveMessagesOnChannel( channel, (SteamNetworkingMessage_t **)out, max ); }
int shim_gns_msg_accept( const void *identity ) { if ( !identity ) return 0; return Msgs()->AcceptSessionWithUser( Id( identity ) ) ? 1 : 0; }
int shim_gns_msg_close_session( const void *identity ) { if ( !identity ) return 0; return Msgs()->CloseSessionWithUser( Id( identity ) ) ? 1 : 0; }
int shim_gns_msg_close_channel( const void *identity, int channel ) { if ( !identity ) return 0; return Msgs()->CloseChannelWithUser( Id( identity ), channel ) ? 1 : 0; }
int shim_gns_msg_session_info( const void *identity, void *info, void *status ) { if ( !identity ) return 0; return (int)Msgs()->GetSessionConnectionInfo( Id( identity ), (SteamNetConnectionInfo_t *)info, (SteamNetConnectionRealTimeStatus_t *)status ); }

int shim_gns_relay_status( void *out ) { return (int)SteamNetworkingUtils()->GetRelayNetworkStatus( (SteamRelayNetworkStatus_t *)out ); }
void shim_gns_init_relay( void ) { SteamNetworkingUtils()->InitRelayNetworkAccess(); }
void *shim_gns_allocate_message( int cb ) { return SteamNetworkingUtils()->AllocateMessage( cb ); }

}

namespace
{
	struct Bridge { void **vtbl; void *target; };
	const int k_nSockets012Slots = 48;
	const int k_iSendMessages = 12;
	Bridge g_sockets012;
	void *g_sockets012_vtbl[k_nSockets012Slots];

#ifdef _WIN64
	const size_t k_cbThunk = 16;
	void WriteThunk( unsigned char *p, void *target )
	{
		static const unsigned char head[] = { 0x48, 0x8B, 0x49, 0x08, 0x48, 0xB8 };
		memcpy( p, head, sizeof head ); memcpy( p + 6, &target, 8 ); p[14] = 0xFF; p[15] = 0xE0;
	}
	#define SHIM_TC_CXX
#else
	const size_t k_cbThunk = 12;
	void WriteThunk( unsigned char *p, void *target )
	{
		static const unsigned char head[] = { 0x8B, 0x49, 0x04, 0xB8 };
		memcpy( p, head, sizeof head ); memcpy( p + 4, &target, 4 ); p[8] = 0xFF; p[9] = 0xE0; p[10] = p[11] = 0xCC;
	}
	#define SHIM_TC_CXX __attribute__((thiscall))
#endif

	void SHIM_TC_CXX SendMessages012( Bridge *self, int n, SteamNetworkingMessage_t **msgs, int64 *results )
	{
		static_cast<ISteamNetworkingSockets *>( self->target )->SendMessages( n, msgs, results, true );
	}

	bool BuildSockets012()
	{
		void *target = Sock();
		if ( !target ) return false;
		void **real = *(void ***)target;
		unsigned char *code = (unsigned char *)VirtualAlloc( nullptr, k_cbThunk * k_nSockets012Slots, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE );
		if ( !code ) return false;
		for ( int i = 0; i < k_nSockets012Slots; ++i )
		{
			WriteThunk( code + i * k_cbThunk, real[i] );
			g_sockets012_vtbl[i] = code + i * k_cbThunk;
		}
		g_sockets012_vtbl[k_iSendMessages] = (void *)&SendMessages012;
		DWORD old;
		VirtualProtect( code, k_cbThunk * k_nSockets012Slots, PAGE_EXECUTE_READ, &old );
		FlushInstructionCache( GetCurrentProcess(), code, k_cbThunk * k_nSockets012Slots );
		g_sockets012.target = target;
		g_sockets012.vtbl = g_sockets012_vtbl;
		return true;
	}
}

extern "C" {

void *shim_gns_native( const char *version )
{
	if ( !version || !shim_gns_use() ) return nullptr;
	if ( !strcmp( version, "SteamNetworkingSockets013" ) ) return Sock();
	if ( !strcmp( version, "SteamNetworkingSockets012" ) )
	{
		static volatile LONG built;
		if ( InterlockedCompareExchange( &built, 1, 0 ) == 0 ) InterlockedExchange( &built, BuildSockets012() ? 2 : 3 );
		while ( built == 1 ) Sleep( 1 );
		return built == 2 ? &g_sockets012 : nullptr;
	}
	if ( !strcmp( version, "SteamNetworkingMessages002" ) ) return Msgs();
	if ( !strcmp( version, "SteamNetworkingUtils004" ) ) return SteamNetworkingUtils();
	return nullptr;
}

}
