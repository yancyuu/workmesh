# WorkMesh

**A local-first work mesh for AI coding agents.**

`workmesh` keeps an independent, content-addressed history of your workspace and agent events beside Git. It never creates Git commits, branches, or tags, never rewrites `.git`, and never syncs Git internals. Two machines pair with one command each, work offline, and synchronize the missing objects whenever they can reach each other again.

## Why

AI coding agents change files in many small steps, explore dead ends, and often work on machines that are not always online. Git remains the system of record for reviewed code, but it does not capture:

- which prompt or tool call changed a file,
- the workspace state before an agent's edit,
- the working state needed to continue the same task on another machine,
- offline work on two devices that later reconnect.

`workmesh` adds that missing local-first layer without replacing Git.

## Install

From source:

```bash
cargo install --git https://github.com/yancyuu/workmesh.git
```

Or clone and build:

```bash
git clone https://github.com/yancyuu/workmesh.git
cd workmesh
cargo install --path .
```

Requires Rust 1.93 or newer.

## Two-machine quick start

On the machine that currently has the project:

```bash
cd ~/work/my-project
workmesh share
```

`share` initializes `workmesh` if needed, captures the current workspace, and prints a one-time command such as:

```text
workmesh join eyJ2ZXJzaW9uIjox...
```

On the other machine:

```bash
mkdir ~/work/my-project && cd ~/work/my-project
workmesh join eyJ2ZXJzaW9uIjox... --target .
```

The second machine receives the workspace snapshot, checkpoint history, and agent event history. Git metadata is intentionally not copied; the original Git repository remains untouched.

For automatic checkpoints and reconnection on both machines:

```bash
workmesh start
```

Check state with:

```bash
workmesh status
workmesh sync
```

Stop the background daemon with:

```bash
workmesh stop
```

## Sharing a pairing ticket

The default workflow is to copy the printed one-line command from one terminal to the other:

```bash
workmesh join eyJ2ZXJzaW9uIjox...
```

For a file-based transfer over `scp`, AirDrop, a password manager, or another encrypted channel:

```bash
# Machine A
workmesh share --ticket-out /tmp/workmesh-ticket

# Machine B
mkdir ~/work/my-project && cd ~/work/my-project
workmesh join --ticket-file /tmp/workmesh-ticket --target .
rm /tmp/workmesh-ticket
```

The ticket file is created with owner-only permissions and contains only the ticket, not workspace data. Delete it after use.

When the other device is nearby, render a QR code in the terminal:

```bash
workmesh share --qr
```

The QR code encodes the complete `workmesh join ...` command. Scan it only over a trusted path: a ticket is a bearer invitation until it expires or is redeemed.

## Keys, pairing, and recovery

You do not create, remember, or type a password.

- A pairing ticket is valid for one successful join and expires after ten minutes.
- When someone redeems the ticket, the machine running `workmesh share` shows the requesting device name, device ID, and endpoint ID, then asks for confirmation.
- Only after you answer `y` can that device join.
- For scripts and automation, `workmesh share --yes` skips this interactive confirmation; use it only where the ticket channel is trusted.
- After `join` succeeds, both devices rotate the one-time ticket into a saved peer credential in `.workmesh/config.json`.
- Later `workmesh sync`, `workmesh start`, and incoming peer connections use those saved credentials automatically.
- The ticket is not needed again after pairing and cannot be reused after a successful join.
- If a ticket expires before it is used, run `workmesh share` again on an already-paired device.

A ticket is still a bearer credential before redemption: anyone who has it can request to join. Send tickets only over a channel you trust, and reject the confirmation prompt if an unexpected device name or endpoint ID appears.

To add or recover a machine, use any surviving paired machine:

```bash
# On a machine that still has .workmesh/
workmesh stop
workmesh share

# On the new or recovered machine
workmesh join <new-ticket> --target .
```

If a device is lost or no longer trusted:

```bash
workmesh stop
workmesh status           # copy the full endpoint ID shown as Peer <id>
workmesh unpair <endpoint-id>
workmesh start
```

Run `workmesh unpair` on every surviving trusted device. There is intentionally no hosted account service that can recover or revoke keys globally. Keep at least one always-on trusted peer, such as a home server or NAS, or keep an encrypted backup of `.workmesh/` if losing every device would be unacceptable.

