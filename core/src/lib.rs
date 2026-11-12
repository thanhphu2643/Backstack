//! Tenant backup shelf: finds tenant backups (folders, .tar.gz archives, pg_dump files) on
//! disk and in S3, shows what is inside, and restores them into PostgreSQL, in Docker or on
//! any server.
//! The Swift app calls the C functions below, which take and return JSON strings.

pub mod inspect;
pub mod pg;
pub mod s3;
pub mod scan;
pub mod source;
pub mod stamp;

use serde::{Deserialize, Serialize};
use std::ffi::{c_char, CStr, CString};
use std::path::PathBuf;

#[derive(Deserialize)]
struct ScanRequest {
    folders: Vec<PathBuf>,
    #[serde(default)]
    buckets: Vec<s3::Config>,
}

#[derive(Deserialize)]
struct InspectRequest {
    source: source::Source,
    kind: scan::Kind,
}

#[derive(Deserialize)]
struct DropRequest {
    target: pg::Target,
    database: String,
}

#[derive(Serialize)]
struct Listed {
    objects: usize,
}

#[derive(Deserialize)]
struct JobRequest {
    job: u64,
}

#[derive(Serialize)]
struct Started {
    job: u64,
}

#[derive(Serialize)]
struct Done {
    ok: bool,
}

fn json_call<I, O>(input: *const c_char, f: impl FnOnce(I) -> Result<O, String>) -> *mut c_char
where
    I: for<'de> Deserialize<'de>,
    O: Serialize,
{
    let out = (|| -> Result<String, String> {
        if input.is_null() {
            return Err("null input".into());
        }
        let text = unsafe { CStr::from_ptr(input) }.to_str().map_err(|e| e.to_string())?;
        let parsed: I = serde_json::from_str(text).map_err(|e| e.to_string())?;
        serde_json::to_string(&f(parsed)?).map_err(|e| e.to_string())
    })()
    .unwrap_or_else(|e| serde_json::json!({ "error": e }).to_string());
    CString::new(out).map_or(std::ptr::null_mut(), CString::into_raw)
}

/// {folders, buckets?: [S3 config]} → {backups, errors}
#[no_mangle]
pub extern "C" fn backstack_scan(json: *const c_char) -> *mut c_char {
    json_call(json, |r: ScanRequest| Ok(scan::scan(&r.folders, &r.buckets)))
}

/// S3 config → {objects}: checks the settings by listing the bucket.
#[no_mangle]
pub extern "C" fn backstack_list_bucket(json: *const c_char) -> *mut c_char {
    json_call(json, |c: s3::Config| s3::list(&c).map(|o| Listed { objects: o.len() }))
}

/// {source: {path, s3?}, kind} → Details
#[no_mangle]
pub extern "C" fn backstack_inspect(json: *const c_char) -> *mut c_char {
    json_call(json, |r: InspectRequest| inspect::inspect(&r.source, r.kind))
}

/// {} → [{name, image, published}]: running PostgreSQL containers.
#[no_mangle]
pub extern "C" fn backstack_containers(json: *const c_char) -> *mut c_char {
    json_call(json, |_: serde_json::Value| pg::postgres_containers())
}

/// Target → [Database]
#[no_mangle]
pub extern "C" fn backstack_databases(json: *const c_char) -> *mut c_char {
    json_call(json, |t: pg::Target| pg::databases(&t))
}

/// {target, database} → {ok}
#[no_mangle]
pub extern "C" fn backstack_drop(json: *const c_char) -> *mut c_char {
    json_call(json, |r: DropRequest| pg::drop(&r.target, &r.database).map(|()| Done { ok: true }))
}

/// RestoreRequest → {job}
#[no_mangle]
pub extern "C" fn backstack_restore(json: *const c_char) -> *mut c_char {
    json_call(json, |r: pg::RestoreRequest| pg::start_restore(r).map(|job| Started { job }))
}

/// {job} → JobStatus
#[no_mangle]
pub extern "C" fn backstack_job(json: *const c_char) -> *mut c_char {
    json_call(json, |r: JobRequest| pg::status(r.job).ok_or_else(|| "no such job".to_string()))
}

/// {job} → {ok}
#[no_mangle]
pub extern "C" fn backstack_cancel(json: *const c_char) -> *mut c_char {
    json_call(json, |r: JobRequest| {
        pg::cancel(r.job);
        Ok(Done { ok: true })
    })
}

/// # Safety
/// `s` must come from one of the functions above and be freed once.
#[no_mangle]
pub unsafe extern "C" fn backstack_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

#[cfg(test)]
mod tests {
    use super::scan::{scan, Kind};
    use super::*;
    use flate2::write::GzEncoder;
    use std::fs;
    use std::path::Path;

