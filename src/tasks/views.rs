use super::model::{Status, View};
use chrono::{Duration, Utc};

pub fn select(
    mut items: Vec<View>,
    mode: &str,
    project: Option<&str>,
    search: Option<&str>,
    days: i64,
    limit: usize,
) -> Vec<View> {
    let boundary = Utc::now() - Duration::days(days.clamp(1, 36500));
    for item in &mut items {
        item.reason = if item.conflict {
            "CONFLICT: choose a revision".into()
        } else if item.task.needs_input {
            "needs your decision".into()
        } else if item.task.status == Status::Blocked {
            format!("blocked: {}", item.task.blocker)
        } else if item.updated_at < boundary {
            "stale: verify last reported state".into()
        } else if item.task.status == Status::Active {
            "active: continue next step".into()
        } else if item.task.next.is_empty() {
            "triage: missing next step".into()
        } else {
            "ready for next step".into()
        };
    }
    items.retain(|v| {
        if project.is_some_and(|p| v.task.project != p) {
            return false;
        }
        if search.is_some_and(|s| {
            !format!("{} {} {}", v.task.title, v.task.goal, v.task.summary)
                .to_lowercase()
                .contains(&s.to_lowercase())
        }) {
            return false;
        }
        match mode {
            "all" => true,
            "inbox" => v.task.status == Status::Inbox || v.conflict,
            "blocked" => v.task.status == Status::Blocked || !v.task.blocker.is_empty(),
            "stale" => !v.task.status.closed() && v.updated_at < boundary,
            "needs-me" => v.task.needs_input || v.conflict,
            _ => {
                v.conflict
                    || (!v.task.status.closed()
                        && (v.task.status != Status::Inbox || v.task.needs_input))
            }
        }
    });
    items.sort_by_key(|v| {
        (
            if v.conflict {
                0
            } else if v.task.needs_input {
                1
            } else if v.task.status == Status::Active {
                2
            } else if v.task.status == Status::Blocked {
                3
            } else if v.updated_at < boundary {
                4
            } else {
                5
            },
            v.task.priority,
            v.updated_at,
            v.task.id.clone(),
        )
    });
    items.truncate(limit);
    items
}

pub fn print_list(items: &[View]) {
    if items.is_empty() {
        println!("Nothing in this view. Try `workmesh inbox` or `workmesh task list`.");
        return;
    }
    for item in items {
        println!(
            "{}  {} / {}  [{} P{}]",
            short(&item.task.id),
            clean(&item.task.project),
            clean(&item.task.title),
            if item.conflict {
                "CONFLICT"
            } else {
                item.task.status.label()
            },
            item.task.priority
        );
        println!(
            "  {} | last reported {} by {}",
            clean(&item.reason),
            item.updated_at.format("%Y-%m-%d %H:%M UTC"),
            clean(&item.machine)
        );
        println!(
            "  next: {}",
            if item.task.next.is_empty() {
                "(not recorded)"
            } else {
                &item.task.next
            }
        );
    }
}
pub fn short(id: &str) -> String {
    id.chars().take(11).collect()
}
pub fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
