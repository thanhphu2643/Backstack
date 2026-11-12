//! Finds tenant backups in folders and S3 buckets: extracted folders (metadata.json +
//! database.sql + files/), .tar.gz archives of the same, and pg_dump custom-format .dump files.

use crate::{s3, stamp};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Folder,
    Archive,
    Dump,
}

#[derive(Serialize, Debug, Clone)]
pub struct Copy {
    /// A local path, or `s3://bucket/key`.
    pub path: String,
    pub kind: Kind,
    pub size: u64,
    pub remote: bool,
}

#[derive(Serialize, Debug, Clone)]
pub struct Backup {
    /// Path of the first copy; stable across scans.
    pub id: String,
    pub tenant: String,
    /// Company name from the tenant config.
    pub company: Option<String>,
    /// When the backup was taken, seconds since 1970.
    pub taken_at: Option<u64>,
    pub app_version: Option<String>,
    pub includes: Vec<String>,
    /// Database name on the server the backup came from.
    pub db_name: Option<String>,
    /// No metadata.json: an old archive or a bare pg_dump file.
    pub legacy: bool,
    /// The same backup can be a folder, its archive, and the archive's copy in S3.
    /// Local copies come first.
    pub copies: Vec<Copy>,
}

#[derive(Serialize, Debug, Default)]
pub struct Scan {
    pub backups: Vec<Backup>,
    /// One per bucket that could not be listed.
    pub errors: Vec<String>,
}

/// The parts of metadata.json worth showing. Every other member, such as credentials a
/// backup may carry, is never read into memory as a value.
#[derive(Deserialize, Default)]
struct Metadata {
    tenant: Option<String>,
    timestamp: Option<String>,
    version: Option<String>,
    includes: Option<Vec<String>>,
    config: Option<Config>,
}

#[derive(Deserialize, Default)]
struct Config {
    name: Option<String>,
    slug: Option<String>,
    #[serde(rename = "dbName")]
    db_name: Option<String>,
    #[serde(rename = "appVersion")]
    app_version: Option<String>,
}

const ARCHIVE_EXTS: &[&str] = &[".tar.gz", ".tgz"];
/// metadata.json and database.sql are the first members of an archive.
const ARCHIVE_PROBE_ENTRIES: usize = 4;

#[derive(Default)]
struct Slot {
    copies: Vec<Copy>,
    meta: Option<Metadata>,
    /// The `{slug}/` folder of an S3 key.
    tenant: Option<String>,
    modified: Option<u64>,
}

impl Slot {
    fn add(&mut self, copy: Copy, meta: Option<Metadata>, modified: Option<u64>) {
        self.copies.push(copy);
        if self.meta.is_none() {
            self.meta = meta;
        }
        self.modified = self.modified.or(modified);
    }
}

/// Probing an S3 archive reads this much of it: metadata.json is its first member.
const REMOTE_PROBE_BYTES: u64 = 256 << 10;
const REMOTE_PROBE_THREADS: usize = 8;

pub fn scan(folders: &[PathBuf], buckets: &[s3::Config]) -> Scan {
    // Copies are grouped by name: "x", "x.tar.gz" and s3://…/x.tar.gz are one backup.
    let mut found: BTreeMap<String, Slot> = BTreeMap::new();
    for folder in folders {
        let Ok(rd) = fs::read_dir(folder) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(ft) = e.file_type() else { continue };
            let probe = if ft.is_dir() {
                probe_folder(&path).map(|m| (name.clone(), Kind::Folder, m))
            } else if let Some(stem) = archive_stem(&name) {
                fs::File::open(&path).ok().and_then(probe_archive).map(|m| (stem.to_string(), Kind::Archive, m))
            } else if name.ends_with(".dump") && is_custom_dump(&path) {
                Some((name.trim_end_matches(".dump").to_string(), Kind::Dump, None))
            } else {
                None
            };
            let Some((stem, kind, meta)) = probe else { continue };
            let size = if kind == Kind::Folder { folder_size(&path) } else { e.metadata().map_or(0, |m| m.len()) };
            let copy = Copy { path: path.to_string_lossy().into_owned(), kind, size, remote: false };
            found.entry(stem).or_default().add(copy, meta, modified(&path));
        }
    }

    let mut errors = Vec::new();
    for cfg in buckets {
        match scan_bucket(cfg) {
            Ok(items) => {
                for (stem, tenant, copy, meta, modified) in items {
                    let slot = found.entry(stem).or_default();
                    slot.tenant.get_or_insert(tenant);
                    slot.add(copy, meta, modified);
                }
            }
            Err(e) => errors.push(format!("{}: {e}", cfg.path_of(&cfg.prefix()))),
        }
    }

    let mut backups: Vec<Backup> = found
        .into_iter()
        .map(|(stem, slot)| {
            let mut copies = slot.copies;
            copies.sort_by_key(|a| (a.remote, a.kind));
            let legacy = slot.meta.is_none();
            let meta = slot.meta.unwrap_or_default();
            let config = meta.config.unwrap_or_default();
            let from_name = stamp::split_name(&stem);
            let tenant = meta
                .tenant
                .or(config.slug)
                .or(slot.tenant)
                .or_else(|| from_name.map(|(t, _)| t.to_string()))
                .unwrap_or_else(|| stem.clone());
            let taken_at = meta
                .timestamp
                .as_deref()
                .and_then(stamp::parse)
                .or_else(|| from_name.and_then(|(_, s)| stamp::parse(s)))
                .or(slot.modified);
            Backup {
                id: copies[0].path.clone(),
                tenant,
                company: config.name,
                taken_at,
                app_version: config.app_version.or(meta.version),
                includes: meta.includes.unwrap_or_default(),
                db_name: config.db_name,
                legacy,
                copies,
            }
        })
        .collect();
    backups.sort_by(|a, b| a.tenant.cmp(&b.tenant).then(b.taken_at.cmp(&a.taken_at)));
    Scan { backups, errors }
}

