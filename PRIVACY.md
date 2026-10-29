# Privacy

Buwiz is an offline filer. This page says what stays on your device, what
leaves it, and when.

## What stays on your device

Everything you type stays on your device, in an encrypted database:
taxpayers, branches, tax profiles, drafts, filed returns, receipts and
settings. Filling, checking, computing penalties and printing all happen on
your device. Buwiz has no analytics, no tracking and no ads.

## What leaves your device, and when

Nothing leaves your device until you choose to send a return, and only then:

| When | What is sent | Why |
|---|---|---|
| You sign in to send | Your email address, and a public key for this device | To give you an account and recognise this device |
| You claim a TIN | Your BIR Form 2303, a government ID and any supporting files you choose | So a person can confirm the TIN is yours before anyone files for it |
| You press **Send** | The return (encrypted so only the sender can read it), and its TIN, form, period, file hash and confirmation email | To file the return with BIR and give you a receipt |

The email address in the return's file name is where BIR sends its
confirmation. You pick it. Buwiz never reads your inbox: to keep BIR's
confirmation, print it from your own email.

## How long the relay keeps things

- **The return:** encrypted while it waits to be sent, then deleted once it is
  sent or after 24 hours, whichever comes first.
- **Claim documents:** encrypted while they wait for review, then deleted when
  the claim is decided or withdrawn. Only their fingerprints (hashes) are kept.
- **Receipts and account records:** kept so you can see what you filed. They
  hold the TIN, form, period, file hashes and time. They don't hold the
  return itself.
- **Logs:** metadata only, never the contents of a return.

## Your choices

- Use Buwiz without an account. Everything except sending works.
- Revoke a device, or close your account, from your account page.
- Delete a draft or a taxpayer on your device at any time.

## The law

The relay processes personal information under the Philippine Data Privacy
Act of 2012 (RA 10173). Questions or requests about your data go through the
contact on the Buwiz account page.
