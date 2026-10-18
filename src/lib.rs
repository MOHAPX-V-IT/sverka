use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use walkdir::WalkDir;

pub type Result<T> = std::result::Result<T, String>;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub size: u64,
    pub modified_ns: u128,
    pub sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub schema: u32,
    pub root: String,
    pub created_unix: u64,
    pub excludes: Vec<String>,
    pub symlinks: String,
    pub files: BTreeMap<String, Entry>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointHeader {
    schema: u32,
    root: String,
    excludes: Vec<String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckpointEntry {
    path: String,
    entry: Entry,
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Changes {
    pub added: Vec<String>,
    pub missing: Vec<String>,
    pub changed: Vec<String>,
    pub moved: Vec<Move>,
    pub unchanged: usize,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Move {
    pub from: String,
    pub to: String,
}
impl Changes {
    pub fn has_changes(&self) -> bool {
        !self.added.is_empty()
            || !self.missing.is_empty()
            || !self.changed.is_empty()
            || !self.moved.is_empty()
    }
}
fn stat(path: &Path) -> Result<(u64, u128)> {
    let m = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !m.is_file() || m.file_type().is_symlink() {
        return Err(format!("not a regular file: {}", path.display()));
    }
    let modified = m
        .modified()
        .map_err(|e| e.to_string())?
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    Ok((m.len(), modified))
}
pub fn hash_file(path: &Path) -> Result<Entry> {
    let before = stat(path)?;
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 65536];
    let mut total = 0;
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        hash.update(&buffer[..n]);
    }
    if stat(path)? != before || total != before.0 {
        return Err(format!("file changed while reading: {}", path.display()));
    }
    Ok(Entry {
        size: total,
        modified_ns: before.1,
        sha256: format!("{:x}", hash.finalize()),
    })
}
fn portable(path: &Path) -> Result<String> {
    path.to_str()
        .map(|s| s.replace('\\', "/"))
        .ok_or_else(|| format!("non-UTF8 path: {}", path.display()))
}
fn validate_relative(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\\')
        && !name.starts_with('/')
        && !name.contains(':')
        && name
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != "..")
}
pub fn load(path: &Path) -> Result<Snapshot> {
    let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let s: Snapshot = serde_json::from_reader(f).map_err(|e| format!("invalid snapshot: {e}"))?;
    if s.schema != 1 || s.symlinks != "skip" {
        return Err("unsupported snapshot schema or symlink policy".into());
    }
    for (name, e) in &s.files {
        if !validate_relative(name)
            || e.sha256.len() != 64
            || !e
                .sha256
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return Err(format!("invalid entry: {name}"));
        }
    }
    Ok(s)
}
fn excluded(name: &str, excludes: &[String]) -> bool {
    excludes
        .iter()
        .any(|x| name == x || name.starts_with(&(x.clone() + "/")))
}
fn absolute(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let parent = path
        .parent()
        .ok_or("output requires parent")?
        .canonicalize()
        .map_err(|e| format!("output parent: {e}"))?;
    Ok(parent.join(path.file_name().ok_or("output requires filename")?))
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dest = absolute(path)?;
    let temp = dest.with_file_name(format!(
        ".{}.{}.tmp",
        dest.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| format!("temporary file {}: {e}", temp.display()))?;
        f.write_all(bytes).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        drop(f);
        fs::rename(&temp, &dest).map_err(|e| format!("atomic replace {}: {e}", dest.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
pub fn create_snapshot(
    root: &Path,
    output: Option<&Path>,
    checkpoint: Option<&Path>,
    resume: bool,
    workers: usize,
    mut excludes: Vec<String>,
) -> Result<Snapshot> {
    if !(1..=64).contains(&workers) {
        return Err("workers must be between 1 and 64".into());
    }
    let root = root.canonicalize().map_err(|e| format!("root: {e}"))?;
    if !root.is_dir() {
        return Err("root is not a directory".into());
    }
    let root_name = root.to_str().ok_or("non-UTF8 root")?.to_owned();
    if let (Some(a), Some(b)) = (output, checkpoint)
        && absolute(a)? == absolute(b)?
    {
        return Err("output and checkpoint must be different files".into());
    }
    for path in [output, checkpoint].into_iter().flatten() {
        if let Ok(meta) = fs::symlink_metadata(path)
            && (meta.file_type().is_symlink() || !meta.is_file())
        {
            return Err(format!("refusing non-regular output: {}", path.display()));
        }
    }
    if let Some(path) = output
        && path.exists()
    {
        load(path).map_err(|e| format!("refusing to overwrite a non-snapshot file: {e}"))?;
    }
    for x in &mut excludes {
        *x = x.trim_matches('/').replace('\\', "/");
        if !validate_relative(x) {
            return Err(format!("exclude must be a relative path prefix: {x}"));
        }
    }
    for path in [output, checkpoint].into_iter().flatten() {
        let p = absolute(path)?;
        if let Ok(relative) = p.strip_prefix(&root) {
            excludes.push(portable(relative)?);
        }
    }
    excludes.sort();
    excludes.dedup();
    let header = CheckpointHeader {
        schema: 1,
        root: root_name.clone(),
        excludes: excludes.clone(),
    };
    let mut cached = BTreeMap::new();
    let journal = if let Some(path) = checkpoint {
        if path.exists() && !resume {
            let bytes = fs::read(path).map_err(|e| e.to_string())?;
            let first = bytes
                .split(|b| *b == b'\n')
                .next()
                .ok_or("empty checkpoint")?;
            let old: CheckpointHeader = serde_json::from_slice(first)
                .map_err(|_| "refusing to overwrite a non-checkpoint file")?;
            if old.schema != 1 {
                return Err("unsupported checkpoint schema".into());
            }
        }
        if resume && path.exists() {
            let bytes = fs::read(path).map_err(|e| e.to_string())?;
            if !bytes.contains(&b'\n') {
                return Err("incomplete checkpoint header".into());
            }
            let mut lines = bytes.split(|b| *b == b'\n');
            let first = lines.next().ok_or("empty checkpoint")?;
            let old: CheckpointHeader =
                serde_json::from_slice(first).map_err(|e| format!("checkpoint header: {e}"))?;
            if old.schema != 1 || old.root != header.root || old.excludes != header.excludes {
                return Err("checkpoint belongs to another root/options".into());
            }
            let mut valid = first.len() + 1;
            let rest: Vec<_> = lines.collect();
            for (i, line) in rest.iter().enumerate() {
                if i == rest.len() - 1 && !bytes.ends_with(b"\n") {
                    break;
                }
                if line.is_empty() {
                    if i != rest.len() - 1 {
                        valid += 1;
                    }
                    continue;
                }
                match serde_json::from_slice::<CheckpointEntry>(line) {
                    Ok(v) => {
                        if !validate_relative(&v.path)
                            || v.entry.sha256.len() != 64
                            || !v
                                .entry
                                .sha256
                                .bytes()
                                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
                        {
                            return Err("invalid checkpoint entry".into());
                        }
                        cached.insert(v.path, v.entry);
                        valid += line.len() + 1;
                    }
                    Err(e) => return Err(format!("corrupt checkpoint record: {e}")),
                }
            }
            let f = OpenOptions::new()
                .write(true)
                .open(path)
                .map_err(|e| e.to_string())?;
            f.set_len(valid.min(bytes.len()) as u64)
                .map_err(|e| e.to_string())?;
            drop(f);
        } else {
            let mut b = serde_json::to_vec(&header).map_err(|e| e.to_string())?;
            b.push(b'\n');
            atomic_write(path, &b)?;
        }
        Some(Mutex::new(
            OpenOptions::new()
                .append(true)
                .open(path)
                .map_err(|e| e.to_string())?,
        ))
    } else {
        if resume {
            return Err("--resume requires --checkpoint".into());
        }
        None
    };
    let mut tasks = Vec::new();
    let mut files = BTreeMap::new();
    let walker = WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0
                || portable(e.path().strip_prefix(&root).unwrap())
                    .map(|s| !excluded(&s, &excludes))
                    .unwrap_or(true)
        });
    for item in walker {
        let item = item.map_err(|e| e.to_string())?;
        if item.file_type().is_symlink() {
            continue;
        }
        if item.file_type().is_file() {
            let relative = portable(item.path().strip_prefix(&root).unwrap())?;
            if !validate_relative(&relative) {
                return Err(format!("unsupported path: {relative}"));
            }
            if let Some(old) = cached.get(&relative)
                && stat(item.path())? == (old.size, old.modified_ns)
            {
                files.insert(relative, old.clone());
                continue;
            }
            tasks.push((relative, item.path().to_path_buf()));
        }
    }
    let cursor = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    let failures = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let tasks = &tasks;
            let cursor = &cursor;
            let results = &results;
            let failures = &failures;
            let journal = &journal;
            scope.spawn(move || {
                loop {
                    let i = cursor.fetch_add(1, Ordering::Relaxed);
                    let Some((name, path)) = tasks.get(i) else {
                        break;
                    };
                    match hash_file(path) {
                        Ok(entry) => {
                            if let Some(j) = journal {
                                let mut f = j.lock().unwrap();
                                let record = CheckpointEntry {
                                    path: name.clone(),
                                    entry: entry.clone(),
                                };
                                let record = serde_json::to_vec(&record).unwrap();
                                if let Err(e) = f
                                    .write_all(&record)
                                    .and_then(|_| f.write_all(b"\n"))
                                    .and_then(|_| f.sync_data())
                                {
                                    failures.lock().unwrap().push(format!("checkpoint: {e}"));
                                    continue;
                                }
                            }
                            results.lock().unwrap().push((name.clone(), entry));
                        }
                        Err(e) => failures.lock().unwrap().push(e),
                    }
                }
            });
        }
    });
    let errors = failures.into_inner().unwrap();
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    files.extend(results.into_inner().unwrap());
    let snapshot = Snapshot {
        schema: 1,
        root: root_name,
        created_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        excludes,
        symlinks: "skip".into(),
        files,
    };
    if let Some(path) = output {
        let mut bytes = serde_json::to_vec_pretty(&snapshot).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        atomic_write(path, &bytes)?;
    }
    Ok(snapshot)
}
pub fn diff(old: &Snapshot, new: &Snapshot) -> Changes {
    let mut result = Changes::default();
    let mut removed = BTreeSet::new();
    let mut added = BTreeSet::new();
    for (name, a) in &old.files {
        match new.files.get(name) {
            Some(b) if a.sha256 == b.sha256 && a.size == b.size => result.unchanged += 1,
            Some(_) => result.changed.push(name.clone()),
            None => {
                removed.insert(name.clone());
            }
        }
    }
    for name in new.files.keys() {
        if !old.files.contains_key(name) {
            added.insert(name.clone());
        }
    }
    let mut old_hash: BTreeMap<(String, u64), Vec<String>> = BTreeMap::new();
    let mut new_hash: BTreeMap<(String, u64), Vec<String>> = BTreeMap::new();
    for name in &removed {
        let e = &old.files[name];
        old_hash
            .entry((e.sha256.clone(), e.size))
            .or_default()
            .push(name.clone());
    }
    for name in &added {
        let e = &new.files[name];
        new_hash
            .entry((e.sha256.clone(), e.size))
            .or_default()
            .push(name.clone());
    }
    for (hash, names) in old_hash {
        if names.len() == 1
            && let Some(new_names) = new_hash.get(&hash)
            && new_names.len() == 1
        {
            removed.remove(&names[0]);
            added.remove(&new_names[0]);
            result.moved.push(Move {
                from: names[0].clone(),
                to: new_names[0].clone(),
            });
        }
    }
    result.moved.sort_by(|a, b| a.from.cmp(&b.from));
    result.missing = removed.into_iter().collect();
    result.added = added.into_iter().collect();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_sha() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a");
        fs::write(&p, b"abc").unwrap();
        assert_eq!(
            hash_file(&p).unwrap().sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
    #[test]
    fn rejects_traversal() {
        assert!(!validate_relative("../secret"));
        assert!(!validate_relative("C:/secret"));
        assert!(validate_relative("docs/пример.txt"));
    }
}
