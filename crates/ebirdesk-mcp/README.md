# ebirdesk-mcp

The desktop app without a window: the **eBIRForms Desktop** mini app for the Buwiz Bot. A stdio MCP server the Bot
spawns, speaking Mini App Protocol 2.0 through the SDK crate `buwiz-plugin`
([buwiz-app/buwiz-sdk](https://github.com/buwiz-app/buwiz-sdk), pinned by commit in the root `Cargo.toml`).

| Name | Value |
|---|---|
| Plugin id | `app.buwiz.ebirdesk` |
| Manifest `short` / tool prefix | `ebirdesk` / `ebirdesk_*` |
| Binary, MCP server name | `ebirdesk-mcp` |
| Publisher, release key | `app.buwiz`, `gc-2026-01` |
| Signed manifest file | `app.buwiz.ebirdesk.manifest.json` |

These clash with neither the standalone eBIRForms mini app (`app.buwiz.ebirforms`, `ebirforms_*`), the desktop app
(`com.buwiz.ebirforms`, `buwiz`) nor the old mini app (`dev.goldcoders.bir`, `bir_*`); `tests/names.rs` keeps it so.

It works on the desktop app's own database (same data folder, same keychain key), opened through
`BirApp::open_live`. The database is shared: the desktop app and this process can both have it open, and each sees
the other's saves. This process never runs the queue worker. After it queues a return, it takes the worker lease
only if no other app holds it, runs the queue once through bir-submit's relay, then lets the lease go. When the
desktop app is open, the desktop app sends the return.

## Commands

```
ebirdesk-mcp mcp                        what the Bot spawns
ebirdesk-mcp manifest [--key-id <id>]   the unsigned manifest as JCS text, under gc-2026-01
ebirdesk-mcp app-source forms           the Forms app's TSX source, exactly as served
```

## Environment

| Variable | What |
|---|---|
| `BUWIZ_EBIRDESK_MANIFEST` | The signed manifest to serve. If it is unset, the binary looks for `app.buwiz.ebirdesk.manifest.json` next to itself. If neither exists, it serves the unsigned manifest, and the Bot refuses it. |
| `BUWIZ_DATABASE_PATH` | The database file (default: the desktop app's data folder). |
| `BUWIZ_DB_KEY`, `BUWIZ_DB_KEY_FILE` | The database key (default: the desktop app's keychain entry). |
| `BUWIZ_API_URL` | The Buwiz server that sending goes through (default: the app's setting). |

## Signing a release (the owner)

Nothing in this repo signs or reads the publisher key. Build, then print the unsigned manifest and the app source:

```
RUSTC_WRAPPER= cargo build --release -p ebirdesk-mcp
OUT=target/ebirdesk-release; BIN=target/release/ebirdesk-mcp
mkdir -p "$OUT" && cp "$BIN" "$OUT/"
"$BIN" manifest > "$OUT/manifest.json"
"$BIN" app-source forms > "$OUT/app-forms.tsx"
```

Sign with `gc-2026-01` from the login Keychain using rox-broker (`ROX_BROKER_BIN` names the binary), the same
signer and arguments as the standalone mini app's `tools/release/prepare.sh`:

```
"$ROX_BROKER_BIN" sign-manifest "$OUT/manifest.json" \
  --keychain gc-2026-01 --app forms="$OUT/app-forms.tsx" \
  --out "$OUT/app.buwiz.ebirdesk.manifest.json"
```

The signed `app.buwiz.ebirdesk.manifest.json` ships next to the `ebirdesk-mcp` binary, which serves it (or point
`BUWIZ_EBIRDESK_MANIFEST` at it). The Bot's trust store must approve `{ "id": "app.buwiz.ebirdesk",
"publisher_key_id": "gc-2026-01" }`. Install it in the Bot as a local plugin: command `<path>/ebirdesk-mcp`, args
`mcp`. If a later build changes a tool, the app source or any manifest text, the old signature no longer matches.
The binary logs that the signed manifest is stale, and the manifest must be signed again.
