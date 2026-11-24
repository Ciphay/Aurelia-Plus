#include <steam/steamnetworkingsockets.h>
#include <steam/isteamnetworkingutils.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>
#include <fstream>
#include <thread>
#include <chrono>

extern "C" void GameNetworkingSockets_SetAppID( unsigned nAppID );
namespace SteamNetworkingSocketsLib { bool CertStore_AddCertFromBase64( const char *pszBase64, SteamNetworkingErrMsg &errMsg ); }

static std::string g_spew;
static void Spew( ESteamNetworkingSocketsDebugOutputType t, const char *msg ) { g_spew += std::to_string( (int)t ) + ": " + msg + "\n"; }

static std::vector<unsigned char> B64( const std::string &s )
{
	std::vector<unsigned char> out; unsigned v = 0; int bits = 0;
	for ( char c : s )
	{
		int d = c >= 'A' && c <= 'Z' ? c - 'A' : c >= 'a' && c <= 'z' ? c - 'a' + 26 : c >= '0' && c <= '9' ? c - '0' + 52 : c == '+' ? 62 : c == '/' ? 63 : -1;
		if ( d < 0 ) continue;
		v = ( v << 6 ) | d; bits += 6;
		if ( bits >= 8 ) { bits -= 8; out.push_back( ( v >> bits ) & 0xff ); }
	}
	return out;
}

int main( int argc, char **argv )
{
	if ( argc < 5 ) { fprintf( stderr, "usage: %s <pub_out> <signed_b64_in> <certs_in> <result_out>\n", argv[0] ); return 2; }
	std::string res;
	SteamNetworkingErrMsg err;
	if ( !GameNetworkingSockets_Init( nullptr, err ) ) { fprintf( stderr, "Init failed: %s\n", err ); return 3; }
	if ( argc > 5 ) GameNetworkingSockets_SetAppID( (unsigned)atoi( argv[5] ) );
	SteamNetworkingUtils()->SetDebugOutputFunction( k_ESteamNetworkingSocketsDebugOutputType_Warning, Spew );

	int added = 0, failed = 0;
	{
		std::ifstream f( argv[3] ); std::string line;
		while ( std::getline( f, line ) )
		{
			if ( line.empty() ) continue;
			if ( SteamNetworkingSocketsLib::CertStore_AddCertFromBase64( line.c_str(), err ) ) ++added; else { ++failed; res += std::string( "cert rejected: " ) + err + "\n"; }
		}
	}
	res += "certificates added: " + std::to_string( added ) + ", rejected: " + std::to_string( failed ) + "\n";

	ISteamNetworkingSockets *sock = SteamNetworkingSockets();
	unsigned char blob[1024]; int cb = sizeof blob;
	if ( !sock->GetCertificateRequest( &cb, blob, err ) ) { fprintf( stderr, "GetCertificateRequest failed: %s\n", err ); return 4; }
	int off = -1;
	for ( int i = 0; i + 34 <= cb; ++i ) if ( blob[i] == 0x12 && blob[i + 1] == 0x20 ) { off = i + 2; break; }
	if ( off < 0 ) { fprintf( stderr, "public key not found in the request\n" ); return 5; }
	{
		std::ofstream o( argv[1] ); char h[3];
		for ( int i = 0; i < 32; ++i ) { snprintf( h, sizeof h, "%02x", blob[off + i] ); o << h; }
		o << "\n";
	}
	res += "certificate request of " + std::to_string( cb ) + " bytes\n";

	std::string signed_b64;
	for ( int i = 0; i < 600 && signed_b64.empty(); ++i )
	{
		std::ifstream f( argv[2] ); std::getline( f, signed_b64 );
		if ( signed_b64.size() < 20 ) { signed_b64.clear(); std::this_thread::sleep_for( std::chrono::milliseconds( 200 ) ); }
	}
	if ( signed_b64.empty() ) { fprintf( stderr, "no signed certificate\n" ); return 6; }
	std::vector<unsigned char> bin = B64( signed_b64 );
	g_spew.clear();
	bool ok = sock->SetCertificate( bin.data(), (int)bin.size(), err );
	res += std::string( "SetCertificate: " ) + ( ok ? "ok" : "FAILED" ) + ( ok ? "" : std::string( " (" ) + err + ")" ) + "\n";
	res += g_spew.empty() ? "library warnings: none (the chain was accepted)\n" : "library warnings:\n" + g_spew;
	SteamNetworkingIdentity id; sock->GetIdentity( &id ); char ids[SteamNetworkingIdentity::k_cchMaxString]; id.ToString( ids, sizeof ids );
	res += std::string( "identity: " ) + ids + "\n";
	std::ofstream( argv[4] ) << res; fputs( res.c_str(), stdout );
	GameNetworkingSockets_Kill();
	return ok ? 0 : 1;
}
