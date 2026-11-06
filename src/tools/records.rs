//! The `records_*` tools: what the agent keeps for the user across conversations (see
//! [`crate::records`]). `records_list`, `records_describe` and `records_lookup` read,
//! `records_save` adds or updates, and `records_drop`, in interactive runs only, deletes a set.

use super::{Tool, ToolContext};
use crate::records::{
    RecordsContext, Refused, SaveOptions, LOOKUP_DEFAULT_ROWS, LOOKUP_MAX_ROWS, MAX_BATCH,
    MAX_NAME_CHARS, MAX_RECORD_CHARS,
};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

pub const RECORDS_LIST: &str = "records_list";
pub const RECORDS_DESCRIBE: &str = "records_describe";
pub const RECORDS_SAVE: &str = "records_save";
pub const RECORDS_LOOKUP: &str = "records_lookup";
pub const RECORDS_DROP: &str = "records_drop";

/// Every records tool, by name.
pub const RECORDS_TOOLS: &[&str] = &[
    RECORDS_LIST,
    RECORDS_DESCRIBE,
    RECORDS_SAVE,
    RECORDS_LOOKUP,
    RECORDS_DROP,
];

/// What a drop in an unattended run gets back.
const READ_ONLY: &str = "records_drop is refused in unattended runs; tell the user instead";

fn records(ctx: &ToolContext) -> Result<&RecordsContext> {
    ctx.records
        .as_ref()
        .ok_or_else(|| anyhow!("records are not available in this run"))
}

fn set_name(input: &Value) -> Result<&str> {
    input["set"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("set must be the name of a record set"))
}

fn refused(r: Refused) -> anyhow::Error {
    anyhow!("{r}")
}

fn set_schema(what: &str) -> Value {
    json!({ "type": "string", "maxLength": MAX_NAME_CHARS,
            "pattern": "^[a-z][a-z0-9_]*$",
            "description": what })
}

/// The user's record sets, with their size and columns.
pub struct RecordsList;

#[async_trait]
impl Tool for RecordsList {
    fn name(&self) -> &str {
        RECORDS_LIST
    }

    fn description(&self) -> &str {
        "List the record sets you keep for this user from earlier conversations: each set's name \
         (its table name in records_lookup), description, key fields, how many records it holds \
         and when it was last saved. Call it when you need more than the summary in your \
         instructions, or to check a name before records_save."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn call(&self, ctx: &ToolContext, _input: &Value) -> Result<String> {
        let r = records(ctx)?;
        let sets = r.records.list(&r.owner).await?;
        let columns = r.records.columns_by_set(&r.owner, &sets).await?;
        let sets: Vec<Value> = sets
            .into_iter()
            .map(|s| {
                let mut v = serde_json::to_value(&s).unwrap_or_default();
                v["columns"] = json!(columns.get(&s.name).cloned().unwrap_or_default());
                v
            })
            .collect();
        Ok(json!({ "sets": sets, "limits": r.records.limits() }).to_string())
    }
}

/// A set's columns and a few records from each end.
pub struct RecordsDescribe;

#[async_trait]
impl Tool for RecordsDescribe {
    fn name(&self) -> &str {
        RECORDS_DESCRIBE
    }

    fn description(&self) -> &str {
        "Describe one record set before you query it: its columns with their type and how many \
         records fill each, its key fields, and the oldest and newest records as examples. Use \
         it to write a records_lookup query that fits the data."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": { "set": set_schema("The set's name, from records_list.") },
                "required": ["set"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let r = records(ctx)?;
        let name = set_name(input)?;
        match r.records.describe(&r.owner, name).await? {
            Some(d) => Ok(serde_json::to_string(&d)?),
            None => Err(refused(Refused::NotFound(name.to_string()))),
        }
    }
}

/// Add or update records in a set, creating it on first use.
pub struct RecordsSave;

#[async_trait]
impl Tool for RecordsSave {
    fn name(&self) -> &str {
        RECORDS_SAVE
    }

