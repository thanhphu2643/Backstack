//! Looks inside one backup copy: the tables in database.sql with their row counts,
//! and how many uploaded files it carries.

use crate::scan::Kind;
use crate::source::Source;
use flate2::read::GzDecoder;
use serde::Serialize;
use std::fs;
use std::io::{BufRead, BufReader, Read};

#[derive(Serialize, Debug, Default)]
pub struct Details {
    /// PostgreSQL version the dump was taken from.
    pub pg_version: Option<String>,
    /// Size of database.sql (or of the .dump file).
    pub sql_bytes: u64,
    /// Largest first.
    pub tables: Vec<Table>,
    pub total_rows: u64,
    pub files: u64,
    pub files_bytes: u64,
}

#[derive(Serialize, Debug)]
pub struct Table {
    pub name: String,
    pub rows: u64,
}

/// Reads the whole copy; for one in S3 that means downloading it.
pub fn inspect(src: &Source, kind: Kind) -> Result<Details, String> {
    let err = |e: std::io::Error| e.to_string();
    let mut d = Details::default();
    match kind {
        Kind::Dump => {
            let (_, len) = src.open()?;
            d.sql_bytes = len.unwrap_or(0);
        }
        Kind::Folder => {
            let path = src.dir()?;
            let sql = path.join("database.sql");
            if sql.is_file() {
                parse_sql(BufReader::with_capacity(1 << 20, fs::File::open(&sql).map_err(err)?), &mut d).map_err(err)?;
            }
            let mut stack = vec![path.join("files")];
            while let Some(p) = stack.pop() {
                let Ok(rd) = fs::read_dir(&p) else { continue };
                for e in rd.flatten() {
                    match e.file_type() {
                        Ok(t) if t.is_dir() => stack.push(e.path()),
                        Ok(t) if t.is_file() => {
                            d.files += 1;
                            d.files_bytes += e.metadata().map_or(0, |m| m.len());
                        }
                        _ => {}
                    }
                }
            }
        }
        Kind::Archive => {
            let (file, _) = src.open()?;
            let mut archive = tar::Archive::new(GzDecoder::new(file));
            for entry in archive.entries().map_err(err)? {
                let entry = entry.map_err(err)?;
                let name = entry.path().map_err(err)?.to_string_lossy().trim_start_matches("./").to_string();
                if name == "database.sql" {
                    parse_sql(BufReader::with_capacity(1 << 20, entry), &mut d).map_err(err)?;
                } else if name.starts_with("files/") && entry.header().entry_type().is_file() {
                    d.files += 1;
                    d.files_bytes += entry.header().size().unwrap_or(0);
                }
            }
        }
    }
    d.tables.sort_by(|a, b| b.rows.cmp(&a.rows).then(a.name.cmp(&b.name)));
    d.total_rows = d.tables.iter().map(|t| t.rows).sum();
    Ok(d)
}

/// Reads a plain pg_dump: each `COPY table (...) FROM stdin;` block holds one row per line
/// until a line with `\.`.
pub fn parse_sql<R: Read>(mut r: BufReader<R>, d: &mut Details) -> std::io::Result<()> {
    let mut line = Vec::with_capacity(4096);
    let mut in_copy = false;
    loop {
        line.clear();
        let n = r.read_until(b'\n', &mut line)?;
        if n == 0 {
            return Ok(());
        }
        d.sql_bytes += n as u64;
        if in_copy {
            if line.starts_with(b"\\.") && line[2..].iter().all(|c| matches!(c, b'\n' | b'\r')) {
                in_copy = false;
            } else if let Some(t) = d.tables.last_mut() {
                t.rows += 1;
            }
        } else if let Some(rest) = line.strip_prefix(b"COPY ") {
            let end = rest.iter().position(|&c| c == b' ' || c == b'(').unwrap_or(rest.len());
            let name = String::from_utf8_lossy(&rest[..end]);
            d.tables.push(Table { name: name.strip_prefix("public.").unwrap_or(&name).to_string(), rows: 0 });
            in_copy = true;
        } else if let Some(v) = line.strip_prefix(b"-- Dumped from database version ") {
            d.pg_version = Some(String::from_utf8_lossy(v).trim().to_string());
        }
    }
}
