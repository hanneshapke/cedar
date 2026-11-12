//! Slack through its Web API: the channels the app can see, the messages in one (or in a thread),
//! the people in the workspace, and posting a message. Like the Google tools they run in the cedar
//! process: the token stays here, and the sandbox has no network.
//!
//! The token is a Slack app's bot token (`xoxb-...`), set in the environment or saved by an admin.
//! The app reads only the channels it was invited to, and posts as itself. It needs the scopes
//! `channels:read`, `channels:history`, `chat:write` and `users:read`; `groups:read` and
//! `groups:history` add private channels, `users:read.email` people's addresses.

use super::{truncate_output, Tool, ToolContext};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use reqwest::StatusCode;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The tools implemented here, in the order they are offered.
const TOOLS: &[&str] = &[
    "list_channels",
    "read_messages",
    "list_users",
    "send_message",
];
/// The prefix of every tool's name, which is also their group.
pub const PREFIX: &str = "slack";
pub const SEND_MESSAGE: &str = "slack_send_message";

const DEFAULT_LIST: usize = 50;
const MAX_LIST: usize = 200;
const DEFAULT_MESSAGES: u64 = 30;
const MAX_MESSAGES: u64 = 100;
/// What one listing request asks Slack for, and how many requests a listing or a name lookup
/// makes at most.
const PAGE_SIZE: usize = 200;
const MAX_PAGES: usize = 10;
/// Slack cuts a message's text at 40,000 characters and asks for under 4,000.
const MAX_TEXT_CHARS: usize = 4000;
/// At most this many people are looked up by ID for one result; the rest stay IDs.
const MAX_USER_LOOKUPS: usize = 25;
const MAX_FIELD_CHARS: usize = 300;
const USER_AGENT: &str = concat!(
    "cedar/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/hanneshapke/cedar)"
);

/// A Slack workspace as one token sees it.
pub struct SlackApi {
    http: reqwest::Client,
    token: String,
    base_url: String,
    /// Names of the people looked up so far, by user ID.
    users: Mutex<HashMap<String, String>>,
}

