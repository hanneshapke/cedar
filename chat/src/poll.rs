//! Looking at what a trigger watches: one request to cedar, with no model involved.
//!
//! A web address, a tool and a mail or calendar search go through cedar's
//! `POST /v1/tools/{name}/call`, which is where the address rules, the Google account and the
//! personal tools' sandboxes live. A record set is read from `GET /v1/records/{name}`.

use crate::api::AppState;
use crate::error::AppError;
use crate::trigger::{Observation, Request, Trigger};
use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use serde_json::json;

/// What `trigger` watches, as it is at `now`. The error is a sentence for the agent's page:
/// why the check didn't work.
pub async fn observe(state: &AppState, trigger: &Trigger, now: DateTime<Utc>) -> Result<Observation, String> {
    match trigger.request(now).ok_or("this trigger doesn't watch anything")? {
        Request::Tool { name, input } => {
            let reply = state.cedar.call_tool(&name, &input).await.map_err(|e| why(&name, e))?;
            let output = reply["output"].as_str().unwrap_or_default().to_string();
            if reply["is_error"] == true {
                return Err(output);
            }
            if reply["truncated"] == true {
                return Err(format!("{name} returned more than can be compared; narrow what it is asked for"));
            }
            Ok(match reply.get("json").filter(|j| !j.is_null()) {
                Some(json) => Observation { text: output, json: Some(json.clone()) },
                None => Observation { text: output, json: None },
            })
        }
        Request::Records { set } => {
            let reply = state.cedar.record_set(&set).await.map_err(|e| match e {
                AppError::Upstream { status: StatusCode::NOT_FOUND, .. } => format!("there is no record set {set:?}"),
                other => other.to_string(),
            })?;
            let records: Vec<_> = reply["records"].as_array().map(|rows| rows.iter().map(|r| r["data"].clone()).collect()).unwrap_or_default();
            Ok(Observation::of_json(json!({ "count": records.len(), "updated_at": reply["set"]["updated_at"], "records": records })))
        }
    }
}

fn why(name: &str, e: AppError) -> String {
    match e {
        AppError::Upstream { status: StatusCode::NOT_FOUND, .. } if name == "url" => {
            "cedar can't read web addresses right now (WEB_FETCH is off, or cedar is older than this feature)".to_string()
        }
        AppError::Upstream { status: StatusCode::NOT_FOUND, .. } if name.starts_with("gmail_") || name.starts_with("calendar_") => {
            format!("cedar doesn't offer {name}: connect Google under Settings → Integrations (and use the built-in Google backend, not GOOGLE_BACKEND=mcp)")
        }
        AppError::Upstream { status: StatusCode::NOT_FOUND, .. } => format!("cedar doesn't offer a tool called {name} right now"),
        other => other.to_string(),
    }
}
