//! The OpenShell policies cedar's sandboxes run under.
//!
//! Every sandbox (a conversation's, a personal tool call's, a tool's package install, a run's
//! browser) is an OpenShell sandbox, and OpenShell checks every connection it opens against a
//! policy: which program may reach which host and port, and for HTTP which methods and paths.
//! Anything no rule allows is refused and logged. Files are confined the same way (Landlock).
//!
//! A conversation's sandbox starts from the base policy `SANDBOX_POLICY` names: `offline` (the
//! default, no network at all), `packages` (`pip`/`npm` downloads from the public registries,
//! read-only), or the path of a policy file of the operator's own. cedar adds what its own tools
//! need (read-write `/workspace`) and the endpoints the key's owner has always allowed, and writes
//! the result where `openshell sandbox create --policy` reads it.
//!
//! Policies are written as JSON, which OpenShell reads as the YAML it is (JSON is a subset), so
//! cedar needs no YAML library. A policy file of the operator's own must be JSON for the same
//! reason; see `sandbox/policies/` for the presets as files to start from.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};
use std::path::Path;

/// Where every sandbox keeps the files that outlive a run (a Docker volume mounted there).
pub const WORKSPACE: &str = "/workspace";

/// The programs in the sandbox image that fetch packages: the Python interpreter (`pip` is a
/// script it runs, and OpenShell names the program that opens a connection by its real path)
/// and Node.js (`npm`).
const PYTHON: &str = "/usr/bin/python3*";
const NODE: &[&str] = &["/usr/bin/node", "/usr/local/bin/node"];

/// The paths every cedar sandbox reads: the system, its libraries and fonts, `/proc`, and the
/// random device. Paths an image lacks are skipped (`best_effort`).
const READ_ONLY: &[&str] = &[
    "/bin",
    "/sbin",
    "/usr",
    "/lib",
    "/lib64",
    "/etc",
    "/opt",
    "/proc",
    "/var/log",
    "/var/cache/fontconfig",
    "/dev/urandom",
    "/dev/random",
    "/dev/zero",
    "/dev/full",
];

/// The paths every cedar sandbox writes besides its working directory.
const READ_WRITE: &[&str] = &["/tmp", "/dev/null", "/dev/shm", "/home/agent"];

/// A connection the base policy doesn't allow, which the user let one program make: what the
/// approval of a blocked connection adds (see `approvals`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Endpoint {
    /// The program, by the path OpenShell reported for it (e.g. `/usr/bin/curl`).
    pub binary: String,
    pub host: String,
    pub port: u16,
}

impl Endpoint {
    /// How it is remembered when the user always allows it: `program host:port`.
    pub fn grant(&self) -> String {
        format!("{} {}:{}", self.binary, self.host, self.port)
    }

    /// The inverse of [`Self::grant`]. `None` for anything else in the grants table.
    pub fn from_grant(grant: &str) -> Option<Self> {
        let (binary, target) = grant.split_once(' ')?;
        let (host, port) = target.rsplit_once(':')?;
        let endpoint = Endpoint {
            binary: binary.to_string(),
            host: host.to_string(),
            port: port.parse().ok()?,
        };
        endpoint.check().ok()?;
        Some(endpoint)
    }

    /// Whether this is a rule cedar is willing to write: an absolute program path, a host name
    /// or address without anything that could change the rule's meaning, and a real port.
    pub fn check(&self) -> Result<()> {
        if !self.binary.starts_with('/') || self.binary.contains(['*', '?', '[', ' ', '\0']) {
            bail!("not a program path: {:?}", self.binary);
        }
        let host_ok = !self.host.is_empty()
            && self.host.len() <= 253
            && self
                .host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'))
            && !self.host.starts_with(['.', '-']);
        if !host_ok {
            bail!("not a host name: {:?}", self.host);
        }
        if self.port == 0 {
            bail!("port 0");
        }
        Ok(())
    }

    /// The name of the rule that allows it: stable, so allowing it again changes nothing.
    fn rule_name(&self) -> String {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(self.grant().as_bytes());
        format!("cedar_allowed_{}", &hex::encode(digest)[..12])
    }
}

/// A policy, as the JSON document OpenShell reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    /// What the base policy is called in `run.started` and the environment note: `offline`,
    /// `packages`, or the file's name.
    pub name: String,
    doc: Value,
}