    fn description(&self) -> &str {
        "Save records for the user to come back to in later conversations: data they gave you \
         (an export, measurements, a log, a weekly report), one JSON object per row with flat, \
         lowercase field names, into a named set. The first save of a set needs a description \
         and should name its key: the fields that identify a row (a date, an id), so saving the \
         same row again updates it instead of adding a second one. Later saves into the set use \
         its key. Keep rows small and normalized (numbers as numbers, dates as ISO 8601 \
         strings), and save a large export in batches. Returns how many records were added and \
         updated and the set's new size. Don't save conversation text, tool results you could \
         fetch again, or anything the user didn't give you."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "set": set_schema("The set to save into, e.g. \"running_log\". Lowercase letters, digits and underscores."),
                    "records": {
                        "type": "array", "minItems": 1, "maxItems": MAX_BATCH,
                        "items": { "type": "object" },
                        "description": format!("The rows, each a JSON object of at most {MAX_RECORD_CHARS} characters with lowercase field names.")
                    },
                    "description": { "type": "string",
                                     "description": "What the set holds, for a new set (or to change it), e.g. \"One row per run from the user's weekly Garmin export\"." },
                    "key": { "type": "array", "items": { "type": "string" },
                             "description": "The fields that identify a row, e.g. [\"date\"]. Set it on the first save; it can't change once the set holds records. Without one, every save appends." },
                    "health_data": { "type": "boolean",
                                     "description": "True when the set holds personal health data (heart rate, glucose, weight, sleep); it brings the health rules into later runs." }
                },
                "required": ["set", "records"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let r = records(ctx)?;
        let name = set_name(input)?;
        let rows = input["records"]
            .as_array()
            .ok_or_else(|| anyhow!("records must be an array of objects"))?;
        let key: Option<Vec<String>> = match &input["key"] {
            Value::Null => None,
            Value::Array(fields) => Some(
                fields
                    .iter()
                    .map(|f| {
                        f.as_str()
                            .map(|s| s.trim().to_string())
                            .ok_or_else(|| anyhow!("key must be a list of field names"))
                    })
                    .collect::<Result<_>>()?,
            ),
            _ => bail!("key must be a list of field names"),
        };
        let health_data = match &input["health_data"] {
            Value::Null => None,
            Value::Bool(b) => Some(*b),
            _ => bail!("health_data must be true or false"),
        };
        let saved = r
            .records
            .save(
                &r.owner,
                name,
                rows,
                SaveOptions {
                    description: input["description"].as_str(),
                    key: key.as_deref(),
                    health_data,
                    run_id: Some(r.run_id),
                },
            )
            .await?
            .map_err(refused)?;
        Ok(json!({
            "set": saved.set.name,
            "added": saved.added,
            "updated": saved.updated,
            "count": saved.set.count,
            "key": saved.set.key_fields,
        })
        .to_string())
    }
}

/// A read-only SQL query over the user's sets.
pub struct RecordsLookup;

#[async_trait]
impl Tool for RecordsLookup {
    fn name(&self) -> &str {
        RECORDS_LOOKUP
    }

    fn description(&self) -> &str {
        "Read the user's records with one read-only SQL query (SQLite syntax). Each record set is \
         a table of the same name, with its records' fields as columns plus _id and _saved_at \
         (ISO 8601); nothing else is reachable. Use it to find, filter, aggregate and compare \
         what was saved before: GROUP BY strftime('%Y-%W', date) for weeks, window functions \
         for changes between rows, joins across sets on a shared date. Returns the columns and \
         rows, capped at max_rows (truncated says if more matched), so aggregate in SQL rather \
         than reading every row. Only SELECT (or WITH ... SELECT); to change records use \
         records_save."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "sql": { "type": "string",
                             "description": "One SELECT query, e.g. \"SELECT strftime('%Y-%W', date) AS week, sum(km) AS km FROM running_log GROUP BY week ORDER BY week DESC LIMIT 8\"." },
                    "max_rows": { "type": "integer", "minimum": 1, "maximum": LOOKUP_MAX_ROWS,
                                  "default": LOOKUP_DEFAULT_ROWS,
                                  "description": "The most rows to return." }
                },
                "required": ["sql"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let r = records(ctx)?;
        let sql = input["sql"]
            .as_str()
            .ok_or_else(|| anyhow!("sql must be a SELECT query"))?;
        let max_rows = match &input["max_rows"] {
            Value::Null => None,
            v => Some(
                v.as_u64()
                    .ok_or_else(|| anyhow!("max_rows must be a positive integer"))?
                    as usize,
            ),
        };
        let found = r
            .records
            .lookup(&r.owner, sql, max_rows)
            .await?
            .map_err(refused)?;
        Ok(serde_json::to_string(&found)?)
    }
}

/// Delete a set. Interactive runs only.
pub struct RecordsDrop;

#[async_trait]
impl Tool for RecordsDrop {
    fn name(&self) -> &str {
        RECORDS_DROP
    }

    fn description(&self) -> &str {
        "Delete one record set with everything in it, when the user asks for it to go. There is \
         no undo; to remove or correct single rows, save them again with records_save instead. \
         Refused in unattended runs."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": { "set": set_schema("The set to delete.") },
                "required": ["set"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let r = records(ctx)?;
        if !r.interactive {
            bail!(READ_ONLY);
        }
        let name = set_name(input)?;
        if !r.records.drop(&r.owner, name).await? {
            return Err(refused(Refused::NotFound(name.to_string())));
        }
        Ok(json!({ "status": "dropped", "set": name }).to_string())
    }
}