impl SlackApi {
    /// `base_url` is the Web API's root (see `DEFAULT_SLACK_API_URL`): a method `chat.postMessage`
    /// is called at `{base_url}/chat.postMessage`.
    pub fn new(token: String, base_url: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent(USER_AGENT)
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            http,
            token,
            base_url: base_url.trim_end_matches('/').to_string(),
            users: Mutex::new(HashMap::new()),
        })
    }

    /// Check the token with `auth.test`, and say whose it is: `{"team", "user"}`.
    pub async fn check_token(&self, timeout: Duration) -> Result<Value> {
        let who = self.call("auth.test", &[], timeout).await?;
        Ok(json!({ "team": who["team"], "user": who["user"] }))
    }

    /// Call one Web API method with form parameters and return its JSON, turning Slack's
    /// `{"ok": false, "error"}` into an error the model (and the user) can act on.
    async fn call(
        &self,
        method: &str,
        params: &[(&str, String)],
        timeout: Duration,
    ) -> Result<Value> {
        let resp = self
            .http
            .post(format!("{}/{method}", self.base_url))
            .bearer_auth(&self.token)
            .header("Accept", "application/json")
            .form(params)
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| anyhow!("calling Slack's {method}: {}", e.without_url()))?;
        let status = resp.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            let wait = resp
                .headers()
                .get("Retry-After")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok());
            match wait {
                Some(s) => bail!("Slack is rate limiting {method}; try again in {s}s"),
                None => bail!("Slack is rate limiting {method}; try again later"),
            }
        }
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if body["ok"] == true {
            return Ok(body);
        }
        let Some(error) = body["error"].as_str() else {
            bail!("Slack error {status} from {method}");
        };
        match error {
            "invalid_auth" | "not_authed" | "token_revoked" | "token_expired"
            | "account_inactive" => {
                bail!("Slack rejected the token ({error}); an admin should check it")
            }
            "missing_scope" => bail!(
                "the Slack app lacks a scope {method} needs ({}); an admin should add it and \
                 reinstall the app",
                body["needed"].as_str().unwrap_or("see Slack's docs")
            ),
            "not_in_channel" => bail!(
                "the Slack app is not a member of that channel; someone has to invite it there \
                 first (/invite @app)"
            ),
            "channel_not_found" => bail!(
                "Slack knows no such channel, or the app can't see it; find channels with \
                 {PREFIX}_list_channels"
            ),
            "is_archived" => bail!("that Slack channel is archived"),
            "thread_not_found" => bail!("Slack knows no such thread in that channel"),
            "user_not_found" | "users_not_found" => bail!("Slack knows no such user"),
            "msg_too_long" => bail!("the message is too long for Slack"),
            other => bail!("Slack's {method} failed: {}", clip(other, MAX_FIELD_CHARS)),
        }
    }

    /// The channels the token can see, a page at a time, until `keep` says stop or the pages run
    /// out. Returns whether there were more. Private channels are left out when the app lacks
    /// the scope for them.
    async fn each_channel(
        &self,
        timeout: Duration,
        mut keep: impl FnMut(&Value) -> bool,
    ) -> Result<bool> {
        let mut types = "public_channel,private_channel";
        let mut cursor = String::new();
        for _ in 0..MAX_PAGES {
            let params = [
                ("types", types.to_string()),
                ("exclude_archived", "true".to_string()),
                ("limit", PAGE_SIZE.to_string()),
                ("cursor", cursor.clone()),
            ];
            let page = match self.call("conversations.list", &params, timeout).await {
                Err(e) if types.contains(',') && e.to_string().contains("lacks a scope") => {
                    types = "public_channel";
                    cursor.clear();
                    self.call(
                        "conversations.list",
                        &[
                            ("types", types.to_string()),
                            ("exclude_archived", "true".to_string()),
                            ("limit", PAGE_SIZE.to_string()),
                        ],
                        timeout,
                    )
                    .await?
                }
                other => other?,
            };
            let next = next_cursor(&page);
            for channel in page["channels"].as_array().into_iter().flatten() {
                if !keep(channel) {
                    return Ok(true);
                }
            }
            match next {
                Some(c) => cursor = c,
                None => return Ok(false),
            }
        }
        Ok(true)
    }

    /// The ID Slack's methods take for `channel`: an ID as it is, a name (with or without `#`)
    /// looked up among the channels the app can see.
    async fn channel_id(&self, channel: &str, timeout: Duration) -> Result<String> {
        let channel = channel.trim();
        if is_id(channel, "CGD") {
            return Ok(channel.to_string());
        }
        let name = channel.trim_start_matches('#').to_lowercase();
        if name.is_empty() {
            bail!("channel is empty");
        }
        let mut found = None;
        self.each_channel(timeout, |c| {
            if c["name"].as_str() == Some(name.as_str()) {
                found = c["id"].as_str().map(String::from);
            }
            found.is_none()
        })
        .await?;
        found.ok_or_else(|| {
            anyhow!(
                "no Slack channel named #{name} that the app can see; find channels with \
                 {PREFIX}_list_channels"
            )
        })
    }

    /// The display names of `ids`, from the cache or `users.info`. People Slack won't tell us
    /// about (a missing scope, a deleted account) are left out, and so shown by ID.
    async fn names(&self, ids: &[String], timeout: Duration) -> HashMap<String, String> {
        let mut names = HashMap::new();
        let mut lookups = 0;
        for id in ids {
            if let Some(name) = self.users.lock().unwrap().get(id) {
                names.insert(id.clone(), name.clone());
                continue;
            }
            if lookups == MAX_USER_LOOKUPS {
                continue;
            }
            lookups += 1;
            match self
                .call("users.info", &[("user", id.clone())], timeout)
                .await
            {
                Ok(body) => {
                    let name = user_name(&body["user"]);
                    self.users.lock().unwrap().insert(id.clone(), name.clone());
                    names.insert(id.clone(), name);
                }
                // Without the scope no lookup will work, so don't make the rest.
                Err(e) if e.to_string().contains("lacks a scope") => break,
                Err(_) => {}
            }
        }
        names
    }

    async fn list_channels(&self, input: &Value, timeout: Duration) -> Result<Value> {
        let query = str_arg(input, "query").map(|q| q.trim_start_matches('#').to_lowercase());
        let member_only = input["member_only"].as_bool().unwrap_or(false);
        let limit = limit_arg(input);
        let mut channels = Vec::new();
        let more = self
            .each_channel(timeout, |c| {
                let matches = query.as_deref().is_none_or(|q| {
                    ["name", "topic", "purpose"].iter().any(|k| {
                        let text = c[k].as_str().or(c[k]["value"].as_str()).unwrap_or_default();
                        text.to_lowercase().contains(q)
                    })
                });
                if !matches || (member_only && c["is_member"] != true) {
                    return true;
                }
                if channels.len() == limit {
                    return false;
                }
                channels.push(without_empty(json!({
                    "id": c["id"],
                    "name": c["name"],
                    "private": c["is_private"].as_bool().filter(|p| *p),
                    "member": c["is_member"],
                    "members": c["num_members"],
                    "topic": c["topic"]["value"].as_str().map(|s| clip(s, MAX_FIELD_CHARS)),
                    "purpose": c["purpose"]["value"].as_str().map(|s| clip(s, MAX_FIELD_CHARS)),
                })));
                true
            })
            .await?;
        let mut out = json!({ "channels": channels });
        if more {
            out["more"] = json!("there are more channels; narrow the list with query");
        }
        Ok(out)
    }

    async fn list_users(&self, input: &Value, timeout: Duration) -> Result<Value> {
        let query = str_arg(input, "query").map(|q| q.trim_start_matches('@').to_lowercase());
        let limit = limit_arg(input);
        let mut users = Vec::new();
        let mut more = false;
        let mut cursor = String::new();
        'pages: for page_no in 0..MAX_PAGES {
            let params = [("limit", PAGE_SIZE.to_string()), ("cursor", cursor.clone())];
            let page = self.call("users.list", &params, timeout).await?;
            for u in page["members"].as_array().into_iter().flatten() {
                if u["deleted"] == true {
                    continue;
                }
                let profile = &u["profile"];
                let fields = [
                    &u["name"],
                    &u["real_name"],
                    &profile["display_name"],
                    &profile["real_name"],
                    &profile["email"],
                    &profile["title"],
                ];
                let matches = query.as_deref().is_none_or(|q| {
                    fields
                        .iter()
                        .filter_map(|f| f.as_str())
                        .any(|f| f.to_lowercase().contains(q))
                });
                if !matches {
                    continue;
                }
                if users.len() == limit {
                    more = true;
                    break 'pages;
                }
                users.push(without_empty(json!({
                    "id": u["id"],
                    "name": u["name"],
                    "real_name": u["real_name"].as_str().or(profile["real_name"].as_str()),
                    "display_name": profile["display_name"],
                    "title": profile["title"].as_str().map(|s| clip(s, MAX_FIELD_CHARS)),
                    "email": profile["email"],
                    "bot": u["is_bot"].as_bool().filter(|b| *b),
                    "timezone": u["tz"],
                })));
            }
            match next_cursor(&page) {
                Some(c) => cursor = c,
                None => break,
            }
            more = page_no + 1 == MAX_PAGES;
        }
        let mut out = json!({ "users": users });
        if more {
            out["more"] = json!("there are more people; narrow the list with query");
        }
        Ok(out)
    }

    async fn read_messages(&self, input: &Value, timeout: Duration) -> Result<Value> {
        let channel = self
            .channel_id(required_str(input, "channel")?, timeout)
            .await?;
        let limit = input["limit"]
            .as_u64()
            .unwrap_or(DEFAULT_MESSAGES)
            .clamp(1, MAX_MESSAGES);
        let mut params = vec![("channel", channel.clone()), ("limit", limit.to_string())];
        for key in ["oldest", "latest"] {
            if let Some(when) = str_arg(input, key) {
                params.push((
                    key,
                    timestamp(when).with_context(|| format!("{key} {when:?}"))?,
                ));
            }
        }
        if let Some(cursor) = str_arg(input, "cursor") {
            params.push(("cursor", cursor.to_string()));
        }
        let thread = str_arg(input, "thread_ts");
        let method = match thread {
            Some(ts) => {
                params.push(("ts", message_ts(ts)?.to_string()));
                "conversations.replies"
            }
            None => "conversations.history",
        };
        let page = self.call(method, &params, timeout).await?;
        let mut messages: Vec<&Value> = page["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| {
                !matches!(
                    m["subtype"].as_str(),
                    Some("channel_join" | "channel_leave")
                )
            })
            .collect();
        // A channel's history comes newest first, a thread oldest first; show both as they read.
        if thread.is_none() {
            messages.reverse();
        }

        let mut ids: Vec<String> = Vec::new();
        for m in &messages {
            let mentioned = m["text"].as_str().map(mentions).unwrap_or_default();
            for id in m["user"]
                .as_str()
                .map(String::from)
                .into_iter()
                .chain(mentioned)
            {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        let names = self.names(&ids, timeout).await;

        let messages: Vec<Value> = messages
            .iter()
            .map(|m| render_message(m, &names, thread.is_some()))
            .collect();
        let mut out = json!({ "channel": channel, "messages": messages });
        if let Some(ts) = thread {
            out["thread_ts"] = json!(ts);
        }
        if let Some(cursor) = next_cursor(&page) {
            out["next_cursor"] = json!(cursor);
            out["more"] = json!(if thread.is_some() {
                "later replies follow; pass next_cursor as cursor"
            } else {
                "earlier messages exist; pass next_cursor as cursor"
            });
        }
        Ok(out)
    }

    async fn send_message(&self, input: &Value, timeout: Duration) -> Result<Value> {
        let target = required_str(input, "channel")?.trim();
        let text = required_str(input, "text")?.trim();
        let chars = text.chars().count();
        if chars > MAX_TEXT_CHARS {
            bail!("text is {chars} characters; Slack messages should stay under {MAX_TEXT_CHARS}");
        }
        // A user ID as the channel opens (or continues) the app's direct message with them.
        let channel = if is_id(target, "UW") {
            target.to_string()
        } else {
            self.channel_id(target, timeout).await?
        };
        let mut params = vec![("channel", channel), ("text", text.to_string())];
        if let Some(ts) = str_arg(input, "thread_ts") {
            params.push(("thread_ts", message_ts(ts)?.to_string()));
        }
        let sent = self.call("chat.postMessage", &params, timeout).await?;
        Ok(without_empty(json!({
            "sent": true,
            "channel": sent["channel"],
            "ts": sent["ts"],
            "thread_ts": sent["message"]["thread_ts"],
        })))
    }
}

