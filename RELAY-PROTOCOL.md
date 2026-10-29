# Relay protocol

How the Buwiz app sends a return to BIR through the Buwiz relay. This is
the protocol `crates/bir-submit` speaks. Everything before **Send**
(filling, checking, printing) happens on your device and uses none of it.

BIR's own upload needs passphrases and a login that ship inside eBIRForms.
They stay with the relay, never in this app. The upload itself still leaves
from your connection, as it does with eBIRForms: the relay asks the app to
carry encrypted bytes to BIR's hosts and nowhere else.

The relay has two roles:

- The **Gatekeeper** handles accounts, TIN claims, devices and tickets. It
  holds no BIR secret and can't open a sealed return.
- The **Sender** holds what BIR's upload needs and the key returns are
  sealed to. It checks every return again before anything goes to BIR.

## 1. Sign in and register the device

The app signs in with the OAuth device flow (RFC 8628): it shows a code,
and you approve it in a browser on your Buwiz account. The access token
goes in the OS key store.

The app then makes an Ed25519 key for this device, keeps it in the OS key
store (a private file on Android), and registers its public half:

```http
POST /api/devices
Authorization: Bearer <access token>
buwiz-timestamp: 1767225600
buwiz-nonce: 3q2-7wEAAAAAAAAAAAAAAA
buwiz-signature: <base64url Ed25519 signature, made with the new key>

{"public_key": "<base64url, 32 bytes>", "name": "Juana's laptop"}
```

The answer is `201 {"id", "name", "created_at"}`.

## 2. Signed requests

Every request after registration carries a signature from the device key,
so a stolen access token alone can't file anything:

| Header | |
|---|---|
| `buwiz-device` | The device id (absent only when registering) |
| `buwiz-timestamp` | Unix seconds. Refused when more than 60 s off. |
| `buwiz-nonce` | 16 to 64 characters of `[A-Za-z0-9_-]`, never reused |
| `buwiz-signature` | base64url Ed25519 signature over the signing input |

The signing input is these lines joined with `\n`:

```text
buwiz-request-v1
<METHOD>
<path and query, e.g. /api/tickets>
<lowercase hex SHA-256 of the body; of the empty string for a GET>
<timestamp>
<nonce>
```

Errors: `401` `signature_required`, `bad_signature`, `bad_nonce`,
`clock_skew`, `nonce_reused` or `unknown_device`; `403 device_revoked`.

You can revoke a device from your account page. A revoked device is
refused everywhere: its signatures fail, its sign-in ends, and a return it
sealed can no longer be sent.

## 3. Ticket

Before each send the app asks for a ticket that names exactly one return:

```http
POST /api/tickets            (signed)

{"tin": "000-123-453-00000", "form": "1701Qv2018", "period": "2026Q1",
 "payload_sha256": "<hex SHA-256 of the return's XML>",
 "email": "juana@example.com", "idempotency_key": "<1-100 chars>"}
```

`form`, `period` and `email` are the parts of the eBIRForms file name
`{TIN}-{form}-{period}#{email}#.xml`. The email is where BIR sends its
confirmation; you choose it.

The Gatekeeper checks that:

- the TIN is active on your account (`403 tin_not_active`). A TIN becomes
  active after you upload its BIR Form 2303 and a person approves the
  claim. Each TIN has one owner.
- the device is live.
- you are within the quotas: 30 tickets an hour per account and per
  device, and 50 a day per TIN (`429 quota_exceeded`).
- the idempotency key fits:
  - the same key for a different return is `409 idempotency_mismatch`;
  - the same key while its ticket is unused returns that ticket again;
  - the same key after a send that may have reached BIR is
    `409 already_submitted`.

The answer is `201 {"ticket", "jti", "expires_at"}`. The ticket is a
compact JWS (`alg: EdDSA`, `typ: buwiz-ticket+jwt`) for the Sender
(`aud: buwiz-sender`). It carries the account, device, 14-digit TIN, form,
period, email, `payload_sha256` and `jti`. It is valid for five minutes
and works once.

## 4. Seal and submit

The Sender's public key is at `GET /.well-known/buwiz-hpke.json`:

```json
{"keys": [{"kid": "…", "kem": "DHKEM(X25519, HKDF-SHA256)", "kdf": "HKDF-SHA256",
           "aead": "ChaCha20Poly1305", "public_key": "<base64url, 32 bytes>"}]}
```

