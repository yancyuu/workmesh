use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use fs2::FileExt;
use uuid::Uuid;

use super::model::*;

pub const MAX_BUNDLE: u64 = 128 * 1024 * 1024;

pub struct Ledger {
    root: PathBuf,
    pub config: Config,
    events: BTreeMap<String, Event>,
    pub inbox: BTreeMap<String, InboxEvent>,
    _lock: File,
}

impl Ledger {
    pub fn open() -> Result<Self> {
        let root = home_dir()?;
        fs::create_dir_all(&root)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("ledger.lock"))?;
        lock.lock_exclusive()?;
        let config_path = root.join("config.json");
        let config: Config = if config_path.exists() {
            serde_json::from_slice(&fs::read(config_path)?)?
        } else {
            let id = Uuid::new_v4().to_string();
            let config = Config {
                version: VERSION,
                machine_name: std::env::var("HOSTNAME")
                    .or_else(|_| std::env::var("COMPUTERNAME"))
                    .unwrap_or_else(|_| format!("device-{}", &id[..8])),
                machine_id: id,
                wip_limit: 3,
                projects: BTreeMap::new(),
            };
            atomic_write(
                &root.join("config.json"),
                &serde_json::to_vec_pretty(&config)?,
            )?;
            config
        };
        if config.version != VERSION || config.wip_limit == 0 {
            bail!("unsupported or invalid local config");
        }
        let path = root.join("ledger.json");
        let (events, inbox) = if path.exists() {
            let data = read_bundle(File::open(path)?)?;
            let inbox = data.inbox.clone();
            let events = validate_bundle(data)?;
            validate_inbox(&inbox, &events)?;
            (events, inbox)
        } else {
            (BTreeMap::new(), BTreeMap::new())
        };
        Ok(Self {
            root,
            config,
            events,
            inbox,
            _lock: lock,
        })
    }

    pub fn capture(
        &mut self,
        text: String,
        kind: CaptureKind,
        project: Option<String>,
        source: String,
    ) -> Result<String> {
        if text.trim().is_empty() || text.len() > 32_000 {
            bail!("capture text must be non-empty and at most 32 KiB");
        }
        if let Some(project) = &project {
            slug(project)?;
        }
        let event = InboxEvent {
            nonce: Uuid::new_v4().to_string(),
            machine: self.config.machine_id.clone(),
            source,
            at: Utc::now(),
            action: InboxAction::Capture {
                text,
                kind,
                project,
            },
        };
        self.append_inbox(event)
    }
    pub fn inbox(&self) -> &BTreeMap<String, InboxEvent> {
        &self.inbox
    }
    pub fn propose(
        &mut self,
        captures: Vec<String>,
        direction: String,
        reason: String,
        source: String,
    ) -> Result<String> {
        let target = self.view(&direction)?;
        if target.conflict {
            bail!("resolve direction conflict before proposing");
        }
        if captures.is_empty() || reason.trim().is_empty() {
            bail!("proposal requires captures and reason");
        }
        let mut unique = BTreeSet::new();
        for id in &captures {
            if !unique.insert(id) {
                bail!("duplicate capture in proposal");
            }
            let Some(InboxAction::Capture { project, .. }) = self.inbox.get(id).map(|e| &e.action)
            else {
                bail!("capture not found: {id}");
            };
            if project.as_ref().is_some_and(|p| p != &target.task.project) {
                bail!("capture and direction belong to different projects");
            }
        }
        if target.task.status.closed() {
            bail!("cannot organize into a closed direction");
        }
        if self.inbox.values().any(|event| matches!(&event.action, InboxAction::Proposal { captures: existing, direction, revision, .. } if direction == &target.task.id && revision == &target.revision && existing.iter().collect::<BTreeSet<_>>() == unique)) {
            bail!("equivalent proposal already exists for this revision");
        }
        let event = InboxEvent {
            nonce: Uuid::new_v4().to_string(),
            machine: self.config.machine_id.clone(),
            source,
            at: Utc::now(),
            action: InboxAction::Proposal {
                captures,
                direction: target.task.id,
                revision: target.revision,
                reason,
            },
        };
        self.append_inbox(event)
    }
    pub fn review(
        &mut self,
        proposal: &str,
        accept: bool,
        source: String,
        reason: Option<String>,
    ) -> Result<()> {
        let event = self.inbox.get(proposal).context("proposal not found")?;
        let InboxAction::Proposal {
            direction,
            revision,
            ..
        } = &event.action
        else {
            bail!("not a proposal")
        };
        if accept {
            let target = self.view(direction)?;
            if target.conflict || target.revision != *revision {
                bail!("proposal is stale; ask the agent to propose again");
            }
        }
        let supersedes = crate::tasks::organizer::review_heads(self, proposal);
        let event = InboxEvent {
            nonce: Uuid::new_v4().to_string(),
            machine: self.config.machine_id.clone(),
            source,
            at: Utc::now(),
            action: InboxAction::Review {
                proposal: proposal.to_owned(),
                accept,
                supersedes,
                reason,
            },
        };
        self.append_inbox(event)?;
        Ok(())
    }
    fn append_inbox(&mut self, event: InboxEvent) -> Result<String> {
        let id = blake3::hash(&serde_json::to_vec(&event)?)
            .to_hex()
            .to_string();
        let mut next = self.inbox.clone();
        next.insert(id.clone(), event);
        validate_inbox(&next, &self.events)?;
        let mut data = self.bundle();
        data.inbox = next.clone();
        let bytes = serde_json::to_vec(&data)?;
        if bytes.len() as u64 > MAX_BUNDLE {
            bail!("ledger exceeds 128 MiB");
        }
        atomic_write(&self.root.join("ledger.json"), &bytes)?;
        self.inbox = next;
        Ok(id)
    }

    pub fn configure(&mut self, machine: Option<String>, wip: Option<usize>) -> Result<()> {
        if let Some(name) = machine {
            text(&name, "machine", 128, true)?;
            self.config.machine_name = name;
        }
        if let Some(limit) = wip {
            if !(1..=100).contains(&limit) {
                bail!("WIP limit must be 1..100");
            }
            self.config.wip_limit = limit;
        }
        self.save_config()
    }

    pub fn project_add(&mut self, name: String, path: &Path) -> Result<()> {
        slug(&name)?;
        let path = path
            .canonicalize()
            .context("project directory must exist")?;
        if !path.is_dir() {
            bail!("project path must be a directory");
        }
        self.config.projects.insert(
            name,
            path.to_str()
                .context("project path must be UTF-8")?
                .to_owned(),
        );
        self.save_config()
    }

    pub fn create(&mut self, mut task: Task, allow_duplicate: bool) -> Result<View> {
        if !self.config.projects.contains_key(&task.project) {
            bail!("register this project first: workmesh project add NAME --path PATH");
        }
        if !allow_duplicate
            && self.views()?.iter().any(|v| {
                !v.task.status.closed()
                    && v.task.project == task.project
                    && normalized(&v.task.title) == normalized(&task.title)
            })
        {
            bail!(
                "duplicate open title in this project; reuse the task or pass --allow-duplicate deliberately"
            );
        }
        task.id = format!("wm-{}", Uuid::new_v4().simple());
        if let Some(parent) = &task.parent {
            task.parent = Some(self.view(parent)?.task.id);
        }
        task.created_at = Utc::now();
        observe_git(&self.config, &mut task);
        let id = task.id.clone();
        self.append(task, Vec::new(), "created", None)?;
        self.view(&id)
    }

    pub fn update(
        &mut self,
        revision: &str,
        mut task: Task,
        action: &str,
        receipt: Option<Receipt>,
    ) -> Result<View> {
        let current = self.view(&task.id)?;
        if current.conflict {
            bail!("task has concurrent revisions; use task resolve before editing");
        }
        if current.revision != revision {
            bail!("task changed since it was read; reload and retry");
        }
        if task.created_at != current.task.created_at || task.project != current.task.project {
            bail!("task project and creation time are immutable");
        }
        let id = task.id.clone();
        observe_git(&self.config, &mut task);
        self.append(task, current.heads, action, receipt)?;
        self.view(&id)
    }

    pub fn resolve(&mut self, id: &str, choose: &str) -> Result<View> {
        let view = self.view(id)?;
        if !view.conflict {
            bail!("task has no conflict");
        }
        let choices: Vec<_> = view
            .heads
            .iter()
            .filter(|h| h.starts_with(choose))
            .collect();
        if choose.len() < 8 || choices.len() != 1 {
            bail!("choose a unique current head revision (at least 8 characters)");
        }
        let task = self.events[choices[0]].task.clone();
        self.append(task, view.heads, "resolved", None)?;
        self.view(&view.task.id)
    }

    pub fn view(&self, prefix: &str) -> Result<View> {
        if prefix.len() < 4 {
            bail!("task ID prefix must be at least 4 characters");
        }
        let matches = self
            .views()?
            .into_iter()
            .filter(|v| v.task.id.starts_with(prefix))
            .collect::<Vec<_>>();
        match matches.len() {
            0 => bail!("task not found: {prefix}"),
            1 => Ok(matches.into_iter().next().unwrap()),
            _ => bail!("ambiguous task ID; use a longer prefix"),
        }
    }

    pub fn views(&self) -> Result<Vec<View>> {
        views(&self.events)
    }

    pub fn history(&self, id: &str) -> Result<Vec<Envelope>> {
        let id = self.view(id)?.task.id;
        let mut events: Vec<_> = self
            .events
            .iter()
            .filter(|(_, e)| e.task.id == id)
            .map(|(id, event)| Envelope {
                id: id.clone(),
                event: event.clone(),
            })
            .collect();
        events.sort_by(|a, b| a.event.at.cmp(&b.event.at).then(a.id.cmp(&b.id)));
        Ok(events)
    }

    pub fn bundle(&self) -> Bundle {
        let mut data = bundle(&self.events);
        data.inbox = self.inbox.clone();
        data
    }

    pub fn import(&mut self, incoming: Bundle) -> Result<usize> {
        let incoming_inbox = incoming.inbox.clone();
        let incoming = validate_bundle(incoming)?;
        let mut combined = self.events.clone();
        let mut inbox = self.inbox.clone();
        let before = combined.len() + inbox.len();
        combined.extend(incoming);
        inbox.extend(incoming_inbox);
        validate_graph(&combined)?;
        validate_relationships(&views(&combined)?)?;
        validate_inbox(&inbox, &combined)?;
        let count = combined.len() + inbox.len() - before;
        if count > 0 {
            let mut data = bundle(&combined);
            data.inbox = inbox.clone();
            let bytes = serde_json::to_vec(&data)?;
            if bytes.len() as u64 > MAX_BUNDLE {
                bail!("merged ledger exceeds 128 MiB");
            }
            atomic_write(&self.root.join("ledger.json"), &bytes)?;
            self.events = combined;
            self.inbox = inbox;
        }
        Ok(count)
    }

    fn append(
        &mut self,
        task: Task,
        parents: Vec<String>,
        action: &str,
        receipt: Option<Receipt>,
    ) -> Result<()> {
        validate_task(&task)?;
        let current = self.views()?;
        let was_active = current
            .iter()
            .any(|v| v.task.id == task.id && !v.conflict && v.task.status == Status::Active);
        if task.status == Status::Active && !was_active {
            let active = current
                .iter()
                .filter(|v| v.task.id != task.id && v.task.status == Status::Active && !v.conflict)
                .count();
            if active >= self.config.wip_limit {
                bail!(
                    "WIP limit {} reached; move another task out of active first",
                    self.config.wip_limit
                );
            }
        }
        for other in &current {
            let dependency = task.depends_on.contains(&other.task.id);
            let child = other.task.parent.as_ref() == Some(&task.id);
            if ((dependency && matches!(task.status, Status::Active | Status::Done))
                || (child && task.status == Status::Done))
                && (other.conflict || other.task.status != Status::Done)
            {
                bail!(
                    "unfinished dependency/child {} prevents this transition",
                    other.task.id
                );
            }
        }
        let event = Event {
            version: VERSION,
            parents,
            machine_id: self.config.machine_id.clone(),
            machine_name: self.config.machine_name.clone(),
            at: Utc::now(),
            action: action.into(),
            task,
            receipt,
        };
        let id = hash(&event)?;
        let mut updated = self.events.clone();
        updated.insert(id, event);
        validate_graph(&updated)?;
        validate_relationships(&views(&updated)?)?;
        self.persist(&updated)?;
        self.events = updated;
        Ok(())
    }

    fn persist(&self, events: &BTreeMap<String, Event>) -> Result<()> {
        let mut data = bundle(events);
        data.inbox = self.inbox.clone();
        let bytes = serde_json::to_vec(&data)?;
        if bytes.len() as u64 > MAX_BUNDLE {
            bail!("ledger exceeds 128 MiB; export and split your ledger before adding more");
        }
        atomic_write(&self.root.join("ledger.json"), &bytes)
    }
    fn save_config(&self) -> Result<()> {
        atomic_write(
            &self.root.join("config.json"),
            &serde_json::to_vec_pretty(&self.config)?,
        )
    }
}

