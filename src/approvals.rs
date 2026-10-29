//! Which tool calls wait for the user's OK before they run, in runs whose client answers
//! (`interactive`). The rules are a heuristic on the tool and its input: sending email, saving
//! files from the web, installing packages (when the sandbox reaches a network at all), deleting
//! files, and every call of a community tool installed from the marketplace. The `network` rule
//! depends on the sandbox's runtime: a Docker sandbox with a network asks before running any code,
//! since that code can reach anything; an OpenShell sandbox asks afterwards, when its policy
//! refused a connection, whether to let that program reach that host from then on.
//! `TOOL_APPROVALS` picks the rules; `off` turns the gate off.

use crate::policy::Endpoint;
use crate::sandbox::Reach;
use crate::tools::{group_of, writes, PythonTool};
use anyhow::{bail, Result};
use serde_json::Value;
use std::collections::BTreeSet;

/// A kind of call that needs approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rule {
    /// Gmail tools that send, draft or change mail.
    Email,
    /// `download_file`.
    Downloads,
    /// `pip install`, `npm i`, `apt install`, ... in `bash` or `run_python`.
    Installs,
    /// `rm` and friends in `bash`.
    Deletes,
    /// Docker: any `bash` or `run_python` call while the sandbox has a network. OpenShell: a
    /// connection the sandbox's policy refused, which the user may allow from then on.
    Network,
    /// A community tool installed from the marketplace. "Always" covers that release only, so an
    /// update asks again.
    Community,
}

impl Rule {
    pub const ALL: [Rule; 6] = [
        Rule::Email,
        Rule::Downloads,
        Rule::Installs,
        Rule::Deletes,
        Rule::Network,
        Rule::Community,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Rule::Email => "email",
            Rule::Downloads => "downloads",
            Rule::Installs => "installs",
            Rule::Deletes => "deletes",
            Rule::Network => "network",
            Rule::Community => "community",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Rule::ALL.into_iter().find(|r| r.as_str() == s)
    }
}

/// The rules in force, from `TOOL_APPROVALS`: `on` (the default) for all of them, `off` for none,
/// or a comma-separated list of rule names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules(BTreeSet<Rule>);

impl Default for Rules {
    fn default() -> Self {
        Rules(Rule::ALL.into_iter().collect())
    }
}

impl Rules {
    pub fn none() -> Self {
        Rules(BTreeSet::new())
    }

    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim().to_ascii_lowercase();
        match raw.as_str() {
            "" | "on" | "all" => return Ok(Self::default()),
            "off" | "none" => return Ok(Self::none()),
            _ => {}
        }
        let mut rules = BTreeSet::new();
        for name in raw.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            match Rule::parse(name) {
                Some(rule) => {
                    rules.insert(rule);
                }
                None => bail!(
                    "TOOL_APPROVALS: unknown rule {name:?} (use on, off, or a list of: {})",
                    Rule::ALL.map(Rule::as_str).join(", ")
                ),
            }
        }
        Ok(Rules(rules))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn has(&self, rule: Rule) -> bool {
        self.0.contains(&rule)
    }
}

/// Why a call waits, as shown on the approval card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    pub rule: Rule,
    /// One line on what the call will do.
    pub reason: String,
    /// What "Always for ..." would allow from now on (a host, package names), as shown. `None`:
    /// this kind of call is asked about every time.
    pub scope: Option<String>,
    /// The scope, split into what is remembered: one entry per host or package.
    pub grants: Vec<String>,
}

