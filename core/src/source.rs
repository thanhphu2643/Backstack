//! Where one backup copy lives: a local path, or `s3://bucket/key` with the bucket's settings.

use crate::s3;
use serde::Deserialize;
use std::fs;
use std::io::Read;
use std::path::Path;

#[derive(Deserialize, Clone, Debug)]
pub struct Source {
    pub path: String,
    /// Required for `s3://` paths.
    #[serde(default)]
    pub s3: Option<s3::Config>,
}

impl Source {
    pub fn local(path: impl AsRef<Path>) -> Self {
        Source { path: path.as_ref().to_string_lossy().into_owned(), s3: None }
    }

    pub fn is_remote(&self) -> bool {
        self.path.starts_with("s3://")
    }

    /// A local folder, which only exists on disk.
    pub fn dir(&self) -> Result<&Path, String> {
        if self.is_remote() { Err("folders only exist on this Mac".into()) } else { Ok(Path::new(&self.path)) }
    }

    /// Streams a file copy (an archive or a dump), with its size when known.
    pub fn open(&self) -> Result<(Box<dyn Read + Send>, Option<u64>), String> {
        if self.is_remote() {
            let cfg = self.s3.as_ref().ok_or("no S3 settings for this copy")?;
            s3::get(cfg, cfg.key_of(&self.path)?, None)
        } else {
            let f = fs::File::open(&self.path).map_err(|e| format!("{}: {e}", self.path))?;
            let len = f.metadata().map(|m| m.len()).ok();
            Ok((Box::new(f), len))
        }
    }
}