pub fn read_bundle(reader: impl Read) -> Result<Bundle> {
    let mut bytes = Vec::new();
    reader.take(MAX_BUNDLE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BUNDLE {
        bail!("bundle exceeds 128 MiB");
    }
    serde_json::from_slice(&bytes).context("invalid ledger bundle JSON")
}

fn bundle(events: &BTreeMap<String, Event>) -> Bundle {
    Bundle {
        version: VERSION,
        events: events
            .iter()
            .map(|(id, event)| Envelope {
                id: id.clone(),
                event: event.clone(),
            })
            .collect(),
        inbox: BTreeMap::new(),
    }
}

fn validate_inbox(
    inbox: &BTreeMap<String, InboxEvent>,
    events: &BTreeMap<String, Event>,
) -> Result<()> {
    if inbox.len() > 100_000 {
        bail!("too many inbox events");
    }
    for (id, event) in inbox {
        if blake3::hash(&serde_json::to_vec(event)?).to_hex().as_str() != id {
            bail!("inbox integrity check failed");
        }
        text(&event.source, "source", 256, true)?;
        text(&event.machine, "machine", 128, true)?;
        Uuid::parse_str(&event.nonce).context("invalid inbox nonce")?;
        if event.at > Utc::now() + chrono::Duration::minutes(5) {
            bail!("inbox timestamp is in the future; check device clock");
        }
        match &event.action {
            InboxAction::Capture {
                text: raw, project, ..
            } => {
                text(raw, "capture", 32000, true)?;
                if let Some(project) = project {
                    slug(project)?;
                }
            }
            InboxAction::Proposal {
                captures,
                direction,
                revision,
                reason,
            } => {
                text(reason, "reason", 16384, true)?;
                if captures.is_empty()
                    || captures.len() > 100
                    || captures.iter().collect::<BTreeSet<_>>().len() != captures.len()
                {
                    bail!("invalid proposal sources");
                }
                let target = events
                    .get(revision)
                    .context("proposal references missing direction revision")?;
                if target.task.id != *direction || target.task.status.closed() {
                    bail!("proposal direction mismatch or closed target revision");
                }
                for capture in captures {
                    let Some(InboxAction::Capture { project, .. }) =
                        inbox.get(capture).map(|e| &e.action)
                    else {
                        bail!("proposal source is missing or not a capture")
                    };
                    if project.as_ref().is_some_and(|p| p != &target.task.project) {
                        bail!("proposal crosses project boundaries");
                    }
                }
            }
            InboxAction::Review {
                proposal,
                supersedes,
                reason,
                ..
            } => {
                if !matches!(
                    inbox.get(proposal).map(|e| &e.action),
                    Some(InboxAction::Proposal { .. })
                ) {
                    bail!("review references missing proposal");
                }
                if supersedes.len() > 1000
                    || supersedes.iter().collect::<BTreeSet<_>>().len() != supersedes.len()
                {
                    bail!("invalid superseded review set");
                }
                if !supersedes.is_empty() {
                    text(
                        reason
                            .as_deref()
                            .context("superseding a decision requires a reason")?,
                        "review reason",
                        16384,
                        true,
                    )?;
                } else if let Some(reason) = reason {
                    text(reason, "review reason", 16384, true)?;
                }
                for previous in supersedes {
                    if previous == id
                        || !matches!(inbox.get(previous).map(|e| &e.action), Some(InboxAction::Review { proposal: p, .. }) if p == proposal)
                    {
                        bail!("superseded review must belong to the same proposal");
                    }
                }
            }
        }
    }
    let mut review_edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, event) in inbox {
        if let InboxAction::Review { supersedes, .. } = &event.action {
            review_edges.insert(id.clone(), supersedes.clone());
        }
    }
    for start in review_edges.keys() {
        let mut pending = review_edges.get(start).cloned().unwrap_or_default();
        let mut seen = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if id == *start {
                bail!("review supersession cycle detected");
            }
            if seen.insert(id.clone()) {
                pending.extend(review_edges.get(&id).cloned().unwrap_or_default());
            }
        }
    }
    Ok(())
}