impl Policy {
    /// The policy `SANDBOX_POLICY` names: `offline` (also the default), `packages`, or the path of
    /// a JSON policy file. The file is read and checked now, so a mistake stops cedar at startup.
    pub fn from_setting(setting: Option<&str>) -> Result<Self> {
        match setting.map(str::trim) {
            None | Some("") | Some("offline") | Some("none") => Ok(Self::offline()),
            Some("packages") => Ok(Self::packages()),
            Some(path) => Self::from_file(Path::new(path)),
        }
    }

    /// No network at all.
    pub fn offline() -> Self {
        Policy {
            name: "offline".into(),
            doc: base_document(Map::new()),
        }
    }

    /// Package downloads from PyPI and npm, read-only, for the programs that install them.
    pub fn packages() -> Self {
        let mut rules = Map::new();
        rules.insert(
            "pypi".into(),
            json!({
                "endpoints": [
                    read_only("pypi.org", 443),
                    read_only("files.pythonhosted.org", 443),
                ],
                "binaries": [{ "path": PYTHON }],
            }),
        );
        let mut npm = read_only("registry.npmjs.org", 443);
        // Scoped packages (`@types/node`) are requested with an encoded slash.
        npm["allow_encoded_slash"] = json!(true);
        rules.insert(
            "npm".into(),
            json!({
                "endpoints": [npm],
                "binaries": NODE.iter().map(|p| json!({ "path": p })).collect::<Vec<_>>(),
            }),
        );
        Policy {
            name: "packages".into(),
            doc: base_document(rules),
        }
    }

    /// A policy file of the operator's own, in JSON.
    pub fn from_file(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading SANDBOX_POLICY file {}", path.display()))?;
        let doc: Value = serde_json::from_str(&text).with_context(|| {
            format!(
                "SANDBOX_POLICY file {} is not JSON (cedar reads policies as JSON; see sandbox/policies/)",
                path.display()
            )
        })?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "custom".into());
        Self::from_document(name, doc)
    }

    /// A policy from its JSON document, checked for the shape OpenShell expects.
    pub fn from_document(name: String, doc: Value) -> Result<Self> {
        let obj = doc
            .as_object()
            .ok_or_else(|| anyhow!("a sandbox policy is a JSON object"))?;
        match obj.get("version") {
            Some(v) if v == 1 => {}
            other => bail!("a sandbox policy needs \"version\": 1, not {other:?}"),
        }
        if let Some(rules) = obj.get("network_policies") {
            let rules = rules
                .as_object()
                .ok_or_else(|| anyhow!("network_policies is an object of named rules"))?;
            for (rule, body) in rules {
                if !body["endpoints"].is_array() || !body["binaries"].is_array() {
                    bail!("network rule {rule:?} needs \"endpoints\" and \"binaries\" lists");
                }
            }
        }
        if let Some(fs) = obj.get("filesystem_policy") {
            if !fs.is_object() {
                bail!("filesystem_policy is an object");
            }
        }
        Ok(Policy { name, doc })
    }

    /// Whether any program may reach any host at all.
    pub fn networked(&self) -> bool {
        self.doc["network_policies"]
            .as_object()
            .is_some_and(|rules| !rules.is_empty())
    }

    /// The hosts and ports the policy lets something reach, for the note to the model.
    pub fn hosts(&self) -> Vec<String> {
        let mut out: Vec<String> = self.doc["network_policies"]
            .as_object()
            .into_iter()
            .flat_map(|rules| rules.values())
            .flat_map(|rule| rule["endpoints"].as_array().cloned().unwrap_or_default())
            .filter_map(|e| e["host"].as_str().map(String::from))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The policy for a conversation's sandbox: this one with `/workspace` writable, and with a
    /// rule for each of `allowed` (connections the key's owner always allows).
    pub fn for_workspace(&self, allowed: &[Endpoint]) -> Value {
        let mut doc = self.doc.clone();
        ensure_paths(&mut doc, READ_ONLY, &[WORKSPACE]);
        for endpoint in allowed {
            add_endpoint(&mut doc, endpoint);
        }
        doc
    }

    /// The JSON document as it is.
    pub fn document(&self) -> &Value {
        &self.doc
    }
}

