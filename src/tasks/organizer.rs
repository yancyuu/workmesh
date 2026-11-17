use std::collections::BTreeSet;

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::Serialize;

use super::{
    model::{CaptureKind, InboxAction},
    store::Ledger,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Pending,
    Accepted,
    Rejected,
    Stale,
    Conflict,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProposalView {
    pub id: String,
    pub captures: Vec<String>,
    pub direction: String,
    pub revision: String,
    pub reason: String,
    pub state: ReviewState,
    pub reviews: Vec<String>,
    pub source: String,
    pub machine: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptureView {
    pub id: String,
    pub text: String,
    pub kind: CaptureKind,
    pub project: Option<String>,
    pub source: String,
    pub machine: String,
    pub at: DateTime<Utc>,
    pub directions: Vec<String>,
    pub proposals: Vec<String>,
    pub conflict: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrganizerState {
    pub captures: Vec<CaptureView>,
    pub proposals: Vec<ProposalView>,
}

pub fn resolve(db: &Ledger, prefix: &str) -> Result<String> {
    if prefix.len() < 8 {
        bail!("inbox ID prefix must contain at least 8 characters");
    }
    let matches: Vec<_> = db
        .inbox
        .keys()
        .filter(|id| id.starts_with(prefix))
        .cloned()
        .collect();
    match matches.as_slice() {
        [id] => Ok(id.clone()),
        [] => bail!("inbox item not found: {prefix}"),
        _ => bail!("ambiguous inbox ID; use a longer prefix"),
    }
}

pub fn review_heads(db: &Ledger, proposal_id: &str) -> Vec<String> {
    let mut ids = BTreeSet::new();
    let mut superseded = BTreeSet::new();
    for (id, event) in &db.inbox {
        if let InboxAction::Review {
            proposal,
            supersedes,
            ..
        } = &event.action
            && proposal == proposal_id
        {
            ids.insert(id.clone());
            superseded.extend(supersedes.iter().cloned());
        }
    }
    ids.difference(&superseded).cloned().collect()
}

pub fn state(db: &Ledger) -> Result<OrganizerState> {
    let directions = db.views()?;
    let mut proposals = Vec::new();
    for (id, event) in &db.inbox {
        let InboxAction::Proposal {
            captures,
            direction,
            revision,
            reason,
        } = &event.action
        else {
            continue;
        };
        let mut votes = BTreeSet::new();
        let reviews = review_heads(db, id);
        for review_id in &reviews {
            if let InboxAction::Review { accept, .. } = &db.inbox[review_id].action {
                votes.insert(*accept);
            }
        }
        let state = if votes.len() > 1 {
            ReviewState::Conflict
        } else if votes.contains(&true) {
            ReviewState::Accepted
        } else if votes.contains(&false) {
            ReviewState::Rejected
        } else if directions
            .iter()
            .any(|v| v.task.id == *direction && !v.conflict && v.revision == *revision)
        {
            ReviewState::Pending
        } else {
            ReviewState::Stale
        };
        proposals.push(ProposalView {
            id: id.clone(),
            captures: captures.clone(),
            direction: direction.clone(),
            revision: revision.clone(),
            reason: reason.clone(),
            state,
            reviews,
            source: event.source.clone(),
            machine: event.machine.clone(),
            at: event.at,
        });
    }
    proposals.sort_by(|a, b| b.at.cmp(&a.at).then(a.id.cmp(&b.id)));
    let mut captures = Vec::new();
    for (id, event) in &db.inbox {
        let InboxAction::Capture {
            text,
            kind,
            project,
        } = &event.action
        else {
            continue;
        };
        let related: Vec<_> = proposals
            .iter()
            .filter(|p| p.captures.contains(id))
            .collect();
        let links: BTreeSet<_> = related
            .iter()
            .filter(|p| p.state == ReviewState::Accepted)
            .map(|p| p.direction.clone())
            .collect();
        let conflict = links.len() > 1 || related.iter().any(|p| p.state == ReviewState::Conflict);
        captures.push(CaptureView {
            id: id.clone(),
            text: text.clone(),
            kind: *kind,
            project: project.clone(),
            source: event.source.clone(),
            machine: event.machine.clone(),
            at: event.at,
            directions: links.into_iter().collect(),
            proposals: related.iter().map(|p| p.id.clone()).collect(),
            conflict,
        });
    }
    captures.sort_by(|a, b| b.at.cmp(&a.at).then(a.id.cmp(&b.id)));
    Ok(OrganizerState {
        captures,
        proposals,
    })
}
