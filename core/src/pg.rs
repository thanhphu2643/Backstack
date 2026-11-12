//! Talks to PostgreSQL, either inside a Docker container (`docker exec … psql`) or on any
//! server through the psql and pg_restore installed here, and restores backups into it as
//! background jobs.

use crate::scan::Kind;
use crate::source::Source;
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

#[derive(Deserialize, Clone)]
#[serde(tag = "via", rename_all = "lowercase")]
pub enum Target {
    Docker {
        container: String,
        user: String,
    },
    Server {
        host: String,
        port: u16,
        user: String,
        #[serde(default)]
        password: String,
        /// PGSSLMODE: disable, prefer, require, verify-full…
        #[serde(default)]
        sslmode: String,
        /// Dropping or replacing databases is refused unless allowed, so pointing the app
        /// at staging or production is safe by default.
        #[serde(default)]
        allow_drop: bool,
    },
}

impl Target {
    /// `name` (psql, pg_restore) with the connection options, ready for more arguments.
    fn tool(&self, name: &str, interactive: bool) -> Result<Command, String> {
        let bad = |s: &str| s.is_empty() || s.starts_with('-');
        match self {
            Target::Docker { container, user } => {
                if bad(container) || bad(user) {
                    return Err("invalid container or user".into());
                }
                let mut c = Command::new(docker()?);
                c.arg("exec");
                if interactive {
                    c.arg("-i");
                }
                c.args([container.as_str(), name, "-U", user]);
                Ok(c)
            }
            Target::Server { host, port, user, password, sslmode, .. } => {
                if bad(host) || bad(user) {
                    return Err("invalid host or user".into());
                }
                let mut c = Command::new(pg_bin(name)?);
                // -w: fail instead of waiting for a password nobody can type.
                c.args(["-h", host, "-p", &port.to_string(), "-U", user, "-w"]);
                c.env("PGCONNECT_TIMEOUT", "10");
                if !password.is_empty() {
                    c.env("PGPASSWORD", password);
                }
                if !sslmode.is_empty() {
                    c.env("PGSSLMODE", sslmode);
                }
                Ok(c)
            }
        }
    }

    fn may_drop(&self) -> Result<(), String> {
        match self {
            Target::Server { allow_drop: false, host, .. } => {
                Err(format!("server {host} does not allow dropping or overwriting databases (allow it in its settings)"))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Serialize, Debug)]
pub struct Database {
    pub name: String,
    pub size: u64,
}

/// Apps opened from Finder get a bare PATH, so look in the usual install places too.
fn docker() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut candidates: Vec<PathBuf> = vec![
        "/usr/local/bin/docker".into(),
        "/opt/homebrew/bin/docker".into(),
        format!("{home}/.docker/bin/docker").into(),
        format!("{home}/.orbstack/bin/docker").into(),
        "/Applications/Docker.app/Contents/Resources/bin/docker".into(),
    ];
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|d| d.join("docker")));
    }
    candidates.into_iter().find(|p| p.is_file()).ok_or_else(|| "docker command not found".into())
}

#[derive(Serialize, Debug)]
pub struct Container {
    pub name: String,
    pub image: String,
    /// Publishes a port to the host, which the database a project actually uses usually does.
    pub published: bool,
}

