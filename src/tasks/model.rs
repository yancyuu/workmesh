use chrono::{DateTime, Utc};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Inbox,
    Ready,
    Active,
    Blocked,
    Waiting,
    Done,
    Archived,
}
impl Status {
    pub fn closed(self) -> bool {
        matches!(self, Self::Done | Self::Archived)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Ready => "ready",
            Self::Active => "active",
            Self::Blocked => "blocked",
            Self::Waiting => "waiting",
            Self::Done => "done",
            Self::Archived => "archived",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Task,
    Bug,
    Feature,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub project: String,
    pub title: String,
    pub goal: String,
    pub kind: Kind,
    pub priority: u8,
    pub status: Status,
    pub parent: Option<String>,
    pub depends_on: Vec<String>,
    pub summary: String,
    pub next: String,
    pub blocker: String,
    pub needs_input: bool,
    pub harness: Option<String>,
    pub session: Option<String>,
    pub git_head: Option<String>,
    pub git_branch: Option<String>,
    pub evidence: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub summary: String,
    pub next: String,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub version: u32,
    pub parents: Vec<String>,
    pub machine_id: String,
    pub machine_name: String,
    pub at: DateTime<Utc>,
    pub action: String,
    pub task: Task,
    pub receipt: Option<Receipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub id: String,
    pub event: Event,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    pub version: u32,
    pub events: Vec<Envelope>,
    #[serde(default)]
    pub inbox: BTreeMap<String, InboxEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub machine_id: String,
    pub machine_name: String,
    pub wip_limit: usize,
    pub projects: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum CaptureKind {
    Idea,
    Question,
    Bug,
    Request,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InboxAction {
    Capture {
        text: String,
        kind: CaptureKind,
        project: Option<String>,
    },
    Proposal {
        captures: Vec<String>,
        direction: String,
        revision: String,
        reason: String,
    },
    Review {
        proposal: String,
        accept: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        supersedes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxEvent {
    pub nonce: String,
    pub machine: String,
    pub source: String,
    pub at: DateTime<Utc>,
    pub action: InboxAction,
}

#[derive(Debug, Clone, Serialize)]
pub struct View {
    pub task: Task,
    pub revision: String,
    pub heads: Vec<String>,
    pub conflict: bool,
    pub machine: String,
    pub updated_at: DateTime<Utc>,
    pub reason: String,
}
