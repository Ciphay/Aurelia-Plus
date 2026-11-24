#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>
#include <steam/isteamnetworkingsockets.h>

extern "C" {
#include "conformance/probe_table.h"
}

typedef void *( *CreateInterfaceFn )( const char *, int * );
static FILE *g_out;
#define POUT( ... ) do { fprintf( g_out, __VA_ARGS__ ); fflush( g_out ); } while ( 0 )

#ifdef _WIN64
#define TC
#else
#define TC __attribute__((thiscall))
#endif

static int slot_of( const char *iface, const char *method )
{
	for ( int i = 0; i < PROBE_COUNT; i++ )
		if ( !strcmp( g_probe[i].iface, iface ) )
			for ( int k = 0; k < g_probe[i].n; k++ ) if ( !strcmp( g_probe[i].methods[k], method ) ) return k;
	fprintf( stderr, "no slot %s.%s\n", iface, method );
	exit( 9 );
}
template <typename F> static F M( void *obj, const char *iface, const char *method ) { return (F)( ( *(void ***)obj )[slot_of( iface, method )] ); }

static const char *S = "SteamNetworkingSockets012";
static void *g_sock, *g_utils, *g_msgs;

static volatile HSteamNetConnection g_incoming;
static HSteamNetConnection g_conn[64]; static int g_state[64], g_n;
static int *StateOf( HSteamNetConnection c )
{
	for ( int i = 0; i < g_n; i++ ) if ( g_conn[i] == c ) return &g_state[i];
	if ( g_n == 64 ) return &g_state[63];
	g_conn[g_n] = c; g_state[g_n] = 0; return &g_state[g_n++];
}
static void OnStatus( SteamNetConnectionStatusChangedCallback_t *p )
{
	if ( p->m_info.m_eState == k_ESteamNetworkingConnectionState_Connecting && p->m_info.m_hListenSocket && !g_incoming )
	{
		g_incoming = p->m_hConn;
		int r = M<int( TC * )( void *, HSteamNetConnection )>( g_sock, S, "AcceptConnection" )( g_sock, p->m_hConn );
		POUT( "  accepted %u (%d) from %s\n", p->m_hConn, r, p->m_info.m_szConnectionDescription );
	}
	*StateOf( p->m_hConn ) = p->m_info.m_eState;
	POUT( "  connection %u: %d -> %d %s\n", p->m_hConn, p->m_eOldState, p->m_info.m_eState, p->m_info.m_szEndDebug );
}

static void Pump( int ms )
{
	for ( int t = 0; t < ms; t += 10 ) { M<void( TC * )( void * )>( g_sock, S, "RunCallbacks" )( g_sock ); Sleep( 10 ); }
}

static int WaitState( HSteamNetConnection c, int want, int ms )
{
	for ( int t = 0; t < ms; t += 20 )
	{
		M<void( TC * )( void * )>( g_sock, S, "RunCallbacks" )( g_sock );
		int st = *StateOf( c );
		if ( st == want ) return 1;
		if ( st == k_ESteamNetworkingConnectionState_ClosedByPeer || st == k_ESteamNetworkingConnectionState_ProblemDetectedLocally ) return 0;
		Sleep( 20 );
	}
	return 0;
}

static int SendText( HSteamNetConnection c, const char *txt )
{
	int64 n = 0;
	return M<int( TC * )( void *, HSteamNetConnection, const void *, uint32, int, int64 * )>( g_sock, S, "SendMessageToConnection" )( g_sock, c, txt, (uint32)strlen( txt ) + 1, k_nSteamNetworkingSend_Reliable, &n );
}

