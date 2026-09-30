//! Minimal blocking node REST client with an on-disk response cache.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

pub struct Node {
    base: String,
    client: reqwest::blocking::Client,
    cache: Option<PathBuf>,
}

impl Node {
    pub fn new(base: &str, cache: Option<PathBuf>) -> Result<Self> {
        if let Some(dir) = &cache {
            std::fs::create_dir_all(dir).with_context(|| format!("create cache dir {dir:?}"))?;
        }
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()?,
            cache,
        })
    }

    fn cache_path(&self, path: &str) -> Option<PathBuf> {
        let dir = self.cache.as_ref()?;
        let name: String = path
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        Some(dir.join(name))
    }

    /// GET `path` (starting with `/`) and return the body. Successful responses are cached.
    pub fn get(&self, path: &str) -> Result<String> {
        let cached = self.cache_path(path);
        if let Some(p) = &cached {
            if let Ok(s) = std::fs::read_to_string(p) {
                return Ok(s);
            }
        }
        let url = format!("{}{}", self.base, path);
        let mut last = anyhow!("no attempt made");
        for attempt in 0..4u64 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(300 * attempt));
            }
            match self.client.get(&url).send() {
                Ok(r) if r.status().is_success() => match r.text() {
                    Ok(body) => {
                        if let Some(p) = &cached {
                            let tmp = p.with_extension(format!("tmp{}", std::process::id()));
                            if std::fs::write(&tmp, &body).is_ok() {
                                let _ = std::fs::rename(&tmp, p);
                            }
                        }
                        return Ok(body);
                    }
                    Err(e) => last = anyhow!("GET {url}: reading body: {e}"),
                },
                Ok(r) if r.status().is_client_error() => {
                    return Err(anyhow!("GET {url}: HTTP {}", r.status()));
                }
                Ok(r) => last = anyhow!("GET {url}: HTTP {}", r.status()),
                Err(e) => last = anyhow!("GET {url}: {e}"),
            }
        }
        Err(last)
    }

    pub fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let body = self.get(path)?;
        serde_json::from_str(&body).with_context(|| format!("parsing JSON from {path}"))
    }
}
