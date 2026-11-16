# aigit

**Local-first, peer-to-peer checkpoints for AI coding work.**

`aigit` keeps an independent, content-addressed history of your workspace and agent events beside Git. It never creates Git commits, branches, or tags, never rewrites `.git`, and never syncs Git internals. Two machines pair with one command each, work offline, and synchronize the missing objects whenever they can reach each other again.

## Why

AI coding agents change files in many small steps, explore dead ends, and often work on machines that are not always online. Git remains the system of record for reviewed code, but it does not capture:

- which prompt or tool call changed a file,
- the workspace state before an agent's edit,
- the working state needed to continue the same task on another machine,
- offline work on two devices that later reconnect.

`aigit` adds that missing local-first layer without replacing Git.

## Install

From source:

```bash
cargo install --git https://github.com/yancyuu/aigit.git
```

Or clone and build:

```bash
git clone https://github.com/yancyuu/aigit.git
cd aigit
cargo install --path .
```

Requires Rust 1.93 or newer.

## Two-machine quick start

On the machine that currently has the project:

```bash
cd ~/work/my-project
aigit init
aigit checkpoint --intent "before handoff"
aigit share
```

`share` prints a one-time command such as:

```text
aigit join eyJ2ZXJzaW9uIjox...
```

On the other machine:

```bash
mkdir ~/work/my-project && cd ~/work/my-project
aigit join eyJ2ZXJzaW9uIjox... --target .
```

The second machine receives the workspace snapshot, checkpoint history, and agent event history. Git metadata is intentionally not copied; the original Git repository remains untouched.

For automatic checkpoints and reconnection on both machines:

```bash
aigit daemon start
```

Check state with:

```bash
aigit daemon status
aigit status
aigit sync
```

## Commands

| Command | Purpose |
|---|---|
| `aigit init` | Initialize `.aigit/` beside an optional Git repository |
| `aigit checkpoint` | Capture all current files with optional intent, note, and agent |
| `aigit record` | Append a prompt, tool call, decision, test result, or note |
| `aigit share` | Print a one-time join ticket and serve one peer |
| `aigit join` | Pair, synchronize, and materialize a shared workspace |
| `aigit serve` | Continuously accept paired peers |
| `aigit sync` | Exchange all missing objects with paired peers |
| `aigit log` | Show checkpoint history; `--events` shows the agent event DAG |
| `aigit status` | Show repository, head, peers, and last synchronization times |
| `aigit export` | Materialize a checkpoint into a new or empty directory |
| `aigit daemon start/stop/status/run` | Manage automatic checkpoints and background synchronization |

## Offline and shutdown semantics

Every checkpoint is complete on the local machine before synchronization. If the other machine is offline:

1. local work continues,
2. new objects wait in the local object store,
3. the next `aigit sync` or daemon cycle exchanges only missing objects,
4. both histories are preserved as parallel checkpoint branches.

Nothing can retrieve data that exists only on a powered-off machine. For higher availability, run `aigit daemon` on an always-on NAS, home server, or work machine as another equal peer. No machine has special authority.

If both machines changed the same workspace while offline, both checkpoint histories synchronize and both working directories remain untouched. Use `aigit log`, `aigit export`, and your normal Git workflow to resolve the changes. `aigit` only fast-forwards a working directory when it is clean relative to its last checkpoint and the peer checkpoint is a descendant.

## Agent events

Agents or harness hooks can record durable context without creating commits:

```bash
aigit record --kind prompt --agent claude-code --text "Refactor retry handling"
aigit record --kind tool --agent claude-code --text "ran cargo test -p storage"
aigit record --kind decision --text "Use exponential backoff without jitter"
aigit log --events
```

Events form an append-only DAG per device and synchronize with the same object protocol as checkpoints.

## Git compatibility

- `aigit init` adds `.aigit/` to `.git/info/exclude`; it does not modify tracked files.
- Checkpoints record the current `git HEAD` when a Git repository exists.
- `aigit` never runs mutations against Git and never copies `.git`.
- Use Git normally for commits, reviews, and merges.

## Transport and privacy

Networking uses [iroh](https://iroh.computer) encrypted QUIC connections identified by endpoint public keys. By default, iroh may use relay servers for NAT traversal and discovery. Relays only forward already-encrypted traffic and never store repositories.

For LAN, Tailscale, or zero-third-party deployments, use direct-only mode:

```bash
aigit share --direct-only
aigit serve --direct-only
aigit sync --direct-only
```

Direct-only mode disables relays and address lookup. The listen port is persisted per workspace so a restarted machine remains reachable at the same address. Both devices need direct reachability.

Potential secrets such as `.env`, private keys, and common credential files are skipped by default. Skipped paths are printed. `.gitignore` and `.aigitignore` are respected.

## Repository layout

```text
.aigit/
├── config.json       # device identity, endpoint secret, peers
├── objects/          # BLAKE3 content-addressed blobs and JSON objects
├── refs/heads/       # per-device checkpoint heads
├── refs/events/      # per-device agent event heads
├── daemon.log
└── daemon.status
```

Object integrity is verified with BLAKE3 when objects are written and again when read. Pairing tickets expire after ten minutes and authenticate both the repository and endpoint identity.

## Current limits

- Checkpoints are workspace snapshots, not Git history or Git LFS replacements.
- There is no automatic semantic code merge; divergent work stays as parallel histories.
- Agent adapters are not bundled. Harnesses can call `aigit checkpoint` and `aigit record`.
- Direct-only mode requires stable reachability; default relay mode is more resilient across NATs.
- Windows daemon management uses process termination commands and is not yet tested.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- init
```

## License

Apache-2.0. See [LICENSE](LICENSE).