/// The Slack tools, sharing one API client.
pub fn tools(token: String, base_url: String) -> Result<Vec<Box<dyn Tool>>> {
    let api = Arc::new(SlackApi::new(token, base_url)?);
    Ok(TOOLS
        .iter()
        .map(|op| {
            Box::new(SlackTool {
                name: format!("{PREFIX}_{op}"),
                op,
                api: api.clone(),
            }) as Box<dyn Tool>
        })
        .collect())
}

/// One Slack operation; `op` is one of [`TOOLS`].
pub struct SlackTool {
    name: String,
    op: &'static str,
    api: Arc<SlackApi>,
}

#[async_trait]
impl Tool for SlackTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        match self.op {
            "list_channels" => {
                "List the channels of the team's Slack workspace that the Slack app can see, with \
                 their IDs, topics and whether the app is a member (it can only read and post in \
                 channels it was invited to). Narrow the list with query, which matches names, \
                 topics and purposes."
            }
            "read_messages" => {
                "Read the messages of a Slack channel, oldest first, each with who wrote it, \
                 when, and its ts. By default the latest messages; oldest and latest pick a \
                 period. A message with replies has a reply count: pass its ts as thread_ts to \
                 read that thread. The app must be a member of the channel."
            }
            "list_users" => {
                "List the people in the team's Slack workspace with their IDs, names and titles. \
                 Narrow the list with query, which matches names, emails and titles. A user's ID \
                 is what slack_send_message takes to message them directly."
            }
            "send_message" => {
                "Post a message to a Slack channel, a thread, or one person, as the Slack app. \
                 Other people will read it: send only what the user asked for, and say what you \
                 sent. Text is Slack's mrkdwn: *bold*, _italic_, `code`, <https://example.com|a \
                 link>, <@USERID> to mention someone."
            }
            _ => unreachable!("not a Slack tool: {}", self.op),
        }
    }

    fn schema(&self) -> Value {
        let limit = |what: &str| {
            json!({ "type": "integer", "minimum": 1, "maximum": MAX_LIST,
                "description": format!("How many {what} to return (default {DEFAULT_LIST}, at most {MAX_LIST}).") })
        };
        match self.op {
            "list_channels" => json!({ "type": "object", "properties": {
                "query": { "type": "string",
                    "description": "Only channels whose name, topic or purpose contains this." },
                "member_only": { "type": "boolean",
                    "description": "Only channels the Slack app is a member of (default false)." },
                "limit": limit("channels"),
            } }),
            "read_messages" => json!({ "type": "object", "properties": {
                "channel": { "type": "string",
                    "description": "The channel's name (e.g. #general) or ID (e.g. C0123456789)." },
                "thread_ts": { "type": "string",
                    "description": "The ts of a thread's first message, to read that thread instead of the channel." },
                "oldest": { "type": "string",
                    "description": "Only messages after this: a date (2026-03-01), a time (2026-03-01T09:00:00Z) or a message's ts." },
                "latest": { "type": "string",
                    "description": "Only messages before this, given like oldest." },
                "limit": { "type": "integer", "minimum": 1, "maximum": MAX_MESSAGES,
                    "description": format!("How many messages to return (default {DEFAULT_MESSAGES}, at most {MAX_MESSAGES}).") },
                "cursor": { "type": "string",
                    "description": "next_cursor from the previous result, to read on." },
            }, "required": ["channel"] }),
            "list_users" => json!({ "type": "object", "properties": {
                "query": { "type": "string",
                    "description": "Only people whose name, email or title contains this." },
                "limit": limit("people"),
            } }),
            "send_message" => json!({ "type": "object", "properties": {
                "channel": { "type": "string",
                    "description": "Where to post: a channel's name (e.g. #general) or ID, or a user's ID (e.g. U0123456789, from slack_list_users) for a direct message." },
                "text": { "type": "string", "maxLength": MAX_TEXT_CHARS,
                    "description": "The message, in Slack's mrkdwn." },
                "thread_ts": { "type": "string",
                    "description": "The ts of a message to reply to in its thread." },
            }, "required": ["channel", "text"] }),
            _ => unreachable!("not a Slack tool: {}", self.op),
        }
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let t = ctx.timeout;
        let out = match self.op {
            "list_channels" => self.api.list_channels(input, t).await?,
            "read_messages" => self.api.read_messages(input, t).await?,
            "list_users" => self.api.list_users(input, t).await?,
            "send_message" => self.api.send_message(input, t).await?,
            _ => unreachable!("not a Slack tool: {}", self.op),
        };
        Ok(truncate_output(
            &serde_json::to_string(&out)?,
            ctx.max_output,
        ))
    }
}