    const SQL: &str = "--\n-- PostgreSQL database dump\n--\n\n-- Dumped from database version 16.15\n\
        CREATE TABLE public.products (id integer, name text);\n\
        CREATE TABLE public.tags (id integer);\n\
        COPY public.products (id, name) FROM stdin;\n1\tShirt\n2\tHat\n3\tSock\n\\.\n\n\
        COPY public.tags (id) FROM stdin;\n7\n\\.\n";

    fn metadata(tenant: &str, ts: &str) -> String {
        format!(
            r#"{{"schemaVersion":2,"tenant":"{tenant}","timestamp":"{ts}","version":"latest","includes":["database","files"],
            "config":{{"name":"Shop {tenant}","slug":"{tenant}","dbName":"{tenant}_db","appVersion":"1.6.0"}},
            "secrets":{{"apiKey":"TOPSECRET"}}}}"#
        )
    }

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("backstack-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Temp(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_folder(dir: &Path, meta: &str) {
        fs::create_dir_all(dir.join("files/products")).unwrap();
        fs::write(dir.join("metadata.json"), meta).unwrap();
        fs::write(dir.join("database.sql"), SQL).unwrap();
        fs::write(dir.join("files/products/a.jpg"), [1u8; 300]).unwrap();
    }

    fn write_archive(path: &Path, meta: &str) {
        let gz = GzEncoder::new(fs::File::create(path).unwrap(), flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        for (name, data) in [("./metadata.json", meta.as_bytes()), ("./database.sql", SQL.as_bytes()), ("./files/products/a.jpg", &[1u8; 300])] {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, name, data).unwrap();
        }
        b.into_inner().unwrap().finish().unwrap();
    }

    fn docker_target() -> pg::Target {
        let container = std::env::var("BACKSTACK_TEST_CONTAINER").unwrap_or_else(|_| "postgres".into());
        pg::Target::Docker { container, user: "postgres".into() }
    }

    fn wait(job: u64) -> pg::JobStatus {
        loop {
            let s = pg::status(job).unwrap();
            if s.finished {
                return s;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    fn restore(target: &pg::Target, source: source::Source, kind: Kind, database: &str, replace: bool) -> Result<pg::JobStatus, String> {
        pg::start_restore(pg::RestoreRequest { target: target.clone(), source, kind, database: database.into(), replace }).map(wait)
    }

    #[test]
    fn scan_groups_folder_and_archive_and_hides_secrets() {
        let t = Temp::new("scan");
        let meta = metadata("shop", "2026-11-09T02:09:19.179Z");
        write_folder(&t.0.join("shop_2026-11-09T02-08-46-592Z"), &meta);
        write_archive(&t.0.join("shop_2026-11-09T02-08-46-592Z.tar.gz"), &meta);
        write_archive(&t.0.join("shop_2026-10-01T02-00-00-000Z.tar.gz"), &metadata("shop", "2026-10-01T02:00:00Z"));
        let mut dump = b"PGDMP".to_vec();
        dump.extend([0u8; 100]);
        fs::write(t.0.join("Pro_2026-03-17_03-46-03.dump"), dump).unwrap();
        fs::write(t.0.join("notes.txt"), "x").unwrap();
        fs::create_dir(t.0.join("photos")).unwrap();

        let found = scan(std::slice::from_ref(&t.0), &[]).backups;
        let json = serde_json::to_string(&found).unwrap();
        assert!(!json.contains("TOPSECRET") && !json.contains("secrets"));

        assert_eq!(found.len(), 3, "{found:#?}");
        let (pro, newest, older) = (&found[0], &found[1], &found[2]);
        assert_eq!((pro.tenant.as_str(), pro.legacy, pro.copies[0].kind), ("Pro", true, Kind::Dump));
        assert_eq!(pro.taken_at, stamp::parse("2026-03-17_03-46-03"));
        assert_eq!(newest.tenant, "shop");
        assert_eq!(newest.copies.iter().map(|c| c.kind).collect::<Vec<_>>(), [Kind::Folder, Kind::Archive]);
        assert_eq!(newest.company.as_deref(), Some("Shop shop"));
        assert_eq!(newest.db_name.as_deref(), Some("shop_db"));
        assert_eq!(newest.taken_at, stamp::parse("2026-11-09T02:09:19Z"));
        assert!(older.taken_at < newest.taken_at);
    }

    #[test]
    fn inspect_counts_rows_and_files() {
        let t = Temp::new("inspect");
        let meta = metadata("shop", "2026-11-09T02:09:19Z");
        write_folder(&t.0.join("f"), &meta);
        write_archive(&t.0.join("a.tar.gz"), &meta);
        for (path, kind) in [(t.0.join("f"), Kind::Folder), (t.0.join("a.tar.gz"), Kind::Archive)] {
            let d = inspect::inspect(&source::Source::local(&path), kind).unwrap();
            assert_eq!(d.pg_version.as_deref(), Some("16.15"));
            assert_eq!(d.total_rows, 4);
            assert_eq!((d.tables[0].name.as_str(), d.tables[0].rows), ("products", 3));
            assert_eq!((d.files, d.files_bytes), (1, 300));
            assert_eq!(d.sql_bytes, SQL.len() as u64);
        }
    }

    #[test]
    fn db_names() {
        for ok in ["shop_20261109", "_x", "a"] {
            assert!(pg::valid_db_name(ok), "{ok}");
        }
        for bad in ["", "Shop", "1abc", "a-b", "a;drop", "x\"y", &"a".repeat(64)] {
            assert!(!pg::valid_db_name(bad), "{bad}");
        }
    }

    #[test]
    fn targets_parse_from_json() {
        let d: pg::Target = serde_json::from_str(r#"{"via":"docker","container":"pg","user":"postgres"}"#).unwrap();
        assert!(matches!(d, pg::Target::Docker { .. }));
        let s: pg::Target = serde_json::from_str(r#"{"via":"server","host":"db.example.com","port":5432,"user":"admin"}"#).unwrap();
        assert!(matches!(s, pg::Target::Server { allow_drop: false, .. }));
    }

    #[test]
    fn a_server_refuses_drops_unless_allowed() {
        let t = pg::Target::Server {
            host: "db.invalid".into(),
            port: 5432,
            user: "admin".into(),
            password: String::new(),
            sslmode: String::new(),
            allow_drop: false,
        };
        // Refused before anything connects.
        assert!(pg::drop(&t, "shop").unwrap_err().contains("does not allow dropping"));
    }

    /// Needs Docker with a running PostgreSQL container:
    /// BACKSTACK_TEST_CONTAINER=postgres cargo test -- --ignored
    #[test]
    #[ignore]
    fn restores_archive_into_docker_postgres() {
        let target = docker_target();
        let t = Temp::new("restore");
        let archive = t.0.join("shop.tar.gz");
        write_archive(&archive, &metadata("shop", "2026-11-09T02:09:19Z"));
        let db = format!("backstack_test_{}", std::process::id());

        for replace in [false, true] {
            let status = restore(&target, source::Source::local(&archive), Kind::Archive, &db, replace).unwrap();
            assert_eq!(status.error, None);
            assert_eq!(status.bytes, SQL.len() as u64);
        }
        assert!(restore(&target, source::Source::local(&archive), Kind::Archive, &db, false).unwrap_err().contains("already exists"));
        let listed = pg::databases(&target).unwrap();
        assert!(listed.iter().any(|d| d.name == db));
        pg::drop(&target, &db).unwrap();
        assert!(!pg::exists(&target, &db).unwrap());

        // A dump that fails halfway leaves no database behind.
        let broken = t.0.join("broken");
        fs::create_dir(&broken).unwrap();
        fs::write(broken.join("database.sql"), "CREATE TABLE ok (id int);\nTHIS IS NOT SQL;\n").unwrap();
        let status = restore(&target, source::Source::local(&broken), Kind::Folder, &db, false).unwrap();
        assert!(status.error.as_deref().unwrap().contains("syntax error"), "{status:?}");
        assert!(!pg::exists(&target, &db).unwrap());
    }

    /// A PostgreSQL server reached with this Mac's psql:
    /// BACKSTACK_TEST_PG=localhost:5432 BACKSTACK_TEST_PG_PASSWORD=… cargo test -- --ignored
    #[test]
    #[ignore]
    fn restores_into_a_server_with_local_psql() {
        let addr = std::env::var("BACKSTACK_TEST_PG").unwrap_or_else(|_| "localhost:5432".into());
        let (host, port) = addr.split_once(':').unwrap();
        let server = |allow_drop| pg::Target::Server {
            host: host.into(),
            port: port.parse().unwrap(),
            user: "postgres".into(),
            password: std::env::var("BACKSTACK_TEST_PG_PASSWORD").unwrap_or_default(),
            sslmode: "disable".into(),
            allow_drop,
        };
        let t = Temp::new("server");
        write_folder(&t.0.join("f"), &metadata("shop", "2026-11-09T02:09:19Z"));
        let db = format!("backstack_server_{}", std::process::id());

        let status = restore(&server(false), source::Source::local(t.0.join("f")), Kind::Folder, &db, false).unwrap();
        assert_eq!(status.error, None);
        assert!(pg::databases(&server(false)).unwrap().iter().any(|d| d.name == db && d.size > 0));
        // Replacing needs the server's permission.
        assert!(restore(&server(false), source::Source::local(t.0.join("f")), Kind::Folder, &db, true).unwrap_err().contains("does not allow dropping"));
        assert!(pg::drop(&server(false), &db).is_err());
        pg::drop(&server(true), &db).unwrap();
        assert!(!pg::exists(&server(true), &db).unwrap());
    }

    /// An S3-compatible server where a scratch bucket can be made, like a local MinIO:
    /// BACKSTACK_TEST_S3=localhost:9000 BACKSTACK_TEST_S3_KEY=… BACKSTACK_TEST_S3_SECRET=… cargo test -- --ignored
    /// Restores into the Docker container too.
    #[test]
    #[ignore]
    fn reads_and_restores_backups_from_s3() {
        let addr = std::env::var("BACKSTACK_TEST_S3").unwrap_or_else(|_| "localhost:9000".into());
        let (host, port) = addr.split_once(':').unwrap();
        let cfg = s3::Config {
            endpoint: host.into(),
            port: port.parse().ok(),
            use_ssl: false,
            region: String::new(),
            bucket: format!("backstack-test-{}", std::process::id()),
            prefix: "dev".into(),
            access_key: std::env::var("BACKSTACK_TEST_S3_KEY").unwrap(),
            secret_key: std::env::var("BACKSTACK_TEST_S3_SECRET").unwrap(),
        };
        let t = Temp::new("s3");
        let name = "shop_2026-11-09T02-08-46-592Z.tar.gz";
        write_archive(&t.0.join(name), &metadata("shop", "2026-11-09T02:09:19Z"));
        let bytes = fs::read(t.0.join(name)).unwrap();

        s3::testing::create_bucket(&cfg);
        let result = std::panic::catch_unwind(|| {
            s3::testing::put(&cfg, &format!("dev/shop/{name}"), &bytes);
            let mut older = Vec::new();
            write_archive(&t.0.join("older.tar.gz"), &metadata("shop", "2026-10-01T02:00:00Z"));
            older.extend(fs::read(t.0.join("older.tar.gz")).unwrap());
            fs::remove_file(t.0.join("older.tar.gz")).unwrap();
            s3::testing::put(&cfg, "dev/shop/shop_2026-10-01T02-00-00-000Z.tar.gz", &older);
            s3::testing::put(&cfg, "dev/_internal/shop/products.tar.gz", &bytes);
            s3::testing::put(&cfg, "dev/shop/readme.txt", b"x");
            s3::testing::put(&cfg, "other/shop/x.tar.gz", &bytes);

            // The local archive and its S3 copy are one backup, local first.
            let found = scan(std::slice::from_ref(&t.0), std::slice::from_ref(&cfg));
            assert!(found.errors.is_empty(), "{:?}", found.errors);
            assert_eq!(found.backups.len(), 2, "{:#?}", found.backups);
            let b = &found.backups[0];
            assert_eq!((b.tenant.as_str(), b.company.as_deref()), ("shop", Some("Shop shop")));
            assert_eq!(b.copies.iter().map(|c| c.remote).collect::<Vec<_>>(), [false, true]);
            let remote = source::Source { path: b.copies[1].path.clone(), s3: Some(cfg.clone()) };
            assert_eq!(remote.path, format!("s3://{}/dev/shop/{name}", cfg.bucket));

            let d = inspect::inspect(&remote, Kind::Archive).unwrap();
            assert_eq!((d.total_rows, d.files), (4, 1));

            let db = format!("backstack_s3_{}", std::process::id());
            let status = restore(&docker_target(), remote, Kind::Archive, &db, false).unwrap();
            assert_eq!((status.error, status.bytes), (None, SQL.len() as u64));
            pg::drop(&docker_target(), &db).unwrap();

            let wrong = s3::Config { secret_key: "wrong".into(), ..cfg.clone() };
            let err = scan(&[], &[wrong]).errors;
            assert!(err[0].contains("SignatureDoesNotMatch"), "{err:?}");
        });
        s3::testing::delete_all(&cfg);
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    #[test]
    fn invalid_name_never_reaches_docker() {
        let err = pg::start_restore(pg::RestoreRequest {
            target: pg::Target::Docker { container: "x".into(), user: "postgres".into() },
            source: source::Source::local("/nonexistent"),
            kind: Kind::Folder,
            database: "Bad-Name".into(),
            replace: false,
        })
        .unwrap_err();
        assert!(err.contains("invalid"));
    }
}
