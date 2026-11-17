# WorkMesh

**Terminal-first task attention ledger for work across projects, machines, and AI agents.**

WorkMesh answers one question: *across all your projects and agent sessions, what needs attention right now, and what is the next step?*

It does **not** replace Git, sync code directories, run agents, or require an account.

```
Git          → code history, branches, diffs, merges
Harness      → agent sessions, prompts, tools, permissions
WorkMesh     → task attention, original ideas, agent receipts, next steps
```

## Install

```bash
cargo install --git https://github.com/yancyuu/workmesh.git --locked
```

Requires Rust 1.93+.

## Quick start

```bash
# Register a project
workmesh project add myapp --path ~/code/myapp

# Capture a raw idea, question, bug, or request — it is not executed
workmesh capture "Login page feels slow on mobile" --kind bug --project myapp

# See what needs attention
workmesh now

# Start a direction
workmesh task add "Improve mobile login performance" \
  --project myapp --kind feature --priority 1 \
  --goal "Reduce perceived load time below 1s"

# Record meaningful agent progress
workmesh task receipt <task-id> \
  --summary "Identified render-blocking font as bottleneck" \
  --next "Implement font-display:swap and re-test" \
  --evidence "Lighthouse mobile score improved from 42 to 58"

# Hand off to another machine, agent, or harness
workmesh task handoff <task-id>
```

## Core concepts

| Object | What it is | Not |
|---|---|---|
| Capture | Your original words, preserved exactly | An execution instruction |
| Direction | A goal you're actively pushing forward | A subtask tree |
| Proposal | An agent's suggestion to group captures | An automatic action |
| Receipt | A meaningful progress record with evidence | A tool-call log |
| Handoff | Context for the next session to continue | A session migration |

## Attention views

```bash
workmesh now        # What matters right now (default)
workmesh inbox      # Unprocessed captures
workmesh blocked    # Why things stopped
workmesh stale      # No update in N days
workmesh needs-me   # Decisions waiting for you
```

Every item shows **why it's in this view**, **when it was last reported**, **by which machine**, and **what the next step is**.

## Agent integration

WorkMesh uses the [Agent Client Protocol (ACP)](https://agentclientprotocol.com) to connect to local coding agents via their adapters. It never downloads, installs, or authenticates agents for you.

```bash
# Detect installed adapters (does not execute them)
workmesh harness list

# Verify ACP handshake with a specific adapter
workmesh harness check codex --command /path/to/codex-acp

# Run a prompt and link the result to a task
workmesh harness prompt claude \
  --command /path/to/claude-agent-acp \
  --cwd ~/code/myapp \
  --text "Continue improving mobile login" \
  --task <task-id> \
  --timeout 300
```

After the ACP session, WorkMesh writes a receipt to the task with the harness name, session ID, stop reason, and evidence. Permissions are **cancelled by default** — WorkMesh never auto-approves tool calls.

### Supported via ACP adapters

| Agent | Adapter | Install separately |
|---|---|---|
| Claude Code | `claude-agent-acp` | `npm i -g @agentclientprotocol/claude-agent-acp` |
| Codex | `codex-acp` | `npm i -g @agentclientprotocol/codex-acp` |
| Pi | `pi-acp` | Build from [regadas/pi-acp](https://github.com/regadas/pi-acp) |
| OpenCode | `opencode acp` | `npm i -g opencode-ai` |

## Multi-machine sync

WorkMesh syncs task records only — never code, never credentials.

```bash
# Export a bundle
workmesh ledger export --output mesh.json

# Import on another machine
workmesh ledger import mesh.json

# Or sync via SSH (requires workmesh on remote)
workmesh ledger sync --ssh user@remote-host
```

Concurrent edits to the same task from different machines create a visible conflict, never a silent overwrite. Use `workmesh task resolve <id> --choose <revision>` to pick the correct version.

## Optional web UI

The CLI is the primary interface. A local web UI is available as an optional plugin.

```bash
# Install from source repository
workmesh ui install

# Start (opens browser automatically)
workmesh ui

# Or with specific settings
workmesh ui start --port 8477 --no-open

# Check status
workmesh ui status

# Remove UI (task data is never deleted)
workmesh ui uninstall
```

The UI binds to `127.0.0.1` only, uses a random session token in the URL fragment, and enforces CSP headers.

## Task lifecycle

```
Inbox → Ready → Active → Waiting → Done
         ↓        ↓
       Blocked  Blocked
```

- **Active** requires `--next` (a concrete next step)
- **Blocked** requires `--blocker` (why it stopped)
- **Done** requires `--summary` and `--evidence`
- Default WIP limit: 3 active tasks (configurable)

```bash
# Configure
workmesh config --machine home-mac --wip 5
```

## Git boundary

WorkMesh is read-only with respect to Git:
- Observes `git rev-parse HEAD` and current branch for context
- Never creates commits, branches, tags, or stashes
- Never stages files, checks out, rebases, resets, or pushes
- Never copies `.git` or synchronizes working directories

## Agent skill

Coding agents should read [skills/workmesh/SKILL.md](skills/workmesh/SKILL.md) before using WorkMesh. Install it into your agent's skill directory:

```bash
mkdir -p .claude/skills
cp -R skills/workmesh .claude/skills/
```

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo build --release
```

## License

Apache-2.0
