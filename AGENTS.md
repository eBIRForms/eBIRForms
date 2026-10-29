# Buwiz

An offline BIR filer in Rust. `app/` is the nsh desktop app (its own cargo
workspace, like rox's), `crates/` the domain crates (the root workspace),
`nsh/` and `blaze/` submodules on the hexuria forks' `rox` branch.
`README.md` covers building and the live loop.

## Rules

- The UI is Rust-native nsh components. No TSX, no QuickJS, no web views
  except the print preview.
- Domain logic lives in `crates/`, never in `app/`. `bir-core` opens no
  network connection.
- No BIR protocol constants, endpoints or SFTP code in this repo, ever.
  `scripts/check-secrets.sh` enforces it; run `just secrets` before a commit.
- Changes to `nsh/` follow rox's fork process (`Projects/rox/NSH-FORK.md`):
  a topic branch on hexuria/nsh, never a commit on `rox` or `main`.
- Build release binaries with `RUSTC_WRAPPER=` (a stale `kache` crate once
  served a release build in rox).

## Internal docs

`internal/` is gitignored and holds the plan, decisions and progress. If it
exists, read `internal/README.md` before starting work, and update
`internal/PROGRESS.md` when a task changes state.
