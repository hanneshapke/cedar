//! `saved_actions`: what the user showed the agent how to do (see [`crate::actions`]). Without a
//! name it lists the owner's actions; with one it returns that action's steps. It only reads:
//! actions are made and changed by the user, from a recording, through `/v1/actions`.

use super::{Tool, ToolContext};
use crate::actions::{for_model, ActionsContext};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

pub const NAME: &str = "saved_actions";

fn actions(ctx: &ToolContext) -> Result<&ActionsContext> {
    ctx.actions
        .as_ref()
        .ok_or_else(|| anyhow!("saved actions are not available in this run"))
}

pub struct SavedActions;

#[async_trait]
impl Tool for SavedActions {
    fn name(&self) -> &str {
        NAME
    }

    fn description(&self) -> &str {
        "Read how the user does something, from the actions they saved by recording themselves \
         doing it. With `name`, returns that action's steps to follow, the site it happens on, and \
         the names of the secrets saved with it (never their values; the browser tool types \
         them). Without `name`, lists every saved action. Use it when a request matches a saved \
         action, and before you report that you can't do something."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "name": { "type": "string",
                              "description": "The action's name, as your instructions or the list give it. Leave out to list them all." }
                } })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let a = actions(ctx)?;
        let name = match &input["name"] {
            Value::Null => None,
            v => Some(v.as_str().map(str::trim).ok_or_else(|| anyhow!("name must be text"))?),
        };
        let Some(name) = name.filter(|n| !n.is_empty()) else {
            let list: Vec<Value> = a
                .actions
                .list(&a.owner)
                .await?
                .iter()
                .map(|x| json!({ "name": x.name, "title": x.title, "summary": x.summary, "site": x.site, "steps": x.steps.len() }))
                .collect();
            return Ok(json!({ "actions": list }).to_string());
        };
        let action = a.actions.find(&a.owner, name).await?.ok_or_else(|| {
            anyhow!("there is no saved action named {name:?}; call saved_actions without a name to list them")
        })?;
        a.actions.used(&a.owner, action.id).await?;
        Ok(for_model(&action).to_string())
    }
}
