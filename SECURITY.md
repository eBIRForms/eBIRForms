# Security

## Reporting a problem

Report security issues privately through GitHub's **Report a vulnerability**
button on this repository's Security tab. Please don't open a public issue.
Include what you found, how to reproduce it, and what it lets someone do. We
answer within a few days and credit you in the fix unless you'd rather not.

## What Buwiz protects, and how

### Your returns on your computer

- Everything you enter stays on your device in an encrypted database
  (SQLCipher).
- The database key lives in the operating system's key store: the Keychain on
  macOS and iOS, Credential Manager on Windows, and the Secret Service on
  Linux. Android has no system key store that Buwiz can use, so the key is a
  file in the app's private storage, which other apps can't read.
- Forms, checks, deadlines, penalties and printing all work offline. Buwiz
  opens no network connection until you press **Send**.

### No BIR secrets in the app

Filing with BIR needs passphrases and a login that BIR ships inside its own
eBIRForms installer. Any secret inside an app can be read by whoever runs the
app, so Buwiz carries none of them. The repository's `scripts/check-secrets.sh`
fails the build if a BIR passphrase, endpoint, or upload code appears in the
source, and `scripts/verify.sh` also checks the built binaries.

### Sending a return

When you press **Send**, the app talks to the Buwiz relay. The full protocol
is in [RELAY-PROTOCOL.md](RELAY-PROTOCOL.md). In short:

- **You sign in once per device.** The app makes a device key (Ed25519) and
  keeps it in the system key store. Every send request is signed with it, so a
  stolen access token alone can't file a return. You can revoke a device from
  your account.
- **You can only file for a TIN you own.** A TIN becomes active on your
  account after you upload its BIR Form 2303 and a person reviews it. Each TIN
  has one owner.
- **Each send gets a ticket.** The relay checks your account, device and TIN,
  then signs a ticket that names exactly what you're filing (TIN, form,
  period, a hash of the file, and the confirmation email). It expires after
  five minutes and works once.
- **The return is sealed for the sender only.** The app encrypts the return
  to the sender's public key (HPKE, RFC 9180). The part of the relay that
  checks your account can't read it.
- **The sender checks everything again.** It opens the envelope, checks the
  file against the ticket, encrypts it the way eBIRForms does, and uploads it.
  It accepts only BIR's pinned host key, so nothing in between can pose as
  BIR.
- **Traffic leaves from your own connection.** The upload travels through
  your device, so BIR sees your connection, as with eBIRForms. The app only
  passes encrypted bytes along. It never sees the login, and it connects only
  to BIR's own hosts: a fixed list in the source, kept as hashes.
- **You get a signed receipt.** The sender signs a receipt (EdDSA) that
  includes a hash of the exact file you sent. The app checks the signature and
  the hash before it marks the return **Sent**.

### What we don't claim

BIR's passphrases come from BIR's public installer. This design keeps them
out of the app and away from people who run our service, but it can't make
BIR's protocol secret. Sending through the relay doesn't mean BIR has endorsed
it.

## Supported versions

Only the latest release gets security fixes.
