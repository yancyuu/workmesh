use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;
use uuid::Uuid;

use crate::model::{FileEntry, Manifest};
use crate::store::Store;
use crate::{FORMAT_VERSION, MAX_FILE_SIZE};

pub struct Capture {
    pub manifest_hash: String,
    pub file_count: usize,
    pub skipped: Vec<String>,
}

pub fn capture(store: &Store) -> Result<Capture> {
    let root = store.root().to_path_buf();
    let store_state = store.state_dir().to_path_buf();
    let git_state = root.join(".git");
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    let walker = WalkBuilder::new(&root)
        .hidden(false)
        .add_custom_ignore_filename(".workmeshignore")
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .filter_entry(move |entry| {
            let path = entry.path();
            !path.starts_with(&git_state) && !path.starts_with(&store_state)
        })
        .build();

    for entry in walker {
        let entry = entry.context("failed to walk workspace")?;
        let path = entry.path();
        if path == root {
            continue;
        }
        let relative = match path.strip_prefix(&root) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let relative_text = slash_path(relative)?;
        let Some(file_type) = entry.file_type() else {
            skipped.push(format!("{relative_text} (unknown file type)"));
            continue;
        };
        if file_type.is_dir() {
            continue;
        }
        if file_type.is_symlink() || !file_type.is_file() {
            skipped.push(format!("{relative_text} (not a regular file)"));
            continue;
        }
        if is_secret(relative) {
            skipped.push(format!("{relative_text} (possible secret)"));
            continue;
        }
        let metadata = fs::metadata(path).with_context(|| format!("reading {}", path.display()))?;
        if metadata.len() > MAX_FILE_SIZE {
            skipped.push(format!(
                "{relative_text} (larger than {MAX_FILE_SIZE} bytes)"
            ));
            continue;
        }
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let hash = store.put_object(&bytes)?;
        files.push(FileEntry {
            path: relative_text,
            hash,
            size: metadata.len(),
            executable: is_executable(&metadata),
        });
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    let file_count = files.len();
    let manifest = Manifest {
        version: FORMAT_VERSION,
        files,
    };
    let manifest_hash = store.put_json(&manifest)?;
    Ok(Capture {
        manifest_hash,
        file_count,
        skipped,
    })
}

pub fn read_manifest(store: &Store, checkpoint_hash: &str) -> Result<Manifest> {
    let checkpoint = store.get_checkpoint(checkpoint_hash)?;
    store.get_json(&checkpoint.manifest)
}

pub fn materialize_empty(store: &Store, checkpoint_hash: &str, target: &Path) -> Result<usize> {
    if target.exists() {
        for entry in
            fs::read_dir(target).with_context(|| format!("reading {}", target.display()))?
        {
            let entry = entry?;
            if entry.file_name() != ".workmesh" {
                bail!("target directory is not empty: {}", target.display());
            }
        }
    } else {
        fs::create_dir_all(target).with_context(|| format!("creating {}", target.display()))?;
    }
    materialize(store, checkpoint_hash, target, None)
}

pub fn materialize_update(
    store: &Store,
    checkpoint_hash: &str,
    target: &Path,
    previous_checkpoint: &str,
) -> Result<usize> {
    materialize(store, checkpoint_hash, target, Some(previous_checkpoint))
}

fn materialize(
    store: &Store,
    checkpoint_hash: &str,
    target: &Path,
    previous_checkpoint: Option<&str>,
) -> Result<usize> {
    let manifest = read_manifest(store, checkpoint_hash)?;
    let mut relative_files = Vec::with_capacity(manifest.files.len());
    for file in &manifest.files {
        let relative = safe_relative(Path::new(&file.path))?;
        store.get_object(&file.hash)?;
        relative_files.push((relative, file));
    }

    let previous_files = previous_checkpoint
        .map(|previous| -> Result<Vec<(PathBuf, FileEntry)>> {
            let old_manifest = read_manifest(store, previous)?;
            let mut old = Vec::new();
            for file in old_manifest.files {
                let relative = safe_relative(Path::new(&file.path))?;
                old.push((relative, file));
            }
            Ok(old)
        })
        .transpose()?;

    let old_paths = previous_files.as_ref().map(|files| {
        files
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<std::collections::BTreeSet<_>>()
    });
    let backup = previous_files.as_ref().map(|_| {
        store
            .state_dir()
            .join("transactions")
            .join(Uuid::new_v4().simple().to_string())
    });
    if let Some(backup) = &backup {
        for (relative, _) in previous_files.as_ref().unwrap() {
            let source = target.join(relative);
            if source.is_file() {
                let saved = backup.join(relative);
                fs::create_dir_all(saved.parent().context("backup parent")?)?;
                fs::copy(&source, &saved)
                    .with_context(|| format!("backing up {}", source.display()))?;
            }
        }
    }

    let mut applied = Vec::new();
    let result = apply_files(
        store,
        target,
        &relative_files,
        old_paths.as_ref(),
        &mut applied,
    );
    let outcome = match result {
        Ok(()) => {
            if let Some(backup) = &backup {
                fs::remove_dir_all(backup).ok();
            }
            Ok(manifest.files.len())
        }
        Err(error) => {
            rollback(
                target,
                backup.as_deref(),
                &applied,
                &relative_files,
                previous_files.as_deref().unwrap_or(&[]),
            );
            Err(error)
        }
    };

    fn apply_files(
        store: &Store,
        target: &Path,
        files: &[(PathBuf, &crate::model::FileEntry)],
        old_paths: Option<&std::collections::BTreeSet<PathBuf>>,
        applied: &mut Vec<PathBuf>,
    ) -> Result<()> {
        if let Some(old_paths) = old_paths {
            let new_paths = files
                .iter()
                .map(|(path, _)| path.clone())
                .collect::<std::collections::BTreeSet<_>>();
            for old in old_paths {
                if !new_paths.contains(old) {
                    let path = target.join(old);
                    if path.is_file() {
                        fs::remove_file(&path)
                            .with_context(|| format!("removing {}", path.display()))?;
                    }
                }
            }
        }
        for (relative, file) in files {
            let destination = target.join(relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
                ensure_within(target, parent)?;
            }
            ensure_within(target, &destination)?;
            let bytes = store.get_object(&file.hash)?;
            let temporary = destination
                .parent()
                .unwrap_or(target)
                .join(format!(".workmesh-tmp-{}", Uuid::new_v4().simple()));
            fs::write(&temporary, bytes)
                .with_context(|| format!("writing {}", temporary.display()))?;
            fs::rename(&temporary, &destination)
                .with_context(|| format!("replacing {}", destination.display()))?;
            set_executable(&destination, file.executable)?;
            applied.push(relative.clone());
        }
        Ok(())
    }

    fn rollback(
        target: &Path,
        backup: Option<&Path>,
        applied: &[PathBuf],
        files: &[(PathBuf, &crate::model::FileEntry)],
        previous_files: &[(PathBuf, crate::model::FileEntry)],
    ) {
        let old_paths = previous_files
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let planned_paths = files
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<std::collections::BTreeSet<_>>();
        for path in applied.iter().chain(planned_paths.iter()) {
            if !old_paths.contains(path) {
                fs::remove_file(target.join(path)).ok();
            }
        }
        if let Some(backup) = backup {
            for (relative, _) in previous_files {
                let saved = backup.join(relative);
                if saved.is_file() {
                    let destination = target.join(relative);
                    if let Some(parent) = destination.parent() {
                        fs::create_dir_all(parent).ok();
                    }
                    fs::copy(&saved, &destination).ok();
                }
            }
            fs::remove_dir_all(backup).ok();
        }
    }

    outcome
}

fn ensure_within(root: &Path, path: &Path) -> Result<()> {
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("resolving {}", root.display()))?;
    let mut existing = path;
    while !existing.exists() {
        existing = existing
            .parent()
            .with_context(|| format!("path has no parent: {}", path.display()))?;
    }
    let canonical = existing
        .canonicalize()
        .with_context(|| format!("resolving {}", existing.display()))?;
    if canonical != canonical_root && !canonical.starts_with(&canonical_root) {
        bail!("snapshot path escapes target: {}", path.display());
    }
    Ok(())
}

pub fn safe_relative(path: &Path) -> Result<PathBuf> {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => output.push(value),
            _ => bail!("unsafe path in snapshot: {}", path.display()),
        }
    }
    if output.as_os_str().is_empty() {
        bail!("empty path in snapshot");
    }
    Ok(output)
}

fn slash_path(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(
                value
                    .to_str()
                    .context("non-UTF-8 paths are not supported")?
                    .to_owned(),
            ),
            _ => bail!("unsafe workspace path: {}", path.display()),
        }
    }
    Ok(parts.join("/"))
}

fn is_secret(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return true;
    };
    let lower = name.to_ascii_lowercase();
    if lower == ".env.example" || lower == ".env.sample" || lower == ".env.template" {
        return false;
    }
    lower == ".env"
        || lower.starts_with(".env.")
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.starts_with("id_rsa")
        || lower.starts_with("id_ed25519")
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn set_executable(path: &Path, executable: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    let mode = permissions.mode();
    permissions.set_mode(if executable {
        mode | 0o111
    } else {
        mode & !0o111
    });
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path, _executable: bool) -> Result<()> {
    Ok(())
}