## Commands

Daily commands:

| Command | Purpose |
|---|---|
| `workmesh share` | Initialize if needed, checkpoint, print a one-time join ticket, confirm the joining device, and serve one peer; supports `--ticket-out` and `--qr` |
| `workmesh join` | Pair, synchronize, and materialize a shared workspace; accepts a positional ticket or `--ticket-file` |
| `workmesh start` | Start automatic checkpoints and background synchronization |
| `workmesh stop` | Stop the background daemon |
| `workmesh sync` | Exchange all missing objects with paired peers now |
| `workmesh status` | Show workspace, full peer IDs, synchronization times, and daemon state |
| `workmesh unpair` | Remove a paired device from this machine |
| `workmesh log` | Show checkpoint history; `--events` shows the agent event DAG |

Advanced commands remain available for scripts and harnesses:

| Command | Purpose |
|---|---|
| `workmesh init` | Initialize manually; `share` does this automatically |
| `workmesh checkpoint` | Capture all current files with optional intent, note, and agent |
| `workmesh record` | Append a prompt, tool call, decision, test result, or note |
| `workmesh serve` | Continuously accept paired peers in the foreground |
| `workmesh export` | Materialize a checkpoint into a new or empty directory |
| `workmesh daemon run/start/stop/status` | Low-level daemon controls |

## Offline and shutdown semantics

Every checkpoint is complete on the local machine before synchronization. If the other machine is offline:

1. local work continues,
2. new objects wait in the local object store,
3. the next `workmesh sync` or daemon cycle exchanges only missing objects,
4. both histories are preserved as parallel checkpoint branches.

Nothing can retrieve data that exists only on a powered-off machine. For higher availability, run `workmesh start` on an always-on NAS, home server, or work machine as another equal peer. No machine has special authority.

If both machines changed the same workspace while offline, both checkpoint histories synchronize and both working directories remain untouched. Use `workmesh log`, `workmesh export`, and your normal Git workflow to resolve the changes. `workmesh` only fast-forwards a working directory when it is clean relative to its last checkpoint and the peer checkpoint is a descendant.

## Agent events

Agents or harness hooks can record durable context without creating commits:

```bash
workmesh record --kind prompt --agent claude-code --text "Refactor retry handling"
workmesh record --kind tool --agent claude-code --text "ran cargo test -p storage"
workmesh record --kind decision --text "Use exponential backoff without jitter"
workmesh log --events
```

Events form an append-only DAG per device and synchronize with the same object protocol as checkpoints.

## Git compatibility

`workmesh` treats Git as a read-only external system:

- The only Git command it runs is `git rev-parse HEAD` to annotate checkpoints.
- It never creates commits, branches, tags, stashes, hooks, or refs.
- It never stages files, changes the index, checks out, rebases, resets, or pushes.
- It never copies or synchronizes `.git`; Git history remains wherever you put it.
- `workmesh init` adds `.workmesh/` to `.git/info/exclude` so its local state cannot be committed accidentally. This does not change tracked files or Git history.
- Checkpoints can include uncommitted and untracked files, but storing them never changes Git state.
- After synchronization, use `git status` and `git diff` exactly as before. Divergent offline work remains in `workmesh` history until you resolve it with your normal Git workflow.
- `.gitignore` is respected. Put `workmesh`-only exclusions in `.workmeshignore`; there is no need to modify `.gitignore` for `workmesh`.

## Transport and privacy

Networking uses [iroh](https://iroh.computer) encrypted QUIC connections identified by endpoint public keys. By default, iroh may use relay servers for NAT traversal and discovery. Relays only forward already-encrypted traffic and never store repositories.

For LAN, Tailscale, or zero-third-party deployments, use direct-only mode:

```bash
workmesh share --direct-only
workmesh serve --direct-only
workmesh sync --direct-only
```

Direct-only mode disables relays and address lookup. The listen port is persisted per workspace so a restarted machine remains reachable at the same address. Both devices need direct reachability.

Potential secrets such as `.env`, private keys, and common credential files are skipped by default. Skipped paths are printed. `.gitignore` and `.workmeshignore` are respected.

## Repository layout

```text
.workmesh/
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
- Agent adapters are not bundled. Harnesses can call `workmesh checkpoint` and `workmesh record`.
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
