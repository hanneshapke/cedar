//! Google Sheets through the REST API: the tabs of a spreadsheet and the values in a range. Like
//! the other Google tools they run in the cedar process: the OAuth tokens stay here, and the
//! sandbox has no network.
//!
//! Only reading is implemented; nothing here writes a cell.

use super::google::{required_str, select, str_arg, without_empty, GoogleApi};
use super::{truncate_output, Tool, ToolContext};
use crate::config::GoogleApiConfig;
use crate::mcp::GoogleOAuth;
use anyhow::{bail, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// The tools implemented here, in the order they are offered.
const TOOLS: &[&str] = &["get_spreadsheet", "read_range"];
/// How many rows `read_range` returns unless asked otherwise, and at most.
const DEFAULT_MAX_ROWS: u64 = 100;
const MAX_ROWS: u64 = 1000;
/// What `get_spreadsheet` asks for.
const SPREADSHEET_FIELDS: &str = "spreadsheetId,spreadsheetUrl,properties(title,locale,timeZone),sheets(properties(sheetId,title,index,hidden,gridProperties(rowCount,columnCount)))";

/// The Sheets tools `cfg` allows, sharing one API client.
pub fn tools(cfg: &GoogleApiConfig, auth: Arc<GoogleOAuth>) -> Result<Vec<Box<dyn Tool>>> {
    let api = Arc::new(GoogleApi::new(&cfg.url, auth, "Sheets")?);
    Ok(select(&cfg.prefix, &cfg.tools, TOOLS)
        .into_iter()
        .map(|op| {
            Box::new(SheetsTool {
                name: format!("{}_{op}", cfg.prefix),
                op,
                api: api.clone(),
            }) as Box<dyn Tool>
        })
        .collect())
}

/// One Sheets operation; `op` is one of [`TOOLS`].
pub struct SheetsTool {
    name: String,
    op: &'static str,
    api: Arc<GoogleApi>,
}

#[async_trait]
impl Tool for SheetsTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        match self.op {
            "get_spreadsheet" => {
                "The tabs (sheets) of a Google Sheets spreadsheet with their names and sizes, \
                 and its title, locale and link. Find spreadsheets with drive_search_files \
                 (kind spreadsheet); the file's ID is the spreadsheet ID."
            }
            "read_range" => {
                "Read cells from a Google Sheets spreadsheet as rows: a whole tab (by name) or \
                 an A1 range like 'Budget'!A1:F40. Values come formatted as the sheet shows them \
                 unless valueRender says otherwise. Large tabs come in parts: the result says \
                 where to continue with startRow."
            }
            _ => unreachable!("not a Sheets tool: {}", self.op),
        }
    }

    fn schema(&self) -> Value {
        let id = json!({ "type": "string",
            "description": "The spreadsheet's ID (its Drive file ID, from drive_search_files)." });
        match self.op {
            "get_spreadsheet" => json!({ "type": "object", "properties": {
                "spreadsheetId": id,
            }, "required": ["spreadsheetId"] }),
            "read_range" => json!({ "type": "object", "properties": {
                "spreadsheetId": id,
                "range": { "type": "string",
                    "description": "A tab's name (e.g. Budget), or an A1 range (e.g. Budget!A1:F40, or A1:F40 on the first tab). Default: the first tab." },
                "startRow": { "type": "integer", "minimum": 1,
                    "description": "For a tab given by name: the first row to return (default 1), to read the next part of a long tab." },
                "maxRows": { "type": "integer", "minimum": 1, "maximum": MAX_ROWS,
                    "description": format!("How many rows to return (default {DEFAULT_MAX_ROWS}, at most {MAX_ROWS}).") },
                "valueRender": { "type": "string", "enum": ["formatted", "raw", "formula"],
                    "description": "formatted (default): values as shown in the sheet; raw: unformatted numbers and dates as serial numbers; formula: the formulas." },
            }, "required": ["spreadsheetId"] }),
            _ => unreachable!("not a Sheets tool: {}", self.op),
        }
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let t = ctx.timeout;
        let id = required_str(input, "spreadsheetId")?;
        let out = match self.op {
            "get_spreadsheet" => render_spreadsheet(&self.spreadsheet(id, t).await?),
            "read_range" => self.read_range(id, input, t).await?,
            _ => unreachable!("not a Sheets tool: {}", self.op),
        };
        Ok(truncate_output(
            &serde_json::to_string(&without_empty(out))?,
            ctx.max_output,
        ))
    }
}

impl SheetsTool {
    async fn spreadsheet(&self, id: &str, t: Duration) -> Result<Value> {
        self.api
            .get(&[id], &[("fields", SPREADSHEET_FIELDS.to_string())], t)
            .await
    }

