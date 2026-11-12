//! Lists the backups in a bucket, reads the largest, restores it into a throwaway database
//! in Docker and drops it again:
//! `S3_ENDPOINT=localhost:9000 S3_BUCKET=… S3_PREFIX=… S3_KEY=… S3_SECRET=… PG_CONTAINER=… cargo run --release --example s3_check`
use backstack_core::{inspect, pg, s3, scan, source::Source};
use std::time::{Duration, Instant};

fn main() {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let endpoint = env("S3_ENDPOINT");
    let (host, port) = endpoint.split_once(':').map_or((endpoint.as_str(), None), |(h, p)| (h, p.parse().ok()));
    let cfg = s3::Config {
        endpoint: host.into(),
        port,
        use_ssl: env("S3_SSL") == "1",
        region: env("S3_REGION"),
        bucket: env("S3_BUCKET"),
        prefix: env("S3_PREFIX"),
        access_key: env("S3_KEY"),
        secret_key: env("S3_SECRET"),
    };
    let t = Instant::now();
    let found = scan::scan(&[], std::slice::from_ref(&cfg));
    println!("{} backups, errors {:?}, in {:?}", found.backups.len(), found.errors, t.elapsed());
    for b in &found.backups {
        println!("  {:<12} {:<24} {:>10} {}", b.tenant, b.company.as_deref().unwrap_or("-"), b.copies[0].size, b.copies[0].path);
    }
    let Some(b) = found.backups.iter().max_by_key(|b| b.copies[0].size) else { return };
    let src = Source { path: b.copies[0].path.clone(), s3: Some(cfg.clone()) };

    let t = Instant::now();
    let d = inspect::inspect(&src, b.copies[0].kind).unwrap();
    println!("inspect: {} tables, {} rows, {} files in {:?}", d.tables.len(), d.total_rows, d.files, t.elapsed());

    let target = pg::Target::Docker { container: env("PG_CONTAINER"), user: "postgres".into() };
    let db = format!("backstack_check_{}", std::process::id());
    let t = Instant::now();
    let job = pg::start_restore(pg::RestoreRequest { target: target.clone(), source: src, kind: b.copies[0].kind, database: db.clone(), replace: false }).unwrap();
    let s = loop {
        let s = pg::status(job).unwrap();
        if s.finished {
            break s;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let size = pg::databases(&target).unwrap().into_iter().find(|d| d.name == db).map(|d| d.size);
    println!("restore: error={:?} bytes={}/{} db_size={:?} in {:?}", s.error, s.bytes, s.total, size, t.elapsed());
    let _ = pg::drop(&target, &db);
}