static void SendViaBatch( HSteamNetConnection c, const char *txt )
{
	SteamNetworkingMessage_t *m = M<SteamNetworkingMessage_t *( TC * )( void *, int )>( g_utils, "SteamNetworkingUtils004", "AllocateMessage" )( g_utils, (int)strlen( txt ) + 1 );
	memcpy( m->m_pData, txt, strlen( txt ) + 1 );
	m->m_conn = c;
	m->m_nFlags = k_nSteamNetworkingSend_Reliable;
	int64 r = 0;
	M<void( TC * )( void *, int, SteamNetworkingMessage_t *const *, int64 * )>( g_sock, S, "SendMessages" )( g_sock, 1, &m, &r );
	POUT( "  SendMessages(012): result %lld\n", (long long)r );
}

static int Expect( HSteamNetConnection c, const char *txt, int ms )
{
	for ( int t = 0; t < ms; t += 20 )
	{
		SteamNetworkingMessage_t *m[4];
		int n = M<int( TC * )( void *, HSteamNetConnection, SteamNetworkingMessage_t **, int )>( g_sock, S, "ReceiveMessagesOnConnection" )( g_sock, c, m, 4 );
		for ( int i = 0; i < n; i++ )
		{
			int ok = m[i]->m_cbSize == (int)strlen( txt ) + 1 && !memcmp( m[i]->m_pData, txt, m[i]->m_cbSize );
			POUT( "  received on %u: \"%.*s\"\n", c, m[i]->m_cbSize, (const char *)m[i]->m_pData );
			m[i]->m_pfnRelease( m[i] );
			if ( ok ) return 1;
		}
		Pump( 20 );
	}
	return 0;
}

static SteamNetworkingConfigValue_t StatusOpt()
{
	SteamNetworkingConfigValue_t o;
	o.m_eValue = k_ESteamNetworkingConfig_Callback_ConnectionStatusChanged;
	o.m_eDataType = k_ESteamNetworkingConfig_Ptr;
	o.m_val.m_ptr = (void *)OnStatus;
	return o;
}

static int Exchange( const char *what, HSteamNetConnection client )
{
	if ( !g_incoming ) { POUT( "FAIL %s: the server did not see the connection\n", what ); return 0; }
	HSteamNetConnection server = g_incoming;
	if ( !WaitState( client, k_ESteamNetworkingConnectionState_Connected, 30000 ) ) { POUT( "FAIL %s: client did not connect\n", what ); return 0; }
	if ( !WaitState( server, k_ESteamNetworkingConnectionState_Connected, 10000 ) ) { POUT( "FAIL %s: server did not connect\n", what ); return 0; }
	POUT( "  %s: connected (client %u, server %u)\n", what, client, server );
	char detail[2048];
	if ( M<int( TC * )( void *, HSteamNetConnection, char *, int )>( g_sock, S, "GetDetailedConnectionStatus" )( g_sock, client, detail, sizeof detail ) == 0 )
		POUT( "  --- detailed state ---\n%s  ---\n", detail );
	SendText( client, "hello from the client" );
	if ( !Expect( server, "hello from the client", 10000 ) ) { POUT( "FAIL %s: server did not receive\n", what ); return 0; }
	SendViaBatch( server, "hello from the server" );
	if ( !Expect( client, "hello from the server", 10000 ) ) { POUT( "FAIL %s: client did not receive\n", what ); return 0; }
	M<int( TC * )( void *, HSteamNetConnection, int, const char *, bool )>( g_sock, S, "CloseConnection" )( g_sock, client, 0, "end of the test", false );
	M<int( TC * )( void *, HSteamNetConnection, int, const char *, bool )>( g_sock, S, "CloseConnection" )( g_sock, server, 0, "end of the test", false );
	POUT( "OK %s\n", what );
	return 1;
}