    async fn read_range(&self, id: &str, input: &Value, t: Duration) -> Result<Value> {
        let max_rows = input["maxRows"]
            .as_u64()
            .unwrap_or(DEFAULT_MAX_ROWS)
            .clamp(1, MAX_ROWS);
        let start_row = input["startRow"].as_u64().unwrap_or(1).max(1);
        let render = match str_arg(input, "valueRender").unwrap_or("formatted") {
            "formatted" => "FORMATTED_VALUE",
            "raw" => "UNFORMATTED_VALUE",
            "formula" => "FORMULA",
            other => bail!("unknown valueRender {other:?}"),
        };
        // A tab by name is read in row windows, so a long one pages; an explicit A1 range is
        // read as given.
        let (range, windowed) = match str_arg(input, "range") {
            Some(r) if is_a1_range(r) => (r.to_string(), false),
            Some(tab) => (
                format!(
                    "{}!{start_row}:{}",
                    quote_tab(tab),
                    start_row + max_rows - 1
                ),
                true,
            ),
            None => {
                let spreadsheet = self.spreadsheet(id, t).await?;
                let first = spreadsheet["sheets"]
                    .as_array()
                    .and_then(|s| s.first())
                    .and_then(|s| s["properties"]["title"].as_str())
                    .unwrap_or("Sheet1");
                (
                    format!(
                        "{}!{start_row}:{}",
                        quote_tab(first),
                        start_row + max_rows - 1
                    ),
                    true,
                )
            }
        };
        let values = self
            .api
            .get(
                &[id, "values", &range],
                &[
                    ("majorDimension", "ROWS".to_string()),
                    ("valueRenderOption", render.to_string()),
                    ("dateTimeRenderOption", "FORMATTED_STRING".to_string()),
                ],
                t,
            )
            .await?;
        let mut rows: Vec<Value> = values["values"].as_array().cloned().unwrap_or_default();
        let total = rows.len();
        let truncated = total as u64 > max_rows;
        rows.truncate(max_rows as usize);
        // A window that came back full may have more below it.
        let more = if windowed {
            total as u64 == max_rows
        } else {
            truncated
        };
        Ok(json!({
            "range": values["range"],
            "rows": rows,
            "rowCount": rows.len(),
            "more": more,
            "nextStartRow": (windowed && more).then_some(start_row + max_rows),
            "note": (!windowed && truncated).then_some(format!(
                "Only the first {max_rows} rows of the range are shown; read a smaller range, or the tab by name with startRow."
            )),
        }))
    }
}

/// Whether `s` names cells (`A1:F40`, `Sheet!A1:F40`, `Sheet!3:10`) rather than just a tab.
fn is_a1_range(s: &str) -> bool {
    match s.split_once('!') {
        Some((_, cells)) => !cells.is_empty(),
        None => {
            s.contains(':')
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == ':' || c == '$')
        }
    }
}

/// A tab name in A1 notation: quoted, with quotes doubled.
fn quote_tab(name: &str) -> String {
    format!("'{}'", name.trim_matches('\'').replace('\'', "''"))
}

fn render_spreadsheet(s: &Value) -> Value {
    let sheets: Vec<Value> = s["sheets"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|sheet| {
                    let p = &sheet["properties"];
                    json!({
                        "title": p["title"],
                        "sheetId": p["sheetId"],
                        "index": p["index"],
                        "rows": p["gridProperties"]["rowCount"],
                        "columns": p["gridProperties"]["columnCount"],
                        "hidden": p["hidden"].as_bool().filter(|h| *h),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "spreadsheetId": s["spreadsheetId"],
        "title": s["properties"]["title"],
        "locale": s["properties"]["locale"],
        "timeZone": s["properties"]["timeZone"],
        "link": s["spreadsheetUrl"],
        "sheets": sheets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_and_tabs_are_told_apart() {
        assert!(is_a1_range("A1:F40"));
        assert!(is_a1_range("Budget!A1:F40"));
        assert!(is_a1_range("'Q1 2026'!3:10"));
        assert!(!is_a1_range("Budget"));
        assert!(!is_a1_range("Q1 2026"));
        assert!(!is_a1_range("Budget!"));
    }

    #[test]
    fn tab_names_are_quoted() {
        assert_eq!(quote_tab("Budget"), "'Budget'");
        assert_eq!(quote_tab("Ana's"), "'Ana''s'");
        assert_eq!(quote_tab("'Budget'"), "'Budget'");
    }

    #[test]
    fn spreadsheets_are_rendered_for_the_model() {
        let s = render_spreadsheet(&json!({
            "spreadsheetId": "s1", "spreadsheetUrl": "https://docs.google.com/x",
            "properties": { "title": "Budget", "locale": "en_US", "timeZone": "Europe/Zurich" },
            "sheets": [{ "properties": { "sheetId": 0, "title": "2026", "index": 0,
                "gridProperties": { "rowCount": 1000, "columnCount": 26 } } }],
        }));
        assert_eq!(s["title"], "Budget");
        assert_eq!(s["sheets"][0]["title"], "2026");
        assert_eq!(s["sheets"][0]["rows"], 1000);
        assert!(s["sheets"][0]["hidden"].is_null());
    }
}