fn validate_bundle(bundle: Bundle) -> Result<BTreeMap<String, Event>> {
    if bundle.version != VERSION || bundle.events.len() > 100_000 {
        bail!("unsupported or oversized bundle");
    }
    let mut events = BTreeMap::new();
    for item in bundle.events {
        if item.event.version != VERSION || hash(&item.event)? != item.id {
            bail!("event integrity check failed");
        }
        validate_task(&item.event.task)?;
        text(&item.event.machine_id, "machine ID", 128, true)?;
        text(&item.event.machine_name, "machine name", 128, true)?;
        if item.event.at > Utc::now() + chrono::Duration::minutes(5) {
            bail!("event timestamp is in the future; check device clock");
        }
        if item.event.parents.len() > 1000 {
            bail!("too many parents");
        }
        if let Some(receipt) = &item.event.receipt {
            text(&receipt.summary, "receipt", 16384, true)?;
            text(&receipt.next, "next", 16384, false)?;
            if receipt.evidence.len() > 100 {
                bail!("too much receipt evidence");
            }
            for evidence in &receipt.evidence {
                text(evidence, "evidence", 16384, true)?;
            }
        }
        events.insert(item.id, item.event);
    }
    validate_graph(&events)?;
    Ok(events)
}

fn validate_graph(events: &BTreeMap<String, Event>) -> Result<()> {
    let mut degrees = BTreeMap::new();
    let mut children = BTreeMap::<String, Vec<String>>::new();
    let mut roots = BTreeSet::new();
    for (id, event) in events {
        if event.parents.is_empty() && !roots.insert(event.task.id.clone()) {
            bail!("multiple roots for a task");
        }
        let mut unique = BTreeSet::new();
        for parent in &event.parents {
            if !unique.insert(parent) {
                bail!("duplicate parent");
            }
            let ancestor = events.get(parent).context("missing parent in bundle")?;
            if ancestor.task.id != event.task.id
                || ancestor.task.project != event.task.project
                || ancestor.task.created_at != event.task.created_at
            {
                bail!("invalid cross-task ancestry");
            }
            children.entry(parent.clone()).or_default().push(id.clone());
        }
        degrees.insert(id.clone(), event.parents.len());
    }
    let mut ready = degrees
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(id, _)| id.clone())
        .collect::<VecDeque<_>>();
    let mut visited = 0;
    while let Some(id) = ready.pop_front() {
        visited += 1;
        for child in children.get(&id).into_iter().flatten() {
            let degree = degrees.get_mut(child).unwrap();
            *degree -= 1;
            if *degree == 0 {
                ready.push_back(child.clone());
            }
        }
    }
    if visited != events.len() {
        bail!("cyclic event history");
    }
    Ok(())
}