static int PairServer( const SteamNetworkingIdentity &me, const char *exe, const char *out )
{
	SteamNetworkingConfigValue_t opt = StatusOpt();
	g_incoming = 0;
	HSteamListenSocket lp = M<HSteamListenSocket( TC * )( void *, int, int, const SteamNetworkingConfigValue_t * )>( g_sock, S, "CreateListenSocketP2P" )( g_sock, 7, 1, &opt );
	char child_out[600];
	snprintf( child_out, sizeof child_out, "%s.client", out );
	SetEnvironmentVariableA( "PROBE_ROLE", "client" );
	SetEnvironmentVariableA( "PROBE_OUT", child_out );
	SetEnvironmentVariableA( "AURELIA_SHIM_GNS_TEST_NO_LOOPBACK", "1" );
	STARTUPINFOA si = { sizeof si }; PROCESS_INFORMATION pi = {};
	char cmd[600]; snprintf( cmd, sizeof cmd, "\"%s\"", exe );
	if ( !CreateProcessA( exe, cmd, nullptr, nullptr, FALSE, 0, nullptr, nullptr, &si, &pi ) ) { POUT( "FAIL peer: CreateProcess %lu\n", GetLastError() ); return 0; }
	POUT( "peer: P2P listen %u; child %lu connecting through Steam\n", lp, pi.dwProcessId );
	for ( int t = 0; t < 60000 && !g_incoming; t += 50 ) Pump( 50 );
	int ok = 0;
	if ( !g_incoming ) POUT( "FAIL peer: the connection request did not arrive through Steam\n" );
	else if ( !WaitState( g_incoming, k_ESteamNetworkingConnectionState_Connected, 45000 ) ) POUT( "FAIL peer: server did not connect\n" );
	else if ( !Expect( g_incoming, "hello from the client", 15000 ) ) POUT( "FAIL peer: server did not receive\n" );
	else { SendViaBatch( g_incoming, "hello from the server" ); ok = 1; }
	const char *M2 = "SteamNetworkingMessages002";
	int got = 0;
	for ( int t = 0; ok && t < 45000 && !got; t += 50 )
	{
		M<int( TC * )( void *, const SteamNetworkingIdentity & )>( g_msgs, M2, "AcceptSessionWithUser" )( g_msgs, me );
		SteamNetworkingMessage_t *m[4];
		int n = M<int( TC * )( void *, int, SteamNetworkingMessage_t **, int )>( g_msgs, M2, "ReceiveMessagesOnChannel" )( g_msgs, 3, m, 4 );
		for ( int i = 0; i < n; i++ ) { got |= !strcmp( (const char *)m[i]->m_pData, "hello via Messages" ); POUT( "  peer: Messages received \"%s\"\n", (const char *)m[i]->m_pData ); m[i]->m_pfnRelease( m[i] ); }
		Pump( 50 );
	}
	if ( ok && !got ) POUT( "FAIL peer: Messages did not arrive\n" );
	WaitForSingleObject( pi.hProcess, 60000 );
	DWORD code = 99; GetExitCodeProcess( pi.hProcess, &code );
	POUT( "peer: child finished (%lu)\n", code );
	FILE *f = fopen( child_out, "r" );
	if ( f ) { char line[512]; while ( fgets( line, sizeof line, f ) ) POUT( "  [child] %s", line ); fclose( f ); }
	M<int( TC * )( void *, HSteamListenSocket )>( g_sock, S, "CloseListenSocket" )( g_sock, lp );
	ok = ok && got && code == 0;
	POUT( ok ? "OK P2P between processes\n" : "FAIL P2P between processes\n" );
	return ok;
}

