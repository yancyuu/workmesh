---
name: workmesh
description: Manage multi-project AI agent work attention with WorkMesh. Use when the user asks what to work on next, wants to track agent progress across machines, capture raw ideas without executing them, review agent grouping suggestions, record task receipts, or hand off context to another session. Git handles code; the harness handles sessions; WorkMesh handles task attention and cross-session context.
---

# WorkMesh: Agent Task Attention Skill

## Core principles

1. **Capture is not execution.** When the user says an idea, question, or bug, record it with `capture`. Do not run it, implement it, or turn it into a task without explicit user intent.
2. **Original words are immutable.** Never edit, summarize over, or delete a capture. Grouping proposals link captures; they don't replace them.
3. **Agent proposals require review.** Use `propose` to suggest grouping, then wait for the user to `review --accept` or `review --reject`. Never auto-accept.
4. **Done requires evidence.** A task is only `done` with a summary and at least one evidence item. Agent self-report is evidence, not proof.
5. **Respect WIP limits.** If active tasks are at the limit, ask the user to finish or pause one before activating another.
6. **Git is read-only.** WorkMesh observes HEAD and branch name for context. Never use WorkMesh as a reason to modify Git state.
7. **Multi-machine means last-reported, not live.** Views show the last known state and which machine reported it. Offline machines may have newer work that hasn't synced yet.

## Commands

### Check current state

```bash
workmesh now
workmesh inbox
workmesh blocked
workmesh stale --days 7
workmesh needs-me
```

`now` is the default view and shows the most important items with reasons.

### Register projects

```bash
workmesh project add <name> --path <directory>
workmesh project list
```

Project names are slugs (letters, digits, hyphens, underscores). Paths are local to each machine and are not synced.

### Capture raw input

```bash
workmesh capture "text" --kind idea|question|bug|request [--project <name>]
```

Always preserve the user's exact words. The `--kind` helps the organizer but does not determine execution.

### Create and update directions

```bash
workmesh task add "title" --project <name> [--kind task|bug|feature] [--priority 0-3] [--goal "what done looks like"]
workmesh task update <id> --status active --next "concrete next step"
workmesh task update <id> --status blocked --blocker "why it stopped"
workmesh task update <id> --status waiting --next "what you're waiting for"
```

Active and waiting tasks require `--next`. Blocked tasks require `--blocker`.

### Record progress

```bash
workmesh task receipt <id> \
  --summary "what was accomplished" \
  --next "what to do next" \
  --evidence "how you know"
```

Record receipts at meaningful checkpoints, not after every tool call.

### Complete a task

```bash
workmesh task done <id> --summary "result" --evidence "verification"
```

### Hand off context

```bash
workmesh task handoff <id>
```

Output includes goal, completed, next, blocker, Git context, harness, session, and evidence. The receiving agent should verify code and environment before acting.

### Propose grouping

```bash
workmesh propose --capture <id> --direction <task-id> --reason "why these belong together"
```

Then the user reviews:

```bash
workmesh review <proposal-id> --accept
workmesh review <proposal-id> --reject
workmesh review <proposal-id>  # inspect without deciding
```

### Resolve conflicts

When two machines concurrently update the same task:

```bash
workmesh task show <id>          # shows conflict=true and head revisions
workmesh task resolve <id> --choose <revision-prefix>
```

### View history

```bash
workmesh task history <id>
workmesh task show <id>          # includes linked captures
```

### Multi-machine sync

```bash
workmesh ledger export --output file.json
workmesh ledger import file.json
workmesh ledger sync --ssh user@host
```

Only task records sync — never code, credentials, or project paths.

### ACP harness

```bash
workmesh harness list
workmesh harness check <name> [--command /path/to/adapter]
workmesh harness prompt <name> \
  --cwd <project-dir> \
  --text "prompt" \
  --task <task-id> \
  [--command /path/to/adapter] \
  [--timeout 300]
```

After the ACP session, WorkMesh writes a receipt with harness name, session ID, stop reason, and evidence. Permissions are cancelled by default.

### Optional web UI

```bash
workmesh ui install
workmesh ui              # start and open browser
workmesh ui status
workmesh ui uninstall    # task data is never deleted
```

## Agent workflow

### Session start

1. Run `workmesh now` to see current attention items.
2. Run `workmesh task show <id>` for relevant tasks.
3. Check if the user wants to continue an existing direction or capture a new idea.
4. If continuing, verify the next step still makes sense before proceeding.

### During work

1. Record meaningful checkpoints with `task receipt`.
2. If blocked, update with `--status blocked --blocker "reason"`.
3. If waiting on user decision, update with `--needs-input`.

### Session end

1. Write a final receipt with current state and next step.
2. Use `task handoff <id>` to generate context for the next session.
3. Do not mark `done` unless the user confirms the result and you have evidence.

### Handling user ideas

When the user says something that might be a new task:

1. Capture the exact words: `workmesh capture "..." --kind idea`
2. Check if it relates to an existing direction: `workmesh now` and `workmesh inbox`
3. If it does, propose grouping: `workmesh propose --capture <id> --direction <task-id> --reason "..."`
4. Wait for user review. Do not auto-execute.

## Error handling

| Error | Action |
|---|---|
| `task not found` | Run `workmesh now` or `workmesh task list` to find valid IDs |
| `WIP limit reached` | Ask user to finish/pause an active task |
| `ready/active/waiting requires --next` | Provide a concrete next step |
| `blocked requires --blocker` | State why it's blocked |
| `done requires summary and evidence` | Provide both; evidence is reported, not independently verified |
| `proposal is stale` | Re-propose against the current task revision |
| `task has concurrent revisions` | Use `task resolve` after reviewing both versions |
| `ACP adapter not installed` | Point user to the adapter's install instructions; WorkMesh never installs agents |
| `invalid session token` | Restart `workmesh ui`; the token is per-session |

## Boundaries

- Do not use WorkMesh to modify Git state (commit, branch, reset, checkout, push).
- Do not auto-approve ACP permission requests.
- Do not create tasks for every tool call or file read.
- Do not mark tasks done without user confirmation and evidence.
- Do not assume offline machines have no new work.
- Do not sync project paths or credentials across machines.