/// One message as the model sees it. In a thread the replies don't repeat the thread's ts.
fn render_message(m: &Value, names: &HashMap<String, String>, in_thread: bool) -> Value {
    let author = match m["user"].as_str() {
        Some(id) => names.get(id).cloned().unwrap_or_else(|| id.to_string()),
        None => m["username"]
            .as_str()
            .or(m["bot_profile"]["name"].as_str())
            .unwrap_or("unknown")
            .to_string(),
    };
    let ts = m["ts"].as_str().unwrap_or_default();
    let files: Vec<&str> = m["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["name"].as_str().or(f["title"].as_str()))
        .collect();
    let reactions: Vec<String> = m["reactions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| Some(format!("{} x{}", r["name"].as_str()?, r["count"].as_u64()?)))
        .collect();
    without_empty(json!({
        "ts": ts,
        "time": time_of(ts),
        "from": author,
        "user_id": m["user"],
        "bot": m["bot_id"].as_str().map(|_| true),
        "text": render_text(m["text"].as_str().unwrap_or_default(), names),
        "thread_ts": m["thread_ts"].as_str().filter(|t| !in_thread && *t != ts),
        "replies": m["reply_count"].as_u64().filter(|n| !in_thread && *n > 0),
        "edited": m["edited"]["ts"].as_str().map(|_| true),
        "files": files,
        "reactions": reactions,
    }))
}

/// Slack's message markup as plain text: `<@U123>` becomes `@Name`, `<#C123|general>` `#general`,
/// `<https://x|label>` `label (https://x)`, and the three escaped characters are restored.
fn render_text(text: &str, names: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        // A `<` that is never closed is not markup: the rest goes out as it is.
        let Some(len) = rest[open..].find('>') else {
            break;
        };
        out.push_str(&rest[..open]);
        let inner = &rest[open + 1..open + len];
        let (target, label) = match inner.split_once('|') {
            Some((t, l)) => (t, Some(l)),
            None => (inner, None),
        };
        if let Some(id) = target.strip_prefix('@') {
            let name = names.get(id).map(String::as_str).or(label).unwrap_or(id);
            out.push('@');
            out.push_str(name);
        } else if let Some(id) = target.strip_prefix('#') {
            out.push('#');
            out.push_str(label.unwrap_or(id));
        } else if let Some(special) = target.strip_prefix('!') {
            let name = label.unwrap_or(special.split('^').next().unwrap_or(special));
            out.push('@');
            out.push_str(name.trim_start_matches('@'));
        } else {
            match label {
                Some(l) if l != target => out.push_str(&format!("{l} ({target})")),
                _ => out.push_str(target),
            }
        }
        rest = &rest[open + len + 1..];
    }
    out.push_str(rest);
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The user IDs mentioned in a message's text.
fn mentions(text: &str) -> Vec<String> {
    text.split("<@")
        .skip(1)
        .filter_map(|rest| {
            let id = rest.split(['>', '|']).next()?;
            is_id(id, "UW").then(|| id.to_string())
        })
        .collect()
}

/// What to call a person: their display name, else their real name, else their handle.
fn user_name(user: &Value) -> String {
    [
        &user["profile"]["display_name"],
        &user["real_name"],
        &user["profile"]["real_name"],
        &user["name"],
        &user["id"],
    ]
    .iter()
    .filter_map(|v| v.as_str())
    .find(|s| !s.trim().is_empty())
    .unwrap_or("unknown")
    .to_string()
}

/// Whether `s` looks like a Slack ID whose first letter is one of `kinds` (C, G and D for
/// conversations, U and W for people).
fn is_id(s: &str, kinds: &str) -> bool {
    s.len() >= 9
        && s.len() <= 20
        && s.starts_with(|c| kinds.contains(c))
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// A message's ts ("1700000000.000200") as the model passed it.
fn message_ts(ts: &str) -> Result<&str> {
    let ts = ts.trim();
    let ok = ts.split_once('.').is_some_and(|(secs, frac)| {
        !secs.is_empty()
            && !frac.is_empty()
            && secs.len() <= 12
            && frac.len() <= 9
            && secs.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit())
    });
    if !ok {
        bail!("{ts:?} is not a message ts (like 1700000000.000200)");
    }
    Ok(ts)
}

/// A point in time as Slack takes it (seconds since 1970), from a ts, a date, or an RFC 3339 time.
fn timestamp(when: &str) -> Result<String> {
    let when = when.trim();
    if message_ts(when).is_ok() {
        return Ok(when.to_string());
    }
    if !when.is_empty() && when.len() <= 12 && when.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(when.to_string());
    }
    if let Ok(time) = DateTime::parse_from_rfc3339(when) {
        return Ok(time.timestamp().to_string());
    }
    if let Ok(date) = NaiveDate::parse_from_str(when, "%Y-%m-%d") {
        let midnight = date
            .and_hms_opt(0, 0, 0)
            .expect("midnight exists")
            .and_utc();
        return Ok(midnight.timestamp().to_string());
    }
    bail!("is not a date (2026-03-01), a time (2026-03-01T09:00:00Z) or a message ts")
}

/// When a message with this ts was sent, in UTC.
fn time_of(ts: &str) -> Option<String> {
    let secs: i64 = ts.split('.').next()?.parse().ok()?;
    let time: DateTime<Utc> = DateTime::from_timestamp(secs, 0)?;
    Some(time.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// The cursor of the next page, if Slack says there is one.
fn next_cursor(page: &Value) -> Option<String> {
    page["response_metadata"]["next_cursor"]
        .as_str()
        .filter(|c| !c.is_empty())
        .map(String::from)
}

fn str_arg<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input[key].as_str().filter(|s| !s.trim().is_empty())
}

fn required_str<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    str_arg(input, key).ok_or_else(|| anyhow!("missing {key:?}"))
}

fn limit_arg(input: &Value) -> usize {
    input["limit"]
        .as_u64()
        .map_or(DEFAULT_LIST, |n| n as usize)
        .clamp(1, MAX_LIST)
}

/// `s` cut to `max` characters.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// An object without its null, empty-string and empty-array fields.
fn without_empty(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(_, v)| match v {
                    Value::Null => false,
                    Value::String(s) => !s.is_empty(),
                    Value::Array(a) => !a.is_empty(),
                    _ => true,
                })
                .collect::<Map<_, _>>(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> HashMap<String, String> {
        HashMap::from([("U012ABCDEF".to_string(), "Ana".to_string())])
    }

    #[test]
    fn slack_markup_reads_as_plain_text() {
        assert_eq!(
            render_text(
                "hi <@U012ABCDEF> and <@U099ZZZZZZ>, see <#C012ABCDEF|general> &amp; \
                 <https://example.com/a?b=1|the doc> or <https://example.com> <!here> \
                 <!subteam^S012ABCDEF|@eng> 1 &lt; 2",
                &names()
            ),
            "hi @Ana and @U099ZZZZZZ, see #general & the doc (https://example.com/a?b=1) or \
             https://example.com @here @eng 1 < 2"
        );
        assert_eq!(
            render_text("a < b and <unclosed", &names()),
            "a < b and <unclosed"
        );
        assert_eq!(
            mentions("<@U012ABCDEF> <@U099ZZZZZZ|zed> <@nope> <#C012ABCDEF>"),
            ["U012ABCDEF", "U099ZZZZZZ"]
        );
    }

    #[test]
    fn ids_and_timestamps_are_told_apart_from_names_and_dates() {
        assert!(is_id("C012ABCDEF", "CGD"));
        assert!(!is_id("U012ABCDEF", "CGD"));
        assert!(is_id("U012ABCDEF", "UW"));
        assert!(!is_id("general", "CGD"));
        assert!(!is_id("Customers", "CGD"));
        assert!(!is_id("C012", "CGD"));

        assert_eq!(
            message_ts(" 1700000000.000200 ").unwrap(),
            "1700000000.000200"
        );
        assert!(message_ts("1700000000").is_err());
        assert!(message_ts("1700000000.00&channel=x").is_err());

        assert_eq!(timestamp("1700000000.000200").unwrap(), "1700000000.000200");
        assert_eq!(timestamp("1700000000").unwrap(), "1700000000");
        assert_eq!(timestamp("2023-11-14").unwrap(), "1699920000");
        assert_eq!(timestamp("2023-11-14T22:13:20Z").unwrap(), "1700000000");
        assert_eq!(
            timestamp("2023-11-14T23:13:20+01:00").unwrap(),
            "1700000000"
        );
        assert!(timestamp("last week").is_err());
        assert_eq!(
            time_of("1700000000.000200").as_deref(),
            Some("2023-11-14T22:13:20Z")
        );
        assert_eq!(time_of("soon"), None);
    }

    #[test]
    fn a_message_keeps_what_matters() {
        let m = json!({
            "type": "message", "user": "U012ABCDEF", "ts": "1700000000.000200",
            "text": "ship it <@U012ABCDEF>", "thread_ts": "1700000000.000200", "reply_count": 2,
            "files": [{ "name": "plan.pdf" }], "reactions": [{ "name": "tada", "count": 3 }],
            "edited": { "user": "U012ABCDEF", "ts": "1700000050.000000" }, "blocks": [{}]
        });
        assert_eq!(
            render_message(&m, &names(), false),
            json!({ "ts": "1700000000.000200", "time": "2023-11-14T22:13:20Z", "from": "Ana",
                    "user_id": "U012ABCDEF", "text": "ship it @Ana", "replies": 2, "edited": true,
                    "files": ["plan.pdf"], "reactions": ["tada x3"] })
        );
        let bot = json!({ "bot_id": "B01", "username": "deploys", "ts": "1700000001.000000",
                          "text": "done", "thread_ts": "1700000000.000200" });
        let reply = render_message(&bot, &names(), false);
        assert_eq!(reply["from"], "deploys");
        assert_eq!(reply["bot"], true);
        assert_eq!(reply["thread_ts"], "1700000000.000200");
        assert!(render_message(&bot, &names(), true)["thread_ts"].is_null());
    }

    #[test]
    fn people_are_called_by_their_display_name() {
        let user = json!({ "id": "U1", "name": "ana.b", "real_name": "Ana Bell",
                           "profile": { "display_name": "" } });
        assert_eq!(user_name(&user), "Ana Bell");
        assert_eq!(
            user_name(&json!({ "profile": { "display_name": "ana" } })),
            "ana"
        );
        assert_eq!(user_name(&json!({})), "unknown");
    }
}