static int PairClient( const SteamNetworkingIdentity &me )
{
	SteamNetworkingConfigValue_t opt = StatusOpt();
	HSteamNetConnection cp = M<HSteamNetConnection( TC * )( void *, const SteamNetworkingIdentity &, int, int, const SteamNetworkingConfigValue_t * )>( g_sock, S, "ConnectP2P" )( g_sock, me, 7, 1, &opt );
	POUT( "connecting %u\n", cp );
	if ( !WaitState( cp, k_ESteamNetworkingConnectionState_Connected, 45000 ) ) { POUT( "FAIL client did not connect\n" ); return 0; }
	char detail[2048];
	if ( M<int( TC * )( void *, HSteamNetConnection, char *, int )>( g_sock, S, "GetDetailedConnectionStatus" )( g_sock, cp, detail, sizeof detail ) == 0 )
		POUT( "--- detailed state ---\n%s---\n", detail );
	SendText( cp, "hello from the client" );
	if ( !Expect( cp, "hello from the server", 15000 ) ) { POUT( "FAIL client did not receive\n" ); return 0; }
	const char *txt = "hello via Messages";
	int r = M<int( TC * )( void *, const SteamNetworkingIdentity &, const void *, uint32, int, int )>( g_msgs, "SteamNetworkingMessages002", "SendMessageToUser" )( g_msgs, me, txt, (uint32)strlen( txt ) + 1, k_nSteamNetworkingSend_Reliable, 3 );
	POUT( "SendMessageToUser = %d\n", r );
	Pump( 8000 );
	M<int( TC * )( void *, HSteamNetConnection, int, const char *, bool )>( g_sock, S, "CloseConnection" )( g_sock, cp, 0, "end of the test", true );
	Pump( 500 );
	POUT( "OK client\n" );
	return 1;
}