fn views(events: &BTreeMap<String, Event>) -> Result<Vec<View>> {
    let ancestors = events
        .values()
        .flat_map(|e| e.parents.iter())
        .collect::<BTreeSet<_>>();
    let mut groups = BTreeMap::<String, Vec<String>>::new();
    for (id, event) in events {
        if !ancestors.contains(id) {
            groups
                .entry(event.task.id.clone())
                .or_default()
                .push(id.clone());
        }
    }
    let mut result = Vec::new();
    for heads in groups.values() {
        let mut candidates = heads.clone();
        candidates.sort_by(|a, b| events[b].at.cmp(&events[a].at).then(a.cmp(b)));
        let id = &candidates[0];
        let event = &events[id];
        result.push(View {
            task: event.task.clone(),
            revision: id.clone(),
            heads: heads.clone(),
            conflict: heads.len() > 1,
            machine: event.machine_name.clone(),
            updated_at: event.at,
            reason: String::new(),
        });
    }
    Ok(result)
}

fn validate_relationships(views: &[View]) -> Result<()> {
    let tasks = views
        .iter()
        .map(|v| (v.task.id.as_str(), &v.task))
        .collect::<BTreeMap<_, _>>();
    let mut edges = BTreeMap::<&str, BTreeSet<&str>>::new();
    for task in tasks.values() {
        for dependency in &task.depends_on {
            if !tasks.contains_key(dependency.as_str()) {
                bail!("unknown dependency {dependency}");
            }
            edges.entry(&task.id).or_default().insert(dependency);
        }
        if let Some(parent) = &task.parent {
            let parent_task = tasks.get(parent.as_str()).context("unknown parent task")?;
            if parent_task.project != task.project {
                bail!("parent must belong to the same project");
            }
            edges.entry(parent).or_default().insert(&task.id);
        }
    }
    for start in tasks.keys() {
        let mut todo = edges
            .get(start)
            .into_iter()
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        let mut seen = BTreeSet::new();
        while let Some(id) = todo.pop() {
            if &id == start {
                bail!("dependency/parent cycle detected");
            }
            if seen.insert(id) {
                todo.extend(edges.get(id).into_iter().flatten().copied());
            }
        }
    }
    Ok(())
}

