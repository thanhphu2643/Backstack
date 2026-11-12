//! Restores one backup copy into a throwaway database, prints the outcome, then drops it:
//! `cargo run --example restore_check -- <path> <folder|archive|dump> <container>`
use backstack_core::{pg, scan::Kind, source::Source};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let kind = match a[1].as_str() { "folder" => Kind::Folder, "archive" => Kind::Archive, _ => Kind::Dump };
    let target = pg::Target::Docker { container: a[2].clone(), user: "postgres".into() };
    let db = format!("backstack_check_{}", std::process::id());
    let t = std::time::Instant::now();
    let job = pg::start_restore(pg::RestoreRequest { target: target.clone(), source: Source::local(&a[0]), kind, database: db.clone(), replace: false }).unwrap();
    let s = loop {
        let s = pg::status(job).unwrap();
        if s.finished { break s; }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    let size = pg::databases(&target).unwrap().into_iter().find(|d| d.name == db).map(|d| d.size);
    println!("error={:?} bytes={}/{} db_size={:?} in {:?}", s.error, s.bytes, s.total, size, t.elapsed());
    let _ = pg::drop(&target, &db);
}