fn archive_stem(name: &str) -> Option<&str> {
    ARCHIVE_EXTS.iter().find_map(|x| name.strip_suffix(x))
}

type Found = (String, String, Copy, Option<Metadata>, Option<u64>);

/// Backups under `{prefix}{tenant}/`. Folders starting with "_" are not tenants.
fn scan_bucket(cfg: &s3::Config) -> Result<Vec<Found>, String> {
    let prefix = cfg.prefix();
    let mut items: Vec<(String, String, Kind, s3::Object)> = Vec::new();
    for o in s3::list(cfg)? {
        let Some(rel) = o.key.strip_prefix(&prefix) else { continue };
        let Some((tenant, name)) = rel.split_once('/') else { continue };
        if tenant.starts_with('_') || name.contains('/') {
            continue;
        }
        let (stem, kind) = if let Some(stem) = archive_stem(name) {
            (stem, Kind::Archive)
        } else if let Some(stem) = name.strip_suffix(".dump") {
            (stem, Kind::Dump)
        } else {
            continue;
        };
        items.push((stem.to_string(), tenant.to_string(), kind, o));
    }

    // metadata.json of each archive, a few requests at a time.
    let next = AtomicUsize::new(0);
    let metas: Vec<Mutex<Option<Metadata>>> = items.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..REMOTE_PROBE_THREADS.min(items.len()) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some((_, _, kind, o)) = items.get(i) else { break };
                if *kind == Kind::Archive {
                    let probed = s3::get(cfg, &o.key, Some(REMOTE_PROBE_BYTES)).ok().and_then(|(r, _)| probe_archive(r));
                    *metas[i].lock().unwrap() = probed.flatten();
                }
            });
        }
    });

    Ok(items
        .into_iter()
        .zip(metas)
        .map(|((stem, tenant, kind, o), meta)| {
            let copy = Copy { path: cfg.path_of(&o.key), kind, size: o.size, remote: true };
            let modified = o.last_modified.as_deref().and_then(stamp::parse);
            (stem, tenant, copy, meta.into_inner().unwrap(), modified)
        })
        .collect())
}

fn parse_metadata(bytes: &[u8]) -> Metadata {
    serde_json::from_slice(bytes).unwrap_or_default()
}

/// `Some(None)` is a backup folder without metadata.json.
fn probe_folder(dir: &Path) -> Option<Option<Metadata>> {
    let meta = fs::read(dir.join("metadata.json")).ok().map(|b| parse_metadata(&b));
    (meta.is_some() || dir.join("database.sql").is_file()).then_some(meta)
}

fn probe_archive(r: impl Read) -> Option<Option<Metadata>> {
    let mut archive = tar::Archive::new(GzDecoder::new(r));
    let mut has_sql = false;
    for entry in archive.entries().ok()?.take(ARCHIVE_PROBE_ENTRIES) {
        let mut entry = entry.ok()?;
        let name = entry.path().ok()?.to_string_lossy().trim_start_matches("./").to_string();
        if name == "metadata.json" {
            let mut buf = Vec::new();
            entry.by_ref().take(1 << 20).read_to_end(&mut buf).ok()?;
            return Some(Some(parse_metadata(&buf)));
        }
        has_sql |= name == "database.sql";
    }
    has_sql.then_some(None)
}

/// pg_dump's custom format starts with "PGDMP".
fn is_custom_dump(path: &Path) -> bool {
    let mut magic = [0u8; 5];
    fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && &magic == b"PGDMP"
}

fn folder_size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(rd) = fs::read_dir(&p) else { continue };
        for e in rd.flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                Ok(t) if t.is_file() => total += e.metadata().map_or(0, |m| m.len()),
                _ => {}
            }
        }
    }
    total
}

fn modified(path: &Path) -> Option<u64> {
    let t = fs::symlink_metadata(path).ok()?.modified().ok()?;
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}