/// Running containers whose image looks like PostgreSQL, published ones first.
pub fn postgres_containers() -> Result<Vec<Container>, String> {
    let out = Command::new(docker()?)
        .args(["ps", "--format", "{{.Names}}\t{{.Image}}\t{{.Ports}}"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(parse_containers(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_containers(ps: &str) -> Vec<Container> {
    let mut found: Vec<Container> = ps
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let (name, image, ports) = (f.next()?, f.next()?, f.next().unwrap_or(""));
            let img = image.to_ascii_lowercase();
            ["postgres", "postgis", "timescale", "pgvector"].iter().any(|k| img.contains(k)).then(|| Container {
                name: name.to_string(),
                image: image.to_string(),
                published: ports.contains("->"),
            })
        })
        .collect();
    found.sort_by(|a, b| b.published.cmp(&a.published).then(a.name.cmp(&b.name)));
    found
}

/// psql or pg_restore from Homebrew, Postgres.app or PATH.
fn pg_bin(name: &str) -> Result<PathBuf, String> {
    let mut dirs: Vec<PathBuf> = vec![
        "/opt/homebrew/opt/libpq/bin".into(),
        "/opt/homebrew/bin".into(),
        "/usr/local/opt/libpq/bin".into(),
        "/usr/local/bin".into(),
        "/Applications/Postgres.app/Contents/Versions/latest/bin".into(),
    ];
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.into_iter()
        .map(|d| d.join(name))
        .find(|p| p.is_file())
        .ok_or_else(|| format!("{name} not found: install it with `brew install libpq` or Postgres.app"))
}

/// Lowercase identifier PostgreSQL accepts without quoting.
pub fn valid_db_name(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=63).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0] == b'_')
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

fn check_name(s: &str) -> Result<(), String> {
    if valid_db_name(s) { Ok(()) } else { Err(format!("invalid database name: {s}")) }
}

/// Runs one SQL command and returns psql's unaligned output.
fn psql(t: &Target, db: &str, sql: &str) -> Result<String, String> {
    let out = t
        .tool("psql", false)?
        .args(["-d", db, "-v", "ON_ERROR_STOP=1", "-Atq", "-c", sql])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if let (Target::Docker { container, .. }, true) = (t, err.contains("No such container")) {
        return Err(format!("no running container named {container}: pick another one in the Postgres settings"));
    }
    Err(err)
}

pub fn databases(t: &Target) -> Result<Vec<Database>, String> {
    let out = psql(
        t,
        "postgres",
        // Sizes of databases the user may not connect to would fail the whole query.
        "select datname, case when has_database_privilege(datname, 'CONNECT') then pg_database_size(datname) else 0 end \
         from pg_database where not datistemplate and datname <> 'postgres' order by 1",
    )?;
    Ok(out
        .lines()
        .filter_map(|l| l.split_once('|'))
        .map(|(name, size)| Database { name: name.to_string(), size: size.trim().parse().unwrap_or(0) })
        .collect())
}

pub fn exists(t: &Target, name: &str) -> Result<bool, String> {
    check_name(name)?;
    Ok(psql(t, "postgres", &format!("select 1 from pg_database where datname = '{name}'"))?.trim() == "1")
}

pub fn drop(t: &Target, name: &str) -> Result<(), String> {
    t.may_drop()?;
    drop_db(t, name)
}

/// Without the server's permission check: for a database this app just created.
fn drop_db(t: &Target, name: &str) -> Result<(), String> {
    check_name(name)?;
    if name == "postgres" || name.starts_with("template") {
        return Err("system databases cannot be dropped".into());
    }
    psql(t, "postgres", &format!("drop database if exists \"{name}\" with (force)")).map(|_| ())
}

// ---------- restore jobs ----------

#[derive(Deserialize)]
pub struct RestoreRequest {
    pub target: Target,
    pub source: Source,
    pub kind: Kind,
    pub database: String,
    /// Drop an existing database of that name first.
    #[serde(default)]
    pub replace: bool,
}

/// The error of a restore stopped with `cancel`.
pub const CANCELLED: &str = "cancelled";

#[derive(Serialize, Clone, Debug, Default)]
pub struct JobStatus {
    /// create, connect (to S3), open, load, done or failed.
    pub phase: String,
    pub bytes: u64,
    pub total: u64,
    pub finished: bool,
    pub error: Option<String>,
}

#[derive(Default)]
struct Job {
    status: Mutex<JobStatus>,
    bytes: AtomicU64,
    cancel: AtomicBool,
    child: Mutex<Option<Child>>,
}

fn jobs() -> &'static Mutex<HashMap<u64, Arc<Job>>> {
    static JOBS: OnceLock<Mutex<HashMap<u64, Arc<Job>>>> = OnceLock::new();
    JOBS.get_or_init(Default::default)
}

pub fn status(id: u64) -> Option<JobStatus> {
    let job = jobs().lock().unwrap().get(&id)?.clone();
    let mut s = job.status.lock().unwrap().clone();
    s.bytes = job.bytes.load(Ordering::Relaxed);
    Some(s)
}

pub fn cancel(id: u64) {
    let job = jobs().lock().unwrap().get(&id).cloned();
    if let Some(job) = job {
        job.cancel.store(true, Ordering::Relaxed);
        if let Some(child) = job.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }
}

/// Checks the request, then restores in the background. Returns the job id.
pub fn start_restore(req: RestoreRequest) -> Result<u64, String> {
    check_name(&req.database)?;
    if exists(&req.target, &req.database)? {
        if !req.replace {
            return Err(format!("database {} already exists", req.database));
        }
        req.target.may_drop()?;
    }
    let job = Arc::new(Job::default());
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    jobs().lock().unwrap().insert(id, job.clone());
    std::thread::spawn(move || {
        let result = run_restore(&req, &job);
        let cancelled = job.cancel.load(Ordering::Relaxed);
        if result.is_err() || cancelled {
            // Never leave a half-restored database behind.
            let _ = drop_db(&req.target, &req.database);
        }
        let mut s = job.status.lock().unwrap();
        s.finished = true;
        s.error = match result {
            _ if cancelled => Some(CANCELLED.into()),
            Err(e) => Some(e),
            Ok(()) => None,
        };
        s.phase = if s.error.is_none() { "done".into() } else { "failed".into() };
    });
    Ok(id)
}

fn set_phase(job: &Job, phase: &str, total: u64) {
    let mut s = job.status.lock().unwrap();
    s.phase = phase.into();
    s.total = total;
}