int main()
{
	char dll[512], out[512];
	if ( !GetEnvironmentVariableA( "PROBE_DLL", dll, sizeof dll ) || !GetEnvironmentVariableA( "PROBE_OUT", out, sizeof out ) ) return 2;
	g_out = fopen( out, "w" );
	if ( !g_out ) return 2;
	HMODULE h = LoadLibraryA( dll );
	if ( !h ) { POUT( "FAIL load %lu\n", GetLastError() ); return 3; }
	CreateInterfaceFn ci = (CreateInterfaceFn)GetProcAddress( h, "CreateInterface" );
	const char *cv = "SteamClient023";
	void *client = ci( cv, nullptr );
	int pipe = M<int( TC * )( void * )>( client, cv, "CreateSteamPipe" )( client );
	int user = M<int( TC * )( void *, int )>( client, cv, "ConnectToGlobalUser" )( client, pipe );
	auto gen = M<void *( TC * )( void *, int, int, const char * )>( client, cv, "GetISteamGenericInterface" );
	g_sock = gen( client, user, pipe, S );
	g_utils = gen( client, user, pipe, "SteamNetworkingUtils004" );
	g_msgs = gen( client, user, pipe, "SteamNetworkingMessages002" );
	POUT( "interfaces: sockets %p utils %p messages %p\n", g_sock, g_utils, g_msgs );
	if ( !g_sock || !g_utils || !g_msgs ) { POUT( "FAIL interfaces\n" ); return 4; }

	char role[16];
	if ( GetEnvironmentVariableA( "PROBE_ROLE", role, sizeof role ) && !strcmp( role, "client" ) )
	{
		SteamNetworkingIdentity me;
		for ( int t = 0; t < 30000; t += 250 )
		{
			SteamNetAuthenticationStatus_t auth = {};
			if ( M<int( TC * )( void *, SteamNetAuthenticationStatus_t * )>( g_sock, S, "GetAuthenticationStatus" )( g_sock, &auth ) == k_ESteamNetworkingAvailability_Current ) break;
			Pump( 250 );
		}
		M<int( TC * )( void *, SteamNetworkingIdentity * )>( g_sock, S, "GetIdentity" )( g_sock, &me );
		int ok = PairClient( me );
		fclose( g_out );
		return ok ? 0 : 1;
	}

	int fails = 0;
	SteamNetworkingConfigValue_t opt = StatusOpt();
	SteamNetworkingIPAddr addr; addr.Clear(); addr.SetIPv4( 0x7f000001, 27555 );
	HSteamListenSocket ls = M<HSteamListenSocket( TC * )( void *, const SteamNetworkingIPAddr &, int, const SteamNetworkingConfigValue_t * )>( g_sock, S, "CreateListenSocketIP" )( g_sock, addr, 1, &opt );
	HSteamNetConnection c = M<HSteamNetConnection( TC * )( void *, const SteamNetworkingIPAddr &, int, const SteamNetworkingConfigValue_t * )>( g_sock, S, "ConnectByIPAddress" )( g_sock, addr, 1, &opt );
	POUT( "IP: listen %u, connection %u\n", ls, c );
	Pump( 500 );
	fails += !Exchange( "IP", c );
	M<int( TC * )( void *, HSteamListenSocket )>( g_sock, S, "CloseListenSocket" )( g_sock, ls );

	char skip[8];
	if ( !( GetEnvironmentVariableA( "PROBE_SKIP_P2P", skip, sizeof skip ) && skip[0] == '1' ) )
	{
		SteamNetAuthenticationStatus_t auth = {};
		int avail = 0;
		for ( int t = 0; t < 30000; t += 250 )
		{
			avail = M<int( TC * )( void *, SteamNetAuthenticationStatus_t * )>( g_sock, S, "GetAuthenticationStatus" )( g_sock, &auth );
			if ( avail == k_ESteamNetworkingAvailability_Current ) break;
			Pump( 250 );
		}
		POUT( "certificate: %d %s\n", avail, auth.m_debugMsg );
		SteamNetworkingIdentity me;
		M<int( TC * )( void *, SteamNetworkingIdentity * )>( g_sock, S, "GetIdentity" )( g_sock, &me );
		g_incoming = 0;
		HSteamListenSocket lp = M<HSteamListenSocket( TC * )( void *, int, int, const SteamNetworkingConfigValue_t * )>( g_sock, S, "CreateListenSocketP2P" )( g_sock, 7, 1, &opt );
		HSteamNetConnection cp = M<HSteamNetConnection( TC * )( void *, const SteamNetworkingIdentity &, int, int, const SteamNetworkingConfigValue_t * )>( g_sock, S, "ConnectP2P" )( g_sock, me, 7, 1, &opt );
		POUT( "P2P: listen %u, connection %u for steamid %llu\n", lp, cp, (unsigned long long)me.GetSteamID64() );
		for ( int t = 0; t < 30000 && !g_incoming; t += 50 ) Pump( 50 );
		fails += !Exchange( "P2P", cp );
		M<int( TC * )( void *, HSteamListenSocket )>( g_sock, S, "CloseListenSocket" )( g_sock, lp );

		const char *M2 = "SteamNetworkingMessages002";
		const char *txt = "hello via Messages";
		int r = M<int( TC * )( void *, const SteamNetworkingIdentity &, const void *, uint32, int, int )>( g_msgs, M2, "SendMessageToUser" )( g_msgs, me, txt, (uint32)strlen( txt ) + 1, k_nSteamNetworkingSend_Reliable, 3 );
		POUT( "Messages: SendMessageToUser = %d\n", r );
		int got = 0;
		for ( int t = 0; t < 30000 && !got; t += 50 )
		{
			M<int( TC * )( void *, const SteamNetworkingIdentity & )>( g_msgs, M2, "AcceptSessionWithUser" )( g_msgs, me );
			SteamNetworkingMessage_t *m[4];
			int n = M<int( TC * )( void *, int, SteamNetworkingMessage_t **, int )>( g_msgs, M2, "ReceiveMessagesOnChannel" )( g_msgs, 3, m, 4 );
			for ( int i = 0; i < n; i++ ) { got |= !strcmp( (const char *)m[i]->m_pData, txt ); POUT( "  Messages received: \"%s\"\n", (const char *)m[i]->m_pData ); m[i]->m_pfnRelease( m[i] ); }
			Pump( 50 );
		}
		POUT( got ? "OK Messages\n" : "FAIL Messages\n" );

		char exe[512];
		GetModuleFileNameA( nullptr, exe, sizeof exe );
		fails += !PairServer( me, exe, out );
		fails += !got;
	}
	POUT( "END %d failures\n", fails );
	fclose( g_out );
	return fails;
}