fn validate_task(t: &Task) -> Result<()> {
    if !t.id.starts_with("wm-")
        || t.id.len() != 35
        || !t.id[3..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        bail!("invalid task ID");
    }
    slug(&t.project)?;
    text(&t.title, "title", 512, true)?;
    for value in [&t.goal, &t.summary, &t.next, &t.blocker] {
        text(value, "task text", 16384, false)?;
    }
    if t.priority > 3 || t.depends_on.len() > 100 || t.evidence.len() > 100 {
        bail!("invalid priority or too many links");
    }
    for item in &t.evidence {
        text(item, "evidence", 16384, true)?;
    }
    for item in [&t.harness, &t.session, &t.git_head, &t.git_branch]
        .into_iter()
        .flatten()
    {
        text(item, "reference", 2048, false)?;
    }
    if matches!(t.status, Status::Ready | Status::Active | Status::Waiting)
        && t.next.trim().is_empty()
    {
        bail!("ready/active/waiting requires --next");
    }
    if t.status == Status::Blocked && t.blocker.trim().is_empty() {
        bail!("blocked requires --blocker");
    }
    if t.status == Status::Done && (t.summary.trim().is_empty() || t.evidence.is_empty()) {
        bail!(
            "done requires a summary and evidence; evidence is reported, not independently verified"
        );
    }
    Ok(())
}