/// `doc` with a rule that lets `endpoint.binary` reach `endpoint.host:endpoint.port`. TLS is
/// relayed as it is (`tls: skip`): the program checks the server's certificate itself, the way it
/// would anywhere else, and OpenShell sees the host and port but not the requests. OpenShell
/// refuses two rules for the same host and port that disagree on how to treat it, so when the
/// policy already has one, the new rule treats it the same way (e.g. read-only REST for PyPI).
pub fn add_endpoint(doc: &mut Value, endpoint: &Endpoint) {
    let target = existing_endpoint(doc, &endpoint.host, endpoint.port)
        .map(|mut e| {
            if let Some(e) = e.as_object_mut() {
                e.remove("ports");
                e.remove("path");
            }
            e["host"] = json!(endpoint.host);
            e["port"] = json!(endpoint.port);
            e
        })
        .unwrap_or_else(|| json!({ "host": endpoint.host, "port": endpoint.port, "tls": "skip" }));
    let rules = doc
        .as_object_mut()
        .expect("a policy is an object")
        .entry("network_policies")
        .or_insert_with(|| json!({}));
    if let Some(rules) = rules.as_object_mut() {
        rules.insert(
            endpoint.rule_name(),
            json!({
                "endpoints": [target],
                "binaries": [{ "path": endpoint.binary }],
            }),
        );
    }
}

/// An endpoint of `doc` for exactly `host` on `port`, if it has one.
fn existing_endpoint(doc: &Value, host: &str, port: u16) -> Option<Value> {
    doc["network_policies"]
        .as_object()?
        .values()
        .flat_map(|rule| rule["endpoints"].as_array().cloned().unwrap_or_default())
        .find(|e| {
            e["host"].as_str() == Some(host)
                && (e["port"].as_u64() == Some(port.into())
                    || e["ports"]
                        .as_array()
                        .is_some_and(|ps| ps.iter().any(|p| p.as_u64() == Some(port.into()))))
        })
}

/// The policy for one call of a personal tool: its own folder read-only, its environment and the
/// run's workspace writable, and the hosts its manifest declares, reachable by Python only.
pub fn for_tool(tool_mount: &str, env_mount: &str, hosts: &[String]) -> Value {
    let mut rules = Map::new();
    if !hosts.is_empty() {
        let endpoints: Vec<Value> = hosts
            .iter()
            .map(|h| json!({ "host": h, "ports": [80, 443], "tls": "skip" }))
            .collect();
        rules.insert(
            "declared_hosts".into(),
            json!({ "endpoints": endpoints, "binaries": [{ "path": PYTHON }] }),
        );
    }
    let mut doc = base_document(rules);
    ensure_paths(&mut doc, &[tool_mount], &[WORKSPACE, env_mount]);
    doc
}

/// The policy for installing a tool's packages: PyPI only, read-only, for Python, writing to the
/// tool's environment.
pub fn for_tool_setup(env_mount: &str) -> Value {
    let mut doc = Policy::packages().doc;
    ensure_paths(&mut doc, &[], &[env_mount]);
    doc
}

fn read_only(host: &str, port: u16) -> Value {
    json!({
        "host": host,
        "port": port,
        "protocol": "rest",
        "enforcement": "enforce",
        "access": "read-only",
    })
}

fn base_document(network: Map<String, Value>) -> Value {
    let mut doc = json!({
        "version": 1,
        "filesystem_policy": {
            "include_workdir": true,
            "read_only": READ_ONLY,
            "read_write": READ_WRITE,
        },
        "landlock": { "compatibility": "best_effort" },
    });
    if !network.is_empty() {
        doc["network_policies"] = Value::Object(network);
    }
    doc
}