/// Whether calling `name` with `input` needs the user's OK under `rules`, given what the run's
/// sandbox can reach.
pub fn check(rules: &Rules, name: &str, input: &Value, reach: &Reach) -> Option<Gate> {
    match name {
        "download_file" if rules.has(Rule::Downloads) => {
            let url = input["url"].as_str().unwrap_or_default();
            let host = host_of(url);
            let file = input["path"]
                .as_str()
                .filter(|p| !p.is_empty())
                .or_else(|| url.rsplit('/').next().filter(|f| !f.is_empty()))
                .unwrap_or("a file");
            let from = host
                .as_deref()
                .map_or(String::new(), |h| format!(" from {h}"));
            Some(Gate {
                rule: Rule::Downloads,
                reason: format!("Save {file}{from} to the workspace."),
                grants: host.iter().cloned().collect(),
                scope: host,
            })
        }
        "bash" | "run_python" => {
            let code = input[if name == "bash" { "command" } else { "code" }]
                .as_str()
                .unwrap_or_default();
            if name == "bash" && rules.has(Rule::Deletes) && deletes(code) {
                return Some(Gate {
                    rule: Rule::Deletes,
                    reason: format!("Delete files in the workspace: {}", one_line(code)),
                    scope: None,
                    grants: vec![],
                });
            }
            // Without a network an install can't fetch anything, so there is nothing to ask.
            let packages = installs(code);
            if reach.any() && rules.has(Rule::Installs) && !packages.is_empty() {
                return Some(Gate {
                    rule: Rule::Installs,
                    reason: format!("Install {} from the internet.", packages.join(", ")),
                    scope: Some(packages.join(", ")),
                    grants: packages,
                });
            }
            // On Docker the code can reach the whole network, so it is asked about up front.
            if matches!(reach, Reach::Network(_)) && rules.has(Rule::Network) {
                let what = if name == "bash" {
                    "Run a command"
                } else {
                    "Run Python"
                };
                return Some(Gate {
                    rule: Rule::Network,
                    reason: format!("{what} with internet access: {}", one_line(code)),
                    scope: Some("code with internet access".into()),
                    grants: vec!["sandbox".into()],
                });
            }
            None
        }
        _ if rules.has(Rule::Email) && group_of(name) == "gmail" && writes(name) => {
            let to = ["to", "recipient", "recipients"]
                .iter()
                .find_map(|k| match &input[k] {
                    Value::String(s) if !s.is_empty() => Some(s.clone()),
                    Value::Array(a) if !a.is_empty() => Some(
                        a.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                    _ => None,
                });
            let subject = input["subject"].as_str().filter(|s| !s.is_empty());
            let mut reason = format!("{} in your Gmail", humanize(name));
            if let Some(to) = to {
                reason.push_str(&format!(" to {to}"));
            }
            if let Some(subject) = subject {
                reason.push_str(&format!(": \"{}\"", one_line(subject)));
            }
            reason.push('.');
            Some(Gate {
                rule: Rule::Email,
                reason,
                scope: None,
                grants: vec![],
            })
        }
        _ => None,
    }
}

/// Whether the user is asked to let `endpoint.binary` reach `endpoint.host:port` after the
/// sandbox's policy refused it.
pub fn check_connection(rules: &Rules, endpoint: &Endpoint) -> Option<Gate> {
    if !rules.has(Rule::Network) {
        return None;
    }
    let program = endpoint
        .binary
        .rsplit('/')
        .next()
        .unwrap_or(&endpoint.binary);
    let target = format!("{}:{}", endpoint.host, endpoint.port);
    Some(Gate {
        rule: Rule::Network,
        reason: format!(
            "Let {program} connect to {target}. The sandbox's network policy blocked it."
        ),
        scope: Some(format!("{program} → {target}")),
        grants: vec![endpoint.grant()],
    })
}

/// Whether a call of the personal tool `tool` needs the user's OK under `rules`: every call of
/// one installed from the marketplace does, until the user always allows that release.
pub fn check_personal(rules: &Rules, tool: &PythonTool) -> Option<Gate> {
    use crate::tools::Tool;
    let installed = tool.installed()?;
    if !rules.has(Rule::Community) {
        return None;
    }
    let name = tool.name();
    let mut reason = format!(
        "Run {name} {}, a community tool from the marketplace that this server's operator didn't write",
        installed.version
    );
    if !tool.network().is_empty() {
        reason.push_str(&format!(
            ", with internet access to {}",
            tool.network().join(", ")
        ));
    }
    if !tool.secrets().is_empty() {
        let names: Vec<&str> = tool.secrets().iter().map(|s| s.name.as_str()).collect();
        reason.push_str(&format!(", using the saved {}", names.join(", ")));
    }
    if tool.health_data() {
        reason.push_str(", on your health data");
    }
    reason.push('.');
    Some(Gate {
        rule: Rule::Community,
        reason,
        scope: Some(format!("{name} {}", installed.version)),
        grants: vec![format!("{name}@{}", installed.digest)],
    })
}

/// `gmail_send_message` -> "Send message".
fn humanize(name: &str) -> String {
    let words: Vec<&str> = name.split('_').skip(1).collect();
    let mut s = words.join(" ");
    if let Some(first) = s.get(..1) {
        s = first.to_uppercase() + &s[1..];
    }
    s
}

fn host_of(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    Some(host.strip_prefix("www.").map(String::from).unwrap_or(host))
}

/// The first line of `s`, cut to a readable length.
fn one_line(s: &str) -> String {
    let line = s.trim().lines().next().unwrap_or_default();
    let mut out: String = line.chars().take(160).collect();
    if out.len() < line.len() || s.trim().lines().nth(1).is_some() {
        out.push('…');
    }
    out
}

/// The commands in a script, each as its words: split at `;`, `&&`, `||`, pipes, subshells and
/// line breaks, then at whitespace and quotes. Rough, but enough to find what runs.
fn commands(code: &str) -> Vec<Vec<&str>> {
    code.split(['\n', ';', '&', '|', '(', ')', '`'])
        .map(|cmd| {
            cmd.split(|c: char| c.is_whitespace() || c == '"' || c == '\'')
                .filter(|w| !w.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// A command's words without the wrappers that run it (`sudo`, `xargs`, `exec`, ...).
fn unwrapped<'a, 'b>(words: &'b [&'a str]) -> &'b [&'a str] {
    let skip = words
        .iter()
        .take_while(|w| {
            matches!(
                **w,
                "sudo" | "xargs" | "exec" | "then" | "do" | "else" | "env" | "time" | "nohup"
            )
        })
        .count();
    &words[skip..]
}

/// Whether a command removes files: `rm`, `rmdir`, `shred`, `unlink`, or `find ... -delete`.
fn deletes(code: &str) -> bool {
    commands(code).iter().any(|words| {
        let words = unwrapped(words);
        matches!(words.first(), Some(&("rm" | "rmdir" | "shred" | "unlink")))
            || words.contains(&"-delete")
            || words.windows(2).any(|w| w[0] == "-exec" && w[1] == "rm")
    })
}

/// The packages a command or script installs, by name, in order and without repeats.
fn installs(code: &str) -> Vec<String> {
    const MANAGERS: &[(&[&str], &[&str])] = &[
        (&["pip", "pip3"], &["install"]),
        (&["uv"], &["add"]),
        (&["npm", "pnpm"], &["i", "install", "add"]),
        (&["yarn"], &["add"]),
        (&["apt", "apt-get"], &["install"]),
        (&["cargo", "gem", "go", "conda", "brew"], &["install"]),
    ];
    let mut out: Vec<String> = Vec::new();
    for words in commands(code) {
        // Find the package manager anywhere in the command: `python -m pip install`, `!pip ...`,
        // `os.system('pip install ...')`.
        let Some(i) = words.iter().position(|w| {
            let w = w.trim_start_matches(['!', '%']);
            MANAGERS.iter().any(|(names, _)| names.contains(&w))
        }) else {
            continue;
        };
        let tool = words[i].trim_start_matches(['!', '%']);
        let (_, verbs) = MANAGERS
            .iter()
            .find(|(names, _)| names.contains(&tool))
            .unwrap();
        let mut j = i + 1;
        if tool == "uv" && words.get(j) == Some(&"pip") {
            j += 1;
        }
        let verb = words.get(j).copied().unwrap_or_default();
        if !(verbs.contains(&verb) || (tool == "uv" && verb == "install")) {
            continue;
        }
        for arg in &words[j + 1..] {
            if arg.starts_with('#') {
                break;
            }
            if arg.starts_with('-') {
                continue;
            }
            let name = if arg.starts_with('@') {
                arg.to_ascii_lowercase()
            } else {
                arg.split(['=', '<', '>', '[', '@', '~', '!'])
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
            };
            let valid = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./@".contains(c));
            if valid && !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts() -> Reach {
        Reach::Hosts(vec!["pypi.org".into()])
    }
    use serde_json::json;

    fn all() -> Rules {
        Rules::default()
    }

    #[test]
    fn rules_parse_from_the_setting() {
        assert_eq!(Rules::parse("on").unwrap(), Rules::default());
        assert!(Rules::parse("off").unwrap().is_empty());
        let some = Rules::parse("email, downloads").unwrap();
        assert!(some.has(Rule::Email) && some.has(Rule::Downloads) && !some.has(Rule::Network));
        assert!(Rules::parse("emails").is_err());
    }

    #[test]
    fn downloads_are_scoped_to_their_host() {
        let gate = check(
            &all(),
            "download_file",
            &json!({ "url": "https://www.example.com/r/x.pdf", "path": "attachments/x.pdf" }),
            &Reach::Nothing,
        )
        .unwrap();
        assert_eq!(gate.rule, Rule::Downloads);
        assert_eq!(gate.scope.as_deref(), Some("example.com"));
        assert_eq!(gate.grants, ["example.com"]);
        assert_eq!(
            gate.reason,
            "Save attachments/x.pdf from example.com to the workspace."
        );
    }

    #[test]
    fn sending_mail_asks_every_time_but_reading_does_not() {
        let gate = check(
            &all(),
            "gmail_send_message",
            &json!({ "to": ["a@x.org"], "subject": "Q3 numbers" }),
            &Reach::Nothing,
        )
        .unwrap();
        assert_eq!(gate.rule, Rule::Email);
        assert_eq!(gate.scope, None);
        assert_eq!(
            gate.reason,
            "Send message in your Gmail to a@x.org: \"Q3 numbers\"."
        );
        assert!(check(&all(), "gmail_search_threads", &json!({}), &Reach::Nothing).is_none());
        assert!(check(&all(), "calendar_create_event", &json!({}), &Reach::Nothing).is_none());
    }

    #[test]
    fn deletes_are_caught_as_commands_only() {
        let rm = |c: &str| check(&all(), "bash", &json!({ "command": c }), &Reach::Nothing);
        assert_eq!(rm("rm -rf out/").unwrap().rule, Rule::Deletes);
        assert_eq!(rm("cd x && rm a.txt").unwrap().rule, Rule::Deletes);
        assert_eq!(
            rm("find . -name '*.tmp' -delete").unwrap().rule,
            Rule::Deletes
        );
        assert!(rm("grep rm notes.txt").is_none());
        assert!(rm("ls -la").is_none());
    }

    #[test]
    fn installs_need_a_network_and_name_their_packages() {
        let input = json!({ "command": "pip install -q pandas==2.2 'numpy>=1' && python a.py" });
        assert!(check(&all(), "bash", &input, &Reach::Nothing).is_none());
        let gate = check(&all(), "bash", &input, &hosts()).unwrap();
        assert_eq!(gate.rule, Rule::Installs);
        assert_eq!(gate.grants, ["pandas", "numpy"]);
        assert_eq!(gate.scope.as_deref(), Some("pandas, numpy"));
        assert_eq!(
            installs("npm i -D typescript @types/node"),
            ["typescript", "@types/node"]
        );
        assert_eq!(installs("uv pip install requests"), ["requests"]);
        assert_eq!(installs("sudo apt-get install -y jq"), ["jq"]);
        assert_eq!(
            installs("import os\nos.system('pip install rich')"),
            ["rich"]
        );
        assert!(installs("pip list").is_empty());
    }

    #[test]
    fn on_openshell_code_runs_without_asking_and_a_blocked_connection_asks_afterwards() {
        // The policy decides where code can go, so running it needs no OK of its own.
        let input = json!({ "code": "print(1)" });
        assert!(check(&all(), "run_python", &input, &hosts()).is_none());
        assert!(check(&all(), "run_python", &input, &Reach::Nothing).is_none());

        let blocked = Endpoint {
            binary: "/usr/bin/python3.11".into(),
            host: "api.example.com".into(),
            port: 443,
        };
        let gate = check_connection(&all(), &blocked).unwrap();
        assert_eq!(gate.rule, Rule::Network);
        assert_eq!(gate.grants, ["/usr/bin/python3.11 api.example.com:443"]);
        assert_eq!(
            gate.scope.as_deref(),
            Some("python3.11 → api.example.com:443")
        );
        assert!(gate
            .reason
            .contains("python3.11 connect to api.example.com:443"));
        let rules = Rules::parse("email,downloads,installs,deletes").unwrap();
        assert!(check_connection(&rules, &blocked).is_none());
    }

    #[test]
    fn on_docker_code_with_a_network_asks_unless_that_rule_is_off() {
        let input = json!({ "code": "print(1)" });
        let bridge = Reach::Network("bridge".into());
        let gate = check(&all(), "run_python", &input, &bridge).unwrap();
        assert_eq!(gate.rule, Rule::Network);
        assert_eq!(gate.grants, ["sandbox"]);
        assert!(check(&all(), "run_python", &input, &Reach::Nothing).is_none());
        let rules = Rules::parse("email,downloads,installs,deletes").unwrap();
        assert!(check(&rules, "run_python", &input, &bridge).is_none());
    }

    #[test]
    fn nothing_is_asked_with_the_gate_off() {
        let off = Rules::none();
        assert!(check(
            &off,
            "download_file",
            &json!({ "url": "https://a.b/c" }),
            &Reach::Nothing
        )
        .is_none());
        assert!(check(&off, "bash", &json!({ "command": "rm x" }), &Reach::Nothing).is_none());
    }

    #[test]
    fn a_community_tool_asks_for_each_release_and_a_local_one_never() {
        let dir = std::env::temp_dir().join(format!("cedar-approve-{}", uuid::Uuid::new_v4()));
        let write = |rel: &str, text: &str| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        let manifest = r#"{"description": "d", "network": ["api.example.com"],
                           "secrets": ["KEY"], "health_data": true}"#;
        write("community/tool.json", manifest);
        write("community/tool.py", "");
        write(
            "community/.cedar-install.json",
            r#"{"version": "2.0.0", "registry": "r", "digest": "d1", "files": {}, "installed_at": 1}"#,
        );
        write("local/tool.json", manifest);
        write("local/tool.py", "");
        let load = |name: &str| {
            crate::tools::custom::load_folder(&dir.join(name), name)
                .unwrap()
                .unwrap()
        };

        let gate = check_personal(&all(), &load("community")).expect("community tools ask");
        assert_eq!(gate.rule, Rule::Community);
        assert_eq!(
            gate.grants,
            ["community@d1"],
            "always covers this release only"
        );
        assert_eq!(gate.scope.as_deref(), Some("community 2.0.0"));
        assert!(gate.reason.contains("api.example.com"), "{}", gate.reason);
        assert!(gate.reason.contains("KEY"), "{}", gate.reason);
        assert!(gate.reason.contains("health data"), "{}", gate.reason);
        assert!(check_personal(&all(), &load("local")).is_none());
        let rules = Rules::parse("email").unwrap();
        assert!(check_personal(&rules, &load("community")).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