fn run_restore(req: &RestoreRequest, job: &Job) -> Result<(), String> {
    let t = &req.target;
    set_phase(job, "create", 0);
    if req.replace {
        drop(t, &req.database)?;
    }
    psql(t, "postgres", &format!("create database \"{}\"", req.database))?;

    set_phase(job, if req.source.is_remote() { "connect" } else { "open" }, 0);
    let (reader, total) = open_sql(&req.source, req.kind)?;
    set_phase(job, "load", total);
    let mut cmd = t.tool(if req.kind == Kind::Dump { "pg_restore" } else { "psql" }, true)?;
    match req.kind {
        Kind::Dump => cmd.args(["-d", &req.database, "--no-owner", "--no-privileges", "--exit-on-error"]),
        _ => cmd.args(["-d", &req.database, "-v", "ON_ERROR_STOP=1", "-q", "-o", "/dev/null"]),
    };
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    *job.child.lock().unwrap() = Some(child);

    let mut counting = Counting { inner: reader, job };
    let copied = io::copy(&mut counting, &mut stdin);
    std::mem::drop(stdin); // EOF tells psql the dump is complete
    if job.cancel.load(Ordering::Relaxed) {
        cancel_child(job);
    }
    let status = loop {
        let done = job.child.lock().unwrap().as_mut().map(|c| c.try_wait());
        match done {
            Some(Ok(Some(st))) => break Some(st),
            Some(Ok(None)) => std::thread::sleep(Duration::from_millis(200)),
            _ => break None,
        }
    };
    let stderr = errors.join().unwrap_or_default();
    let last_error = || {
        let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
        lines[lines.len().saturating_sub(3)..].join("\n")
    };
    if let Err(e) = copied {
        let tail = last_error();
        return Err(if tail.is_empty() { e.to_string() } else { tail });
    }
    match status {
        Some(st) if st.success() => Ok(()),
        _ => Err(Some(last_error()).filter(|s| !s.is_empty()).unwrap_or_else(|| "psql stopped unexpectedly".into())),
    }
}

fn cancel_child(job: &Job) {
    if let Some(c) = job.child.lock().unwrap().as_mut() {
        let _ = c.kill();
    }
}

/// The SQL (or custom-format dump) of a backup copy, streamed, with its size.
fn open_sql(source: &Source, kind: Kind) -> Result<(Box<dyn Read + Send>, u64), String> {
    match kind {
        Kind::Dump => {
            let (r, len) = source.open()?;
            Ok((r, len.unwrap_or(0)))
        }
        Kind::Folder => {
            let (r, len) = Source::local(source.dir()?.join("database.sql")).open()?;
            Ok((r, len.unwrap_or(0)))
        }
        Kind::Archive => {
            // Read through the archive up to database.sql, then hand out a reader over that
            // member only. From S3 the rest of the archive is never downloaded.
            let (r, _) = source.open()?;
            let member = ArchiveMember::open(r)?;
            let len = member.remaining;
            Ok((Box::new(member), len))
        }
    }
}

/// Streams the `database.sql` member of a tar.gz.
struct ArchiveMember {
    gz: GzDecoder<Box<dyn Read + Send>>,
    remaining: u64,
}

impl ArchiveMember {
    fn open(f: Box<dyn Read + Send>) -> Result<Self, String> {
        let mut gz = GzDecoder::new(f);
        loop {
            let mut header = [0u8; 512];
            gz.read_exact(&mut header).map_err(|_| "no database.sql in the archive".to_string())?;
            if header.iter().all(|&b| b == 0) {
                return Err("no database.sql in the archive".into());
            }
            let h = tar::Header::from_byte_slice(&header);
            let size = h.entry_size().map_err(|e| e.to_string())?;
            let name = h.path().map_err(|e| e.to_string())?.to_string_lossy().trim_start_matches("./").to_string();
            if name == "database.sql" && h.entry_type().is_file() {
                return Ok(ArchiveMember { gz, remaining: size });
            }
            // Skip the member's data, padded to 512-byte blocks.
            let padded = size.div_ceil(512) * 512;
            io::copy(&mut (&mut gz).take(padded), &mut io::sink()).map_err(|e| e.to_string())?;
        }
    }
}

impl Read for ArchiveMember {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        let max = buf.len().min(self.remaining as usize);
        let n = self.gz.read(&mut buf[..max])?;
        self.remaining -= n as u64;
        Ok(n)
    }
}

struct Counting<'a, R> {
    inner: R,
    job: &'a Job,
}

impl<R: Read> Read for Counting<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.job.cancel.load(Ordering::Relaxed) {
            // Not ErrorKind::Interrupted: io::copy would retry it forever.
            return Err(io::Error::other(CANCELLED));
        }
        let n = self.inner.read(buf)?;
        self.job.bytes.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_postgres_containers() {
        let ps = "web\tnginx:1\t0.0.0.0:80->80/tcp\n\
                  app-db-1\tpostgres:16\t5432/tcp\n\
                  main-pg\tpostgres:16-alpine\t0.0.0.0:5433->5432/tcp\n\
                  geo\tpostgis/postgis:16-3.4\t\n";
        let names: Vec<_> = parse_containers(ps).into_iter().map(|c| (c.name, c.published)).collect();
        assert_eq!(names, [("main-pg".into(), true), ("app-db-1".into(), false), ("geo".into(), false)]);
    }
}