/// Make sure `doc`'s filesystem policy lists `read_only` and `read_write`, without dropping
/// anything an operator's own policy lists. A path the policy already has keeps its access.
fn ensure_paths(doc: &mut Value, read_only: &[&str], read_write: &[&str]) {
    let fs = doc
        .as_object_mut()
        .expect("a policy is an object")
        .entry("filesystem_policy")
        .or_insert_with(|| json!({ "include_workdir": true }));
    let listed = |fs: &Value, path: &str| {
        ["read_only", "read_write"].iter().any(|k| {
            fs[*k]
                .as_array()
                .is_some_and(|a| a.iter().any(|p| p == path))
        })
    };
    for (key, paths) in [("read_write", read_write), ("read_only", read_only)] {
        for path in paths {
            if listed(fs, path) {
                continue;
            }
            let list = fs
                .as_object_mut()
                .expect("filesystem_policy is an object")
                .entry(key)
                .or_insert_with(|| json!([]));
            if let Some(list) = list.as_array_mut() {
                list.push(json!(path));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint() -> Endpoint {
        Endpoint {
            binary: "/usr/bin/curl".into(),
            host: "api.example.com".into(),
            port: 443,
        }
    }

    /// `sandbox/policies/` holds the presets as files to start a policy of one's own from; they
    /// must say what cedar does.
    #[test]
    fn the_preset_files_are_the_presets() {
        for (file, preset) in [
            (
                include_str!("../sandbox/policies/offline.json"),
                Policy::offline(),
            ),
            (
                include_str!("../sandbox/policies/packages.json"),
                Policy::packages(),
            ),
        ] {
            let doc: Value = serde_json::from_str(file).unwrap();
            assert_eq!(&doc, preset.document(), "{}", preset.name);
            let from_file = Policy::from_document(preset.name.clone(), doc).unwrap();
            assert_eq!(from_file.for_workspace(&[]), preset.for_workspace(&[]));
        }
    }

    #[test]
    fn the_default_is_offline() {
        for setting in [None, Some(""), Some("offline"), Some(" none ")] {
            let policy = Policy::from_setting(setting).unwrap();
            assert_eq!(policy.name, "offline");
            assert!(!policy.networked());
            assert!(policy.document().get("network_policies").is_none());
        }
    }

    #[test]
    fn packages_reach_the_registries_read_only_from_their_installers() {
        let policy = Policy::from_setting(Some("packages")).unwrap();
        assert!(policy.networked());
        assert_eq!(
            policy.hosts(),
            ["files.pythonhosted.org", "pypi.org", "registry.npmjs.org"]
        );
        let pypi = &policy.document()["network_policies"]["pypi"];
        assert_eq!(pypi["binaries"][0]["path"], PYTHON);
        for e in pypi["endpoints"].as_array().unwrap() {
            assert_eq!(e["access"], "read-only");
            assert_eq!(e["enforcement"], "enforce");
        }
    }

    #[test]
    fn a_workspace_policy_writes_the_workspace_and_adds_allowed_endpoints() {
        let doc = Policy::offline().for_workspace(&[endpoint()]);
        let rw = doc["filesystem_policy"]["read_write"].as_array().unwrap();
        assert!(rw.contains(&json!("/workspace")) && rw.contains(&json!("/tmp")));
        let rules = doc["network_policies"].as_object().unwrap();
        assert_eq!(rules.len(), 1);
        let rule = rules.values().next().unwrap();
        assert_eq!(
            rule["endpoints"][0],
            json!({ "host": "api.example.com", "port": 443, "tls": "skip" })
        );
        assert_eq!(rule["binaries"][0]["path"], "/usr/bin/curl");
        // The same endpoint twice is one rule.
        let doc = Policy::offline().for_workspace(&[endpoint(), endpoint()]);
        assert_eq!(doc["network_policies"].as_object().unwrap().len(), 1);
    }

    #[test]
    fn allowing_an_endpoint_the_policy_has_treats_it_the_same_way() {
        let curl_to_pypi = Endpoint {
            binary: "/usr/bin/curl".into(),
            host: "pypi.org".into(),
            port: 443,
        };
        let doc = Policy::packages().for_workspace(std::slice::from_ref(&curl_to_pypi));
        let added = &doc["network_policies"][curl_to_pypi.rule_name()];
        assert_eq!(
            added["endpoints"][0],
            json!({ "host": "pypi.org", "port": 443, "protocol": "rest", "enforcement": "enforce", "access": "read-only" })
        );
        assert_eq!(added["binaries"][0]["path"], "/usr/bin/curl");
    }

    #[test]
    fn an_operators_policy_keeps_its_own_paths() {
        let doc = json!({
            "version": 1,
            "filesystem_policy": { "read_only": ["/workspace", "/data"], "read_write": ["/scratch"] },
            "network_policies": { "gh": { "endpoints": [{ "host": "api.github.com", "port": 443 }], "binaries": [{ "path": "/usr/bin/gh" }] } },
        });
        let policy = Policy::from_document("mine".into(), doc).unwrap();
        let out = policy.for_workspace(&[]);
        let fs = &out["filesystem_policy"];
        // A path the operator listed keeps the access they gave it.
        assert!(fs["read_only"]
            .as_array()
            .unwrap()
            .contains(&json!("/workspace")));
        assert!(!fs["read_write"]
            .as_array()
            .unwrap()
            .contains(&json!("/workspace")));
        assert!(fs["read_write"]
            .as_array()
            .unwrap()
            .contains(&json!("/scratch")));
        assert!(fs["read_only"].as_array().unwrap().contains(&json!("/usr")));
        assert_eq!(policy.hosts(), ["api.github.com"]);
    }

    #[test]
    fn bad_policy_files_are_refused() {
        for (doc, want) in [
            (json!([]), "object"),
            (json!({}), "version"),
            (json!({ "version": 2 }), "version"),
            (
                json!({ "version": 1, "network_policies": [] }),
                "named rules",
            ),
            (
                json!({ "version": 1, "network_policies": { "x": { "endpoints": [] } } }),
                "binaries",
            ),
        ] {
            let err = Policy::from_document("x".into(), doc)
                .unwrap_err()
                .to_string();
            assert!(err.contains(want), "{err}");
        }
        let dir = std::env::temp_dir().join(format!("cedar-policy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let yaml = dir.join("p.yaml");
        std::fs::write(&yaml, "version: 1\n").unwrap();
        assert!(Policy::from_setting(Some(yaml.to_str().unwrap()))
            .unwrap_err()
            .to_string()
            .contains("not JSON"));
        let good = dir.join("strict.json");
        std::fs::write(&good, r#"{"version": 1}"#).unwrap();
        let policy = Policy::from_setting(Some(good.to_str().unwrap())).unwrap();
        assert_eq!(policy.name, "strict");
        assert!(Policy::from_setting(Some("/no/such/policy.json")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn grants_round_trip_and_bad_ones_are_ignored() {
        let e = endpoint();
        assert_eq!(e.grant(), "/usr/bin/curl api.example.com:443");
        assert_eq!(Endpoint::from_grant(&e.grant()), Some(e));
        let v6 = Endpoint::from_grant("/usr/bin/curl 2001:db8::1:8443").unwrap();
        assert_eq!((v6.host.as_str(), v6.port), ("2001:db8::1", 8443));
        for bad in [
            "sandbox",
            "requests",
            "curl api.example.com:443",
            "/usr/bin/* api.example.com:443",
            "/usr/bin/curl api.example.com:http",
            "/usr/bin/curl api.example.com:0",
            "/usr/bin/curl -x.com:443",
            "/usr/bin/curl a\"b.com:443",
        ] {
            assert_eq!(Endpoint::from_grant(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_tool_reaches_only_its_declared_hosts() {
        let doc = for_tool(
            "/opt/cedar/tool",
            "/opt/cedar/env",
            &["api.example.com".into()],
        );
        let rule = &doc["network_policies"]["declared_hosts"];
        assert_eq!(rule["endpoints"][0]["host"], "api.example.com");
        assert_eq!(rule["binaries"][0]["path"], PYTHON);
        let fs = &doc["filesystem_policy"];
        assert!(fs["read_only"]
            .as_array()
            .unwrap()
            .contains(&json!("/opt/cedar/tool")));
        assert!(fs["read_write"]
            .as_array()
            .unwrap()
            .contains(&json!("/opt/cedar/env")));
        assert!(fs["read_write"]
            .as_array()
            .unwrap()
            .contains(&json!("/workspace")));

        let offline = for_tool("/opt/cedar/tool", "/opt/cedar/env", &[]);
        assert!(offline.get("network_policies").is_none());

        let setup = for_tool_setup("/opt/cedar/env");
        assert_eq!(
            setup["network_policies"]["pypi"]["binaries"][0]["path"],
            PYTHON
        );
        assert!(setup["filesystem_policy"]["read_write"]
            .as_array()
            .unwrap()
            .contains(&json!("/opt/cedar/env")));
    }
}
