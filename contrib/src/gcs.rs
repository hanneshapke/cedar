//! A small Cloud Storage client over its JSON API: upload (optionally only if the object is new),
//! read, delete, list by prefix and patch metadata. Authenticates with the instance's service
//! account through the metadata server, or with `GOOGLE_OAUTH_ACCESS_TOKEN` elsewhere.

use anyhow::{anyhow, bail, Result};
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const API: &str = "https://storage.googleapis.com/storage/v1";
const UPLOAD: &str = "https://storage.googleapis.com/upload/storage/v1";
const METADATA_TOKEN: &str = "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token";

pub struct Gcs {
    http: reqwest::Client,
    bucket: String,
    fixed_token: Option<String>,
    cached: Mutex<Option<(String, Instant)>>,
}

/// What an upload that asked for a new object found.
#[derive(Debug, PartialEq, Eq)]
pub enum Upload {
    Created,
    AlreadyExists,
}

impl Gcs {
    pub fn new(bucket: &str, fixed_token: Option<String>) -> Result<Self> {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(30)).build()?;
        Ok(Gcs { http, bucket: bucket.to_string(), fixed_token, cached: Mutex::new(None) })
    }

    async fn token(&self) -> Result<String> {
        if let Some(t) = &self.fixed_token {
            return Ok(t.clone());
        }
        if let Some((t, until)) = self.cached.lock().unwrap().clone() {
            if Instant::now() < until {
                return Ok(t);
            }
        }
        let resp: Value = self
            .http
            .get(METADATA_TOKEN)
            .header("Metadata-Flavor", "Google")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let token = resp["access_token"].as_str().ok_or_else(|| anyhow!("no access_token from the metadata server"))?.to_string();
        let expires = resp["expires_in"].as_u64().unwrap_or(600).saturating_sub(60);
        *self.cached.lock().unwrap() = Some((token.clone(), Instant::now() + Duration::from_secs(expires)));
        Ok(token)
    }

    fn object_url(&self, name: &str) -> String {
        format!("{API}/b/{}/o/{}", self.bucket, urlencode(name))
    }

    /// Upload `bytes` as `name` with custom `metadata`; with `only_new`, refuse to replace an
    /// existing object (that is how a hash is claimed atomically).
    pub async fn upload(&self, name: &str, bytes: &[u8], content_type: &str, metadata: &BTreeMap<String, String>, only_new: bool) -> Result<Upload> {
        let boundary = "cedar-contrib-7b2e1c";
        let meta = json!({ "name": name, "contentType": content_type, "metadata": metadata });
        let mut body = Vec::with_capacity(bytes.len() + 512);
        body.extend_from_slice(format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n--{boundary}\r\nContent-Type: {content_type}\r\n\r\n").as_bytes());
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--").as_bytes());
        let mut url = format!("{UPLOAD}/b/{}/o?uploadType=multipart", self.bucket);
        if only_new {
            url.push_str("&ifGenerationMatch=0");
        }
        let resp = self
            .http
            .post(url)
            .bearer_auth(self.token().await?)
            .header("content-type", format!("multipart/related; boundary={boundary}"))
            .body(body)
            .send()
            .await?;
        match resp.status() {
            s if s.is_success() => Ok(Upload::Created),
            StatusCode::PRECONDITION_FAILED if only_new => Ok(Upload::AlreadyExists),
            s => bail!("upload of {name} failed: {s} {}", resp.text().await.unwrap_or_default()),
        }
    }

    pub async fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let resp = self.http.get(format!("{}?alt=media", self.object_url(name))).bearer_auth(self.token().await?).send().await?;
        match resp.status() {
            StatusCode::NOT_FOUND => Ok(None),
            s if s.is_success() => Ok(Some(resp.bytes().await?.to_vec())),
            s => bail!("reading {name} failed: {s}"),
        }
    }

    /// Delete; false if there was no such object.
    pub async fn delete(&self, name: &str) -> Result<bool> {
        let resp = self.http.delete(self.object_url(name)).bearer_auth(self.token().await?).send().await?;
        match resp.status() {
            StatusCode::NOT_FOUND => Ok(false),
            s if s.is_success() => Ok(true),
            s => bail!("deleting {name} failed: {s}"),
        }
    }

    /// The names of the objects under `prefix`.
    pub async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let mut names = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut url = format!("{API}/b/{}/o?prefix={}&fields=items(name),nextPageToken&maxResults=1000", self.bucket, urlencode(prefix));
            if let Some(p) = &page {
                url.push_str(&format!("&pageToken={}", urlencode(p)));
            }
            let resp: Value = self.http.get(url).bearer_auth(self.token().await?).send().await?.error_for_status()?.json().await?;
            names.extend(resp["items"].as_array().into_iter().flatten().filter_map(|i| i["name"].as_str().map(String::from)));
            match resp["nextPageToken"].as_str() {
                Some(t) => page = Some(t.to_string()),
                None => return Ok(names),
            }
        }
    }

    /// Replace an object's custom metadata keys with `metadata` (keys not named are kept).
    pub async fn patch_metadata(&self, name: &str, metadata: &BTreeMap<String, String>) -> Result<()> {
        self.http
            .patch(self.object_url(name))
            .bearer_auth(self.token().await?)
            .json(&json!({ "metadata": metadata }))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}

/// Percent-encode an object name or query value (RFC 3986 unreserved characters stay).
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_are_percent_encoded_for_the_api() {
        assert_eq!(super::urlencode("bundles/2026-10/a b.json"), "bundles%2F2026-10%2Fa%20b.json");
    }
}