fn normalized(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
fn hash(event: &Event) -> Result<String> {
    Ok(blake3::hash(&serde_json::to_vec(event)?)
        .to_hex()
        .to_string())
}
fn text(value: &str, label: &str, max: usize, required: bool) -> Result<()> {
    if value.len() > max
        || (required && value.trim().is_empty())
        || value
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        bail!("invalid {label}: empty, oversized, or unsafe control characters");
    }
    Ok(())
}
fn slug(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("project slug must be 1-64 ASCII letters/digits/-/_");
    }
    Ok(())
}
fn observe_git(config: &Config, task: &mut Task) {
    task.git_head = None;
    task.git_branch = None;
    if let Some(path) = config.projects.get(&task.project) {
        let query = |args: &[&str]| -> Option<String> {
            let result = Command::new("git")
                .env("GIT_OPTIONAL_LOCKS", "0")
                .arg("-C")
                .arg(path)
                .args(args)
                .output()
                .ok()?;
            result
                .status
                .success()
                .then(|| String::from_utf8_lossy(&result.stdout).trim().to_owned())
        };
        task.git_head = query(&["rev-parse", "HEAD"]);
        task.git_branch = query(&["symbolic-ref", "--short", "HEAD"]);
    }
}
pub fn home_dir() -> Result<PathBuf> {
    if let Some(value) = std::env::var_os("WORKMESH_HOME") {
        return Ok(PathBuf::from(value));
    }
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .context("LOCALAPPDATA not set")?;
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|v| PathBuf::from(v).join(".local/share")))
        .context("HOME not set")?;
    Ok(base.join("workmesh"))
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("missing parent")?;
    let temp = parent.join(format!(".{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp, path)?;
    Ok(())
}