The app seals the XML with HPKE (RFC 9180) base mode with that suite,
`info = "buwiz-submission-v1:" + jti` and an empty `aad`. Then it posts:

```http
POST /api/v2/submissions     (signed)

{"ticket": "<ticket>",
 "envelope": {"kid": "…", "enc": "<base64url encapsulated key>", "ct": "<base64url ciphertext>"}}
```

The Gatekeeper checks the ticket's signature and expiry, and that it was
issued to this account and device. Then it spends the ticket. A second
use is `409 ticket_used`, an expired ticket is `403 ticket_expired`, a
forged one is `403 ticket_invalid`, and another device's is
`403 ticket_mismatch`. It stores the envelope unopened and answers `201`
with the submission, including its `relay_path`.

## 5. The relay pipe

The app opens a WebSocket to `relay_path` (`wss://`) with the bearer token
and a device signature over `GET` and the path. From then on it only
forwards bytes:

| Frame | Direction | Meaning |
|---|---|---|
| text `{"type":"open","stream":N,"host":H,"port":P}` | relay → app | Connect to `H:P` for stream `N`. |
| text `{"type":"opened","stream":N}` | app → relay | Connected. |
| text `{"type":"refused","stream":N,"reason":"…"}` | app → relay | Not connected. |
| binary `[N][bytes…]` | both | Bytes on stream `N`. The first byte is the stream id. |
| text `{"type":"close","stream":N}` | both | No more bytes on `N` from this side. |
| text `{"type":"result","status":S,"error":E,"receipt":R}` | relay → app | Final. The relay closes the socket after it. |

The app:

- **connects only to the hosts on a signed egress list.** Before it opens
  the pipe, the app fetches `GET /.well-known/buwiz-egress.json`: a compact
  JWS (`alg: EdDSA`, `typ: buwiz-egress+jwt`, `kid`) over
  `{"hosts": ["host:port", …], "iat", "exp"}`. The list is signed offline
  with an egress key the relay never holds; the relay only serves it. The
  app checks the signature against the public key pinned in
  `crates/bir-submit/src/egress.rs` (`EGRESS_KEYS`, filled at release;
  `BUWIZ_EGRESS_KEYS` overrides it for tests and lab servers), the type and
  the expiry, then allows exactly those `host:port` pairs. Anything else
  gets `refused`. Without a valid list there is no send. The source names
  no host, and even a compromised relay can't use your computer as a
  general proxy.
- forwards bytes unchanged. It never terminates TLS or SSH, so all it sees
  is ciphertext.
- keeps the socket open until `result`.

Before anything reaches the network, the Sender:

1. verifies the ticket (signature, `typ`, `aud`, expiry) and that it was
   never used before;
2. opens the envelope, with `info` bound to the ticket's `jti`;
3. checks the XML's SHA-256 against the ticket;
4. checks the XML's own TIN, form, year and quarter against the ticket;
5. uploads it under the file name built from the ticket, accepting only
   BIR's pinned host key.

## 6. The result

| `status` | Meaning | What the app does |
|---|---|---|
| `sent` | BIR's server accepted the file. `receipt` is a signed receipt. | Checks the receipt, marks the return **Sent** and stores the receipt. |
| `dry_run` | The relay is in test mode. Nothing reached BIR. | Marks the send as a test. |
| `failed` | Nothing reached BIR. `error` says why. | Puts the return back in the queue, and retries later with a new ticket. |
| `unknown` | The connection broke during the upload. BIR may have the file. | Never resends on its own. Marks the return **Needs attention** and asks you to check for BIR's confirmation email. |

## 7. Receipts

A receipt is a compact JWS (`alg: EdDSA`, `typ: buwiz-receipt+jwt`) signed
by the Sender. The app verifies it with the key from
`GET /.well-known/jwks.json` (matched by `kid`), then checks that
`xml_sha256` is the hash of the XML it sent and `jti` is the ticket it
used. Only then is the return **Sent**.

| Claim | |
|---|---|
| `sub` | The submission id |
| `account_id`, `tin`, `form_type`, `period`, `filename` | What was filed |
| `xml_sha256` | SHA-256 of the XML the app sent |
| `payload_sha256` | SHA-256 of the file as uploaded (encrypted the way eBIRForms does) |
| `remote_path` | Where it landed on BIR's server |
| `mode` | `live`, `lab` or `dry_run` |
| `status` | `sent` or `dry_run` |
| `iat` | When it was sent (Unix seconds) |
| `iss` | The relay |
| `jti` | The ticket the send used |
