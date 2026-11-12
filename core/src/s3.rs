//! Reads backups from an S3-compatible bucket laid out as `{prefix}{tenant}/{filename}`.
//! Requests are presigned URLs, so no SDK.

use rusty_s3::actions::ListObjectsV2;
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use serde::Deserialize;
use std::io::Read;
use std::sync::OnceLock;
use std::time::Duration;

/// An S3-compatible bucket: AWS, Backblaze, Wasabi, Aliyun OSS, MinIO…
#[derive(Deserialize, Clone, Debug)]
pub struct Config {
    /// Host without scheme, like `s3.amazonaws.com`. A scheme is accepted too.
    pub endpoint: String,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default = "yes")]
    pub use_ssl: bool,
    #[serde(default)]
    pub region: String,
    pub bucket: String,
    #[serde(default)]
    pub prefix: String,
    pub access_key: String,
    pub secret_key: String,
}

fn yes() -> bool {
    true
}

const SIGN_FOR: Duration = Duration::from_secs(3600);

pub struct Object {
    pub key: String,
    pub size: u64,
    pub last_modified: Option<String>,
}

impl Config {
    fn bucket(&self) -> Result<(Bucket, Credentials), String> {
        let mut host = self.endpoint.trim().trim_end_matches('/');
        let mut ssl = self.use_ssl;
        if let Some(h) = host.strip_prefix("https://") {
            (host, ssl) = (h, true);
        } else if let Some(h) = host.strip_prefix("http://") {
            (host, ssl) = (h, false);
        }
        // "<bucket>.<endpoint>" is a common way to write it; the bucket goes in the path.
        let dotted = format!("{}.", self.bucket);
        if host.len() > dotted.len() && host[..dotted.len()].eq_ignore_ascii_case(&dotted) {
            host = &host[dotted.len()..];
        }
        if host.is_empty() || self.bucket.is_empty() {
            return Err("endpoint and bucket are required".into());
        }
        let scheme = if ssl { "https" } else { "http" };
        let url = match self.port {
            Some(p) if p != 0 && p != if ssl { 443 } else { 80 } => format!("{scheme}://{host}:{p}"),
            _ => format!("{scheme}://{host}"),
        };
        let endpoint = url.parse().map_err(|e| format!("invalid endpoint: {e}"))?;
        // Aliyun OSS only takes virtual-hosted requests, and its region without "oss-".
        let aliyun = host.to_ascii_lowercase().ends_with("aliyuncs.com");
        let mut region = if self.region.trim().is_empty() { "us-east-1".to_string() } else { self.region.trim().to_string() };
        if aliyun {
            if let Some(r) = region.strip_prefix("oss-") {
                region = r.to_string();
            }
        }
        let style = if aliyun { UrlStyle::VirtualHost } else { UrlStyle::Path };
        let bucket = Bucket::new(endpoint, style, self.bucket.clone(), region).map_err(|e| e.to_string())?;
        Ok((bucket, Credentials::new(self.access_key.trim(), self.secret_key.trim())))
    }

    /// "" or "<prefix>/".
    pub fn prefix(&self) -> String {
        let p = self.prefix.trim_matches('/');
        if p.is_empty() { String::new() } else { format!("{p}/") }
    }

    /// How copies in this bucket are named: `s3://<bucket>/<key>`.
    pub fn path_of(&self, key: &str) -> String {
        format!("s3://{}/{key}", self.bucket)
    }

    pub fn key_of<'a>(&self, path: &'a str) -> Result<&'a str, String> {
        path.strip_prefix(&format!("s3://{}/", self.bucket)).ok_or_else(|| format!("{path} is not in bucket {}", self.bucket))
    }
}

fn agent() -> &'static ureq::Agent {
    static A: OnceLock<ureq::Agent> = OnceLock::new();
    A.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .build()
            .into()
    })
}

/// `<Error><Code>…</Code><Message>…</Message></Error>` → "Code: Message".
fn s3_error(status: u16, body: &str) -> String {
    let tag = |t: &str| {
        let start = body.find(&format!("<{t}>"))? + t.len() + 2;
        let end = body[start..].find(&format!("</{t}>"))? + start;
        Some(body[start..end].to_string())
    };
    match (tag("Code"), tag("Message")) {
        (Some(c), Some(m)) => format!("S3 {status} {c}: {m}"),
        (Some(c), None) => format!("S3 {status} {c}"),
        _ => format!("S3 error {status}"),
    }
}

type Stream = (Box<dyn Read + Send>, Option<u64>);

fn fetch(url: &str, range: Option<u64>) -> Result<Stream, String> {
    let mut req = agent().get(url);
    if let Some(end) = range {
        req = req.header("Range", format!("bytes=0-{end}"));
    }
    let resp = req.call().map_err(|e| format!("cannot reach S3: {e}"))?;
    let status = resp.status();
    let len = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|s| s.parse().ok());
    let mut body = resp.into_body();
    if !status.is_success() {
        let text = body.read_to_string().unwrap_or_default();
        return Err(s3_error(status.as_u16(), &text));
    }
    Ok((Box::new(body.into_reader()), len))
}

