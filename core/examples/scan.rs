//! Lists backups in a folder (default ~/Downloads) and inspects the newest one:
//! `cargo run --release --example scan [folder]`
use backstack_core::{inspect, scan, source::Source};

fn main() {
    let folder = std::env::args().nth(1).unwrap_or_else(|| format!("{}/Downloads", std::env::var("HOME").unwrap()));
    let t = std::time::Instant::now();
    let found = scan::scan(&[folder.into()], &[]).backups;
    println!("{} backups in {:?}", found.len(), t.elapsed());
    for b in &found {
        let kinds: Vec<String> = b.copies.iter().map(|c| format!("{:?} {:.0} MB", c.kind, c.size as f64 / 1e6)).collect();
        println!("{:<28} {:>12} legacy={:<5} {}", b.tenant, b.taken_at.unwrap_or(0), b.legacy, kinds.join(", "));
    }
    if let Some(b) = found.iter().max_by_key(|b| b.copies[0].size) {
        let t = std::time::Instant::now();
        let d = inspect::inspect(&Source::local(&b.copies[0].path), b.copies[0].kind).unwrap();
        println!("\n{} ({:?}): pg {:?}, {} tables, {} rows, {} files, in {:?}",
            b.tenant, b.copies[0].kind, d.pg_version, d.tables.len(), d.total_rows, d.files, t.elapsed());
        for tb in d.tables.iter().take(5) {
            println!("  {:<40} {}", tb.name, tb.rows);
        }
    }
}
