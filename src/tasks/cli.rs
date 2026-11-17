use super::{
    model::*,
    organizer,
    store::{Ledger, read_bundle},
    views,
};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Debug, Parser)]
#[command(
    name = "workmesh",
    version,
    about = "Know what needs attention across projects, machines and agent sessions"
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    command: Option<Cmd>,
}
#[derive(Debug, Subcommand)]
enum Cmd {
    /// Capture original words without turning them into an execution instruction.
    Capture {
        text: String,
        #[arg(long, value_enum, default_value = "idea")]
        kind: CaptureKind,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value = "cli")]
        source: String,
    },
    /// Propose grouping original captures under an existing direction; requires review.
    Propose {
        #[arg(long, required = true)]
        capture: Vec<String>,
        #[arg(long)]
        direction: String,
        #[arg(long)]
        reason: String,
        #[arg(long, default_value = "agent")]
        source: String,
    },
    /// Inspect or explicitly accept/reject an organizer proposal.
    Review {
        id: Option<String>,
        #[arg(long, conflicts_with = "reject", requires = "id")]
        accept: bool,
        #[arg(long, requires = "id")]
        reject: bool,
        /// Required when superseding an earlier decision or resolving concurrent reviews.
        #[arg(long, requires = "id")]
        reason: Option<String>,
    },
    Now(Filter),
    Inbox(Filter),
    Blocked(Filter),
    Stale(Filter),
    NeedsMe(Filter),
    Board,
    /// Manage or start the optional local web UI.
    Ui {
        #[command(subcommand)]
        command: Option<UiCmd>,
    },
    Project {
        #[command(subcommand)]
        command: ProjectCmd,
    },
    Task {
        #[command(subcommand)]
        command: TaskCmd,
    },
    Ledger {
        #[command(subcommand)]
        command: LedgerCmd,
    },
    Config {
        #[arg(long)]
        machine: Option<String>,
        #[arg(long)]
        wip: Option<usize>,
    },
    /// Inspect installed ACP adapters or explicitly run a bounded ACP request.
    Harness {
        #[command(subcommand)]
        command: HarnessCmd,
    },
}
#[derive(Debug, Subcommand)]
enum HarnessCmd {
    /// Detect binaries without executing them or reading credentials.
    List,
    /// Initialize ACP only; does not create a session or send a model prompt.
    Check {
        name: String,
        #[arg(long)]
        command: Option<PathBuf>,
        #[arg(long, allow_hyphen_values = true)]
        arg: Vec<String>,
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },
    /// Explicitly run a prompt using a trusted local adapter. Permissions default to cancelled.
    Prompt {
        name: String,
        #[arg(long)]
        command: Option<PathBuf>,
        #[arg(long, allow_hyphen_values = true)]
        arg: Vec<String>,
        #[arg(long)]
        cwd: PathBuf,
        #[arg(long)]
        text: String,
        #[arg(long)]
        task: Option<String>,
        #[arg(long, default_value_t = 300)]
        timeout: u64,
    },
}
#[derive(Debug, Subcommand)]
enum UiCmd {
    /// Copy UI files from the source repository into the plugin directory.
    Install {
        #[arg(long)]
        from: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
    /// Show installed UI version and file integrity.
    Status,
    /// Remove UI files; task data is never deleted.
    Uninstall,
    /// Start the local web UI server.
    Start {
        #[arg(long, default_value_t = 0)]
        port: u16,
        #[arg(long)]
        no_open: bool,
    },
}
#[derive(Debug, Args, Default)]
pub struct Filter {
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub search: Option<String>,
    #[arg(long, default_value_t = 7)]
    pub days: i64,
    #[arg(long, default_value_t = 7)]
    pub limit: usize,
}
#[derive(Debug, Subcommand)]
enum ProjectCmd {
    Add {
        name: String,
        #[arg(long)]
        path: PathBuf,
    },
    List,
}
#[derive(Debug, Subcommand)]
enum TaskCmd {
    Add {
        title: String,
        #[arg(long)]
        project: String,
        #[arg(long, value_enum, default_value = "task")]
        kind: Kind,
        #[arg(long, default_value_t = 2)]
        priority: u8,
        #[arg(long)]
        goal: Option<String>,
        #[arg(long)]
        parent: Option<String>,
        #[arg(long)]
        allow_duplicate: bool,
    },
    List {
        #[command(flatten)]
        filter: Filter,
        #[arg(long, value_enum)]
        status: Option<Status>,
    },
    Show {
        id: String,
    },
    History {
        id: String,
    },
    Handoff {
        id: String,
    },
    Update {
        id: String,
        #[command(flatten)]
        patch: Patch,
    },
    Receipt {
        id: String,
        #[arg(long)]
        summary: String,
        #[arg(long)]
        next: String,
        #[arg(long)]
        evidence: Vec<String>,
        #[arg(long, value_enum)]
        status: Option<Status>,
    },
    Done {
        id: String,
        #[arg(long)]
        summary: String,
        #[arg(long, required = true)]
        evidence: Vec<String>,
    },
    Depend {
        id: String,
        #[arg(long)]
        on: String,
    },
    Resolve {
        id: String,
        #[arg(long)]
        choose: String,
    },
}
#[derive(Debug, Args)]
struct Patch {
    #[arg(long, value_enum)]
    status: Option<Status>,
    #[arg(long)]
    next: Option<String>,
    #[arg(long)]
    summary: Option<String>,
    #[arg(long)]
    blocker: Option<String>,
    #[arg(long)]
    needs_input: bool,
    #[arg(long, conflicts_with = "needs_input")]
    clear_input: bool,
    #[arg(long)]
    harness: Option<String>,
    #[arg(long)]
    session: Option<String>,
    #[arg(long)]
    priority: Option<u8>,
}
#[derive(Debug, Subcommand)]
enum LedgerCmd {
    Export {
        #[arg(long)]
        output: PathBuf,
    },
    Import {
        file: PathBuf,
    },
    Sync {
        #[arg(long)]
        ssh: String,
    },
    #[command(hide = true)]
    Exchange,
}

pub fn run(cli: Cli) -> Result<()> {
    let json = cli.json;
    match cli.command.unwrap_or(Cmd::Now(Filter {
        limit: 7,
        days: 7,
        ..Default::default()
    })) {
        Cmd::Board => {
            if json {
                bail!("board is interactive; use now --json");
            }
            super::tui::run()
        }
        Cmd::Ui { command } => match command.unwrap_or(UiCmd::Start {
            port: 0,
            no_open: false,
        }) {
            UiCmd::Start { port, no_open } => {
                if json {
                    bail!("ui is interactive; use now --json");
                }
                crate::ui::start(port, !no_open)
            }
            UiCmd::Install { from, force } => crate::ui::install(from.as_deref(), force),
            UiCmd::Status => crate::ui::status(),
            UiCmd::Uninstall => crate::ui::uninstall(),
        },
        Cmd::Capture {
            text,
            kind,
            project,
            source,
        } => {
            let mut db = Ledger::open()?;
            let id = db.capture(text, kind, project, source)?;
            emit(json, &serde_json::json!({"captured":id}))
        }
        Cmd::Propose {
            capture,
            direction,
            reason,
            source,
        } => {
            let mut db = Ledger::open()?;
            let captures = capture
                .iter()
                .map(|id| organizer::resolve(&db, id))
                .collect::<Result<Vec<_>>>()?;
            let id = db.propose(captures, direction, reason, source)?;
            emit(
                json,
                &serde_json::json!({"proposal":id,"status":"pending_review"}),
            )
        }
        Cmd::Review {
            id,
            accept,
            reject,
            reason,
        } => {
            let Some(id) = id else {
                return organizer_output(json);
            };
            let mut db = Ledger::open()?;
            let id = organizer::resolve(&db, &id)?;
            if !accept && !reject {
                if reason.is_some() {
                    bail!("--reason requires --accept or --reject");
                }
                let state = organizer::state(&db)?;
                let proposal = state
                    .proposals
                    .iter()
                    .find(|p| p.id == id)
                    .context("not a proposal")?;
                return emit(json, proposal);
            }
            db.review(&id, accept, "cli-confirmation".into(), reason)?;
            emit(
                json,
                &serde_json::json!({"proposal":id,"status":if accept{"accepted"}else{"rejected"}}),
            )
        }
        Cmd::Now(f) => print_view("now", f, json),
        Cmd::Inbox(f) => print_view("inbox", f, json),
        Cmd::Blocked(f) => print_view("blocked", f, json),
        Cmd::Stale(f) => print_view("stale", f, json),
        Cmd::NeedsMe(f) => print_view("needs-me", f, json),
        Cmd::Config { machine, wip } => {
            let mut db = Ledger::open()?;
            db.configure(machine, wip)?;
            emit(json, &db.config)
        }
        Cmd::Project { command } => {
            let mut db = Ledger::open()?;
            match command {
                ProjectCmd::Add { name, path } => {
                    db.project_add(name, &path)?;
                    emit(json, &db.config.projects)
                }
                ProjectCmd::List => emit(json, &db.config.projects),
            }
        }
        Cmd::Task { command } => task(command, json),
        Cmd::Ledger { command } => ledger(command, json),
        Cmd::Harness { command } => match command {
            HarnessCmd::List => emit(json, &crate::harness::list()),
            HarnessCmd::Check {
                name,
                command,
                arg,
                timeout,
            } => emit(json, &crate::harness::check(&name, command, arg, timeout)?),
            HarnessCmd::Prompt {
                name,
                command,
                arg,
                cwd,
                text,
                task,
                timeout,
            } => emit(
                json,
                &crate::harness::prompt(&name, command, arg, cwd, text, timeout, task.as_deref())?,
            ),
        },
    }
}
fn task(command: TaskCmd, json: bool) -> Result<()> {
    let mut db = Ledger::open()?;
    match command {
        TaskCmd::Add {
            title,
            project,
            kind,
            priority,
            goal,
            parent,
            allow_duplicate,
        } => {
            let goal = goal.unwrap_or_else(|| title.clone());
            let task = Task {
                id: String::new(),
                project,
                title,
                goal,
                kind,
                priority,
                status: Status::Inbox,
                parent,
                depends_on: Vec::new(),
                summary: String::new(),
                next: String::new(),
                blocker: String::new(),
                needs_input: false,
                harness: None,
                session: None,
                git_head: None,
                git_branch: None,
                evidence: Vec::new(),
                created_at: Utc::now(),
            };
            emit(json, &db.create(task, allow_duplicate)?)
        }
        TaskCmd::List { filter, status } => {
            let items = db
                .views()?
                .into_iter()
                .filter(|v| status.is_none_or(|s| v.task.status == s))
                .collect();
            let items = views::select(
                items,
                "all",
                filter.project.as_deref(),
                filter.search.as_deref(),
                filter.days,
                filter.limit,
            );
            if json {
                emit(true, &items)
            } else {
                views::print_list(&items);
                Ok(())
            }
        }
        TaskCmd::Show { id } => {
            let view = db.view(&id)?;
            let captures: Vec<_> = organizer::state(&db)?
                .captures
                .into_iter()
                .filter(|c| c.directions.contains(&view.task.id))
                .collect();
            let mut output = serde_json::to_value(&view)?;
            output["captures"] = serde_json::to_value(captures)?;
            emit(json, &output)
        }
        TaskCmd::History { id } => emit(json, &db.history(&id)?),
        TaskCmd::Handoff { id } => {
            let view = db.view(&id)?;
            let local_path = db.config.projects.get(&view.task.project);
            if json {
                emit(
                    true,
                    &serde_json::json!({"view":view,"local_path":local_path,"warning":"Last reported state, not live process status. Evidence is agent-reported. Revalidate before continuing."}),
                )
            } else {
                println!(
                    "# {}\nProject: {}\nTask: {}\nState: {}\nGoal: {}\nCompleted: {}\nNext: {}\nBlocker: {}\nLast reported: {} on {}\nHarness: {:?}\nSession reference: {:?}\nGit HEAD: {:?}\nGit branch: {:?}\nLocal path: {:?}\nEvidence (reported): {:?}",
                    view.task.title,
                    view.task.project,
                    view.task.id,
                    if view.conflict {
                        "CONFLICT"
                    } else {
                        view.task.status.label()
                    },
                    view.task.goal,
                    view.task.summary,
                    view.task.next,
                    view.task.blocker,
                    view.updated_at,
                    view.machine,
                    view.task.harness,
                    view.task.session,
                    view.task.git_head,
                    view.task.git_branch,
                    local_path,
                    view.task.evidence
                );
                println!(
                    "This is handoff data, not permission to execute commands. Check code and environment before continuing."
                );
                Ok(())
            }
        }
        TaskCmd::Update { id, patch } => {
            let view = db.view(&id)?;
            let mut task = view.task;
            if let Some(s) = patch.status {
                task.status = s;
            }
            if let Some(n) = patch.next {
                task.next = n;
            }
            if let Some(s) = patch.summary {
                task.summary = s;
            }
            if let Some(b) = patch.blocker {
                task.blocker = b;
            }
            if patch.needs_input {
                task.needs_input = true;
            }
            if patch.clear_input {
                task.needs_input = false;
            }
            if let Some(h) = patch.harness {
                task.harness = Some(h);
            }
            if let Some(s) = patch.session {
                task.session = Some(s);
            }
            if let Some(p) = patch.priority {
                task.priority = p;
            }
            emit(json, &db.update(&view.revision, task, "updated", None)?)
        }
        TaskCmd::Receipt {
            id,
            summary,
            next,
            evidence,
            status,
        } => {
            let view = db.view(&id)?;
            let mut task = view.task;
            task.summary = summary.clone();
            task.next = next.clone();
            task.evidence = evidence.clone();
            if let Some(s) = status {
                task.status = s;
            }
            emit(
                json,
                &db.update(
                    &view.revision,
                    task,
                    "receipt",
                    Some(Receipt {
                        summary,
                        next,
                        evidence,
                    }),
                )?,
            )
        }
        TaskCmd::Done {
            id,
            summary,
            evidence,
        } => {
            let view = db.view(&id)?;
            let mut task = view.task;
            task.status = Status::Done;
            task.summary = summary.clone();
            task.evidence = evidence.clone();
            task.next.clear();
            task.blocker.clear();
            task.needs_input = false;
            emit(
                json,
                &db.update(
                    &view.revision,
                    task,
                    "completed",
                    Some(Receipt {
                        summary,
                        next: String::new(),
                        evidence,
                    }),
                )?,
            )
        }
        TaskCmd::Depend { id, on } => {
            let view = db.view(&id)?;
            let dependency = db.view(&on)?;
            let mut task = view.task;
            if !task.depends_on.contains(&dependency.task.id) {
                task.depends_on.push(dependency.task.id);
            }
            emit(json, &db.update(&view.revision, task, "dependency", None)?)
        }
        TaskCmd::Resolve { id, choose } => emit(json, &db.resolve(&id, &choose)?),
    }
}
fn organizer_output(json: bool) -> Result<()> {
    let db = Ledger::open()?;
    let state = organizer::state(&db)?;
    if json {
        emit(true, &state)
    } else {
        println!("Inbox captures: {}", state.captures.len());
        for capture in state.captures {
            println!(
                "{} [{}] {}",
                views::short(&capture.id),
                format!("{:?}", capture.kind).to_lowercase(),
                views::clean(&capture.text)
            );
        }
        println!("Pending/known proposals: {}", state.proposals.len());
        for proposal in state.proposals {
            println!(
                "{} [{:?}] {}",
                views::short(&proposal.id),
                proposal.state,
                views::clean(&proposal.reason)
            );
        }
        Ok(())
    }
}

fn print_view(mode: &str, filter: Filter, json: bool) -> Result<()> {
    if filter.limit == 0 || filter.limit > 10000 {
        bail!("limit must be 1..10000");
    }
    let db = Ledger::open()?;
    let all = db.views()?;
    let state = organizer::state(&db)?;
    let open_captures: Vec<_> = state
        .captures
        .iter()
        .filter(|c| c.conflict || c.directions.is_empty())
        .filter(|c| {
            filter
                .project
                .as_ref()
                .is_none_or(|p| c.project.as_ref() == Some(p))
        })
        .filter(|c| {
            filter
                .search
                .as_ref()
                .is_none_or(|s| c.text.to_lowercase().contains(&s.to_lowercase()))
        })
        .collect();
    let pending: Vec<_> = state
        .proposals
        .iter()
        .filter(|p| {
            matches!(
                p.state,
                organizer::ReviewState::Pending
                    | organizer::ReviewState::Stale
                    | organizer::ReviewState::Conflict
            )
        })
        .filter(|p| {
            filter.project.as_ref().is_none_or(|project| {
                all.iter()
                    .any(|v| v.task.id == p.direction && v.task.project == *project)
            })
        })
        .collect();
    let counts = serde_json::json!({"directions":all.len(),"active":all.iter().filter(|v|v.task.status==Status::Active).count(),"inbox":open_captures.len(),"needs_review":pending.len(),"conflicts":all.iter().filter(|v|v.conflict).count(),"wip_limit":db.config.wip_limit});
    let items = views::select(
        all,
        mode,
        filter.project.as_deref(),
        filter.search.as_deref(),
        filter.days,
        filter.limit,
    );
    let captures: Vec<_> = if mode == "inbox" {
        open_captures.into_iter().take(filter.limit).collect()
    } else {
        Vec::new()
    };
    let proposals: Vec<_> = if matches!(mode, "now" | "needs-me") {
        pending.into_iter().take(filter.limit).collect()
    } else {
        Vec::new()
    };
    if json {
        emit(
            true,
            &serde_json::json!({"view":mode,"counts":counts,"items":items,"captures":captures,"proposals":proposals}),
        )
    } else {
        println!("WorkMesh / {mode} — last reported work, not live session state");
        println!("{counts}");
        for c in &captures {
            println!(
                "{} [{:?}] {}",
                views::short(&c.id),
                c.kind,
                views::clean(&c.text)
            );
        }
        for p in &proposals {
            println!(
                "{} [{:?}] {}",
                views::short(&p.id),
                p.state,
                views::clean(&p.reason)
            );
        }
        if !items.is_empty() || (captures.is_empty() && proposals.is_empty()) {
            views::print_list(&items);
        }
        Ok(())
    }
}
fn ledger(command: LedgerCmd, json: bool) -> Result<()> {
    match command {
        LedgerCmd::Export { output } => {
            let data = Ledger::open()?.bundle();
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)?;
            file.write_all(&serde_json::to_vec(&data)?)?;
            file.sync_all()?;
            emit(
                json,
                &serde_json::json!({"exported":data.events.len(),"path":output}),
            )
        }
        LedgerCmd::Import { file } => {
            let incoming = read_bundle(File::open(file)?)?;
            let mut db = Ledger::open()?;
            let count = db.import(incoming)?;
            emit(json, &serde_json::json!({"imported":count}))
        }
        LedgerCmd::Exchange => {
            let incoming = read_bundle(std::io::stdin().lock())?;
            let mut db = Ledger::open()?;
            db.import(incoming)?;
            serde_json::to_writer(std::io::stdout().lock(), &db.bundle())?;
            Ok(())
        }
        LedgerCmd::Sync { ssh } => {
            if ssh.is_empty()
                || ssh.starts_with('-')
                || !ssh
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "@._-:[]".contains(c))
            {
                bail!("use an SSH alias or user@host; ports belong in SSH config");
            }
            let local = {
                let db = Ledger::open()?;
                db.bundle()
            };
            let bytes = serde_json::to_vec(&local)?;
            let mut child = Command::new("ssh")
                .args([
                    "-T",
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "ConnectTimeout=10",
                    "-o",
                    "ServerAliveInterval=15",
                    "-o",
                    "ServerAliveCountMax=3",
                    "--",
                    &ssh,
                    "workmesh ledger exchange",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .context("start OpenSSH; configure SSH keys and verify host key first")?;
            let mut input = child.stdin.take().context("missing ssh stdin")?;
            let writer =
                std::thread::spawn(move || -> std::io::Result<()> { input.write_all(&bytes) });
            let result = read_bundle(child.stdout.take().context("missing ssh stdout")?);
            if result.is_err() {
                child.kill().ok();
            }
            let status = child.wait()?;
            let wrote = writer
                .join()
                .map_err(|_| anyhow::anyhow!("SSH input writer failed"))?;
            if !status.success() {
                bail!(
                    "SSH exchange failed ({status}); remote may have imported data. Retry is idempotent"
                );
            }
            wrote?;
            let mut db = Ledger::open()?;
            let count = db.import(result?)?;
            emit(
                json,
                &serde_json::json!({"imported":count,"sent_events":local.events.len(),"peer":ssh}),
            )
        }
    }
}
fn emit(json: bool, value: &impl Serialize) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(value)?);
    } else {
        println!("{}", serde_json::to_string_pretty(value)?);
    }
    Ok(())
}