/// Every object under the prefix.
pub fn list(cfg: &Config) -> Result<Vec<Object>, String> {
    let (bucket, creds) = cfg.bucket()?;
    let prefix = cfg.prefix();
    let mut token: Option<String> = None;
    let mut out = Vec::new();
    loop {
        let mut action = bucket.list_objects_v2(Some(&creds));
        if !prefix.is_empty() {
            action.with_prefix(prefix.as_str());
        }
        if let Some(t) = &token {
            action.with_continuation_token(t.as_str());
        }
        let (mut body, _) = fetch(action.sign(SIGN_FOR).as_str(), None)?;
        let mut text = String::new();
        body.read_to_string(&mut text).map_err(|e| e.to_string())?;
        let page = ListObjectsV2::parse_response(&text).map_err(|e| format!("cannot read the S3 listing: {e}"))?;
        out.extend(page.contents.into_iter().map(|c| Object { key: c.key, size: c.size.unwrap_or(0), last_modified: c.last_modified }));
        match page.next_continuation_token {
            Some(t) => token = Some(t),
            None => return Ok(out),
        }
    }
}

/// Streams an object, or only its first `first_bytes` bytes.
pub fn get(cfg: &Config, key: &str, first_bytes: Option<u64>) -> Result<Stream, String> {
    let (bucket, creds) = cfg.bucket()?;
    let url = bucket.get_object(Some(&creds), key).sign(SIGN_FOR);
    fetch(url.as_str(), first_bytes.map(|n| n.saturating_sub(1)))
}

#[cfg(test)]
pub(crate) mod testing {
    //! Writes to a bucket, for the tests only; the app never does.
    use super::*;

    pub fn send(method: &str, url: &str, body: &[u8]) {
        let resp = match method {
            "PUT" => agent().put(url).send(body),
            "DELETE" => agent().delete(url).call(),
            _ => unreachable!(),
        }
        .unwrap();
        assert!(resp.status().is_success(), "{method} {url}: {}", resp.status());
    }

    pub fn create_bucket(cfg: &Config) {
        let (b, c) = cfg.bucket().unwrap();
        send("PUT", b.create_bucket(&c).sign(SIGN_FOR).as_str(), b"");
    }

    pub fn put(cfg: &Config, key: &str, body: &[u8]) {
        let (b, c) = cfg.bucket().unwrap();
        send("PUT", b.put_object(Some(&c), key).sign(SIGN_FOR).as_str(), body);
    }

    pub fn delete_all(cfg: &Config) {
        let cfg = &Config { prefix: String::new(), ..cfg.clone() };
        let (b, c) = cfg.bucket().unwrap();
        for o in list(cfg).unwrap() {
            send("DELETE", b.delete_object(Some(&c), &o.key).sign(SIGN_FOR).as_str(), b"");
        }
        send("DELETE", b.delete_bucket(&c).sign(SIGN_FOR).as_str(), b"");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(endpoint: &str, port: Option<u16>, ssl: bool) -> Config {
        Config {
            endpoint: endpoint.into(),
            port,
            use_ssl: ssl,
            region: String::new(),
            bucket: "backups".into(),
            prefix: "/prod/".into(),
            access_key: "k".into(),
            secret_key: "s".into(),
        }
    }

    #[test]
    fn endpoint_forms() {
        let url = |c: Config| c.bucket().unwrap().0.base_url().to_string();
        assert_eq!(url(cfg("s3.amazonaws.com", Some(443), true)), "https://s3.amazonaws.com/backups/");
        assert_eq!(url(cfg("localhost", Some(9000), false)), "http://localhost:9000/backups/");
        assert_eq!(url(cfg("backups.s3.example.com", None, true)), "https://s3.example.com/backups/");
        assert_eq!(url(cfg("https://minio.example.com/", Some(443), false)), "https://minio.example.com/backups/");
        assert_eq!(url(cfg("oss-ap-southeast-1.aliyuncs.com", None, true)), "https://backups.oss-ap-southeast-1.aliyuncs.com/");
        let c = cfg("x", None, true);
        assert_eq!(c.prefix(), "prod/");
        assert_eq!(c.key_of(&c.path_of("prod/shop/a.tar.gz")).unwrap(), "prod/shop/a.tar.gz");
    }

    #[test]
    fn error_messages() {
        let body = "<?xml version=\"1.0\"?><Error><Code>AccessDenied</Code><Message>Access Denied.</Message></Error>";
        assert_eq!(s3_error(403, body), "S3 403 AccessDenied: Access Denied.");
        assert_eq!(s3_error(500, ""), "S3 error 500");
    }
}
