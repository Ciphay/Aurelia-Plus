#include <stdio.h>
#include <string.h>
#include <time.h>
#include <windows.h>
#include <bcrypt.h>

#include "shim.h"

#ifndef NT_SUCCESS
#define NT_SUCCESS(s) ((s) >= 0)
#endif

static const uint8_t VALVE_MODULUS[128] = {
    0xdf, 0xec, 0x1a, 0xd6, 0x2c, 0x10, 0x66, 0x2c, 0x17, 0x35, 0x3a, 0x14, 0xb0, 0x7c, 0x59, 0x11,
    0x7f, 0x9d, 0xd3, 0xd8, 0x2b, 0x7a, 0xe3, 0xe0, 0x15, 0xcd, 0x19, 0x1e, 0x46, 0xe8, 0x7b, 0x87,
    0x74, 0xa2, 0x18, 0x46, 0x31, 0xa9, 0x03, 0x14, 0x79, 0x82, 0x8e, 0xe9, 0x45, 0xa2, 0x49, 0x12,
    0xa9, 0x23, 0x68, 0x73, 0x89, 0xcf, 0x69, 0xa1, 0xb1, 0x61, 0x46, 0xbd, 0xc1, 0xbe, 0xbf, 0xd6,
    0x01, 0x1b, 0xd8, 0x81, 0xd4, 0xdc, 0x90, 0xfb, 0xfe, 0x4f, 0x52, 0x73, 0x66, 0xcb, 0x95, 0x70,
    0xd7, 0xc5, 0x8e, 0xba, 0x1c, 0x7a, 0x33, 0x75, 0xa1, 0x62, 0x34, 0x46, 0xbb, 0x60, 0xb7, 0x80,
    0x68, 0xfa, 0x13, 0xa7, 0x7a, 0x8a, 0x37, 0x4b, 0x9e, 0xc6, 0xf4, 0x5d, 0x5f, 0x3a, 0x99, 0xf9,
    0x9e, 0xc4, 0x3a, 0xe9, 0x63, 0xa2, 0xbb, 0x88, 0x19, 0x28, 0xe0, 0xe7, 0x14, 0xc0, 0x42, 0x89,
};
static const uint8_t VALVE_EXPONENT = 17;

static bool valve_signature_ok(const uint8_t *msg, size_t msg_len, const uint8_t *sig) {
    BCRYPT_ALG_HANDLE rsa = NULL, sha = NULL;
    BCRYPT_KEY_HANDLE key = NULL;
    bool ok = false;
    uint8_t blob[sizeof(BCRYPT_RSAKEY_BLOB) + 1 + 128];
    BCRYPT_RSAKEY_BLOB *h = (BCRYPT_RSAKEY_BLOB *)blob;
    uint8_t digest[20];
    memset(blob, 0, sizeof blob);
    h->Magic = BCRYPT_RSAPUBLIC_MAGIC;
    h->BitLength = 1024;
    h->cbPublicExp = 1;
    h->cbModulus = 128;
    blob[sizeof *h] = VALVE_EXPONENT;
    memcpy(blob + sizeof *h + 1, VALVE_MODULUS, 128);
    if (!NT_SUCCESS(BCryptOpenAlgorithmProvider(&sha, BCRYPT_SHA1_ALGORITHM, NULL, 0))) goto done;
    if (!NT_SUCCESS(BCryptHash(sha, NULL, 0, (PUCHAR)msg, (ULONG)msg_len, digest, sizeof digest))) goto done;
    if (!NT_SUCCESS(BCryptOpenAlgorithmProvider(&rsa, BCRYPT_RSA_ALGORITHM, NULL, 0))) goto done;
    if (!NT_SUCCESS(BCryptImportKeyPair(rsa, NULL, BCRYPT_RSAPUBLIC_BLOB, &key, blob, sizeof blob, 0))) goto done;
    BCRYPT_PKCS1_PADDING_INFO pad = { BCRYPT_SHA1_ALGORITHM };
    ok = NT_SUCCESS(BCryptVerifySignature(key, &pad, digest, sizeof digest, (PUCHAR)sig, 128, BCRYPT_PAD_PKCS1));
done:
    if (key) BCryptDestroyKey(key);
    if (rsa) BCryptCloseAlgorithmProvider(rsa, 0);
    if (sha) BCryptCloseAlgorithmProvider(sha, 0);
    return ok;
}

static bool take(const uint8_t *t, uint32_t cb, uint32_t *o, const uint8_t **part, uint32_t *len) {
    uint32_t n;
    if (*o + 4 > cb) return false;
    memcpy(&n, t + *o, 4);
    *o += 4;
    if (n > cb - *o) return false;
    *part = t + *o;
    *len = n;
    *o += n;
    return true;
}

int shim_verify_auth_ticket(const void *ticket, uint32_t cb, uint32_t appid, uint64_t expect_steamid, uint64_t *owner_out) {
    const uint8_t *t = ticket, *token, *header, *own;
    uint32_t o = 0, tl, hl, ol;
    if (owner_out) *owner_out = 0;
    if (!t || !take(t, cb, &o, &token, &tl) || tl < 16 || !take(t, cb, &o, &header, &hl) || !take(t, cb, &o, &own, &ol)) return 8;
    if (ol < 40 + 128) return 8;
    uint64_t token_id, own_id;
    uint32_t own_app, expires;
    memcpy(&token_id, token + 8, 8);
    memcpy(&own_id, own + 8, 8);
    memcpy(&own_app, own + 16, 4);
    memcpy(&expires, own + 36, 4);
    if (owner_out) *owner_out = own_id;
    if (!valve_signature_ok(own, ol - 128, own + ol - 128)) return 8;
    if (own_id != token_id || (expect_steamid && own_id != expect_steamid)) return 8;
    if (appid && own_app != appid) return 2;
    if (expires && (uint32_t)time(NULL) > expires) return 2;
    return 0;
}
