//! Google Slides through the REST API: making a presentation from an outline. The model says
//! what each slide holds (a layout and its text); cedar chooses every position, size, font and
//! colour, so a deck comes out consistent and designed rather than as boxes placed by guesswork.
//! Like the other Google tools these run in the cedar process: the OAuth tokens stay here, and
//! the sandbox has no network.
//!
//! A deck is drawn on blank 16:9 pages with ordinary text boxes, shapes and tables, so its owner
//! can go on editing it in Slides like any other.

mod deck;
mod render;
mod theme;

use super::google::{required_str, select, str_arg, without_empty, GoogleApi};
use super::{truncate_output, Tool, ToolContext};
use crate::config::GoogleApiConfig;
use crate::mcp::GoogleOAuth;
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use deck::Deck;
use render::Rendered;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use theme::{Color, Theme};
use uuid::Uuid;

/// The tools implemented here, in the order they are offered.
const TOOLS: &[&str] = &["create_presentation", "append_slides", "get_presentation"];
/// At most this many requests go in one batch update; a slide's requests stay together.
const MAX_BATCH: usize = 400;
/// The page every deck is made with: 16:9, 10 by 5.625 inches, in EMU.
const PAGE_EMU: (u64, u64) = (9_144_000, 5_143_500);
const NOTES_FIELDS: &str =
    "slides(objectId,slideProperties(notesPage(notesProperties(speakerNotesObjectId))))";
const TEXT: &str = "text(textElements(textRun(content)))";
/// How much of each text on a slide `get_presentation` shows.
const OUTLINE_CHARS: usize = 300;

/// The Slides tools `cfg` allows, sharing one API client.
pub fn tools(cfg: &GoogleApiConfig, auth: Arc<GoogleOAuth>) -> Result<Vec<Box<dyn Tool>>> {
    let api = Arc::new(GoogleApi::new(&cfg.url, auth, "Slides")?);
    Ok(select(&cfg.prefix, &cfg.tools, TOOLS)
        .into_iter()
        .map(|op| {
            Box::new(SlidesTool {
                name: format!("{}_{op}", cfg.prefix),
                op,
                api: api.clone(),
            }) as Box<dyn Tool>
        })
        .collect())
}

/// One Slides operation; `op` is one of [`TOOLS`].
pub struct SlidesTool {
    name: String,
    op: &'static str,
    api: Arc<GoogleApi>,
}

#[async_trait]
impl Tool for SlidesTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        match self.op {
            "create_presentation" => {
                "Make a designed Google Slides presentation in the user's Drive from an outline, \
                 and get its link. You give the content, slide by slide; the layout, type and \
                 colours are done for you, so never describe positions, sizes or fonts. Write it \
                 like a good deck: open with a title slide, one idea per slide, a title that \
                 states the slide's point (\"Revenue grew 18%\", not \"Revenue\"), 3 to 5 short \
                 bullets rather than paragraphs, and what you would say aloud in notes. Vary \
                 the layouts: stats for the few numbers that matter, columns to compare, steps \
                 for a process or timeline, table for figures, quote for a voice, section to \
                 open each part of a long deck, closing to end. **word** makes text bold. The \
                 result lists warnings for slides whose text may not fit; fix those with \
                 slides_append_slides or tell the user."
            }
            "append_slides" => {
                "Add slides to a presentation made with slides_create_presentation: at the end, \
                 or from the position insertAt. Slides are written as for \
                 slides_create_presentation; pass the same theme (and accent and footer) the \
                 deck was made with so they match."
            }
            "get_presentation" => {
                "The outline of a Google Slides presentation: its title, link, and the text on \
                 each slide in order, to check a deck or decide where slides go."
            }
            _ => unreachable!("not a Slides tool: {}", self.op),
        }
    }

    fn schema(&self) -> Value {
        let id = json!({ "type": "string",
            "description": "The presentation's ID (from slides_create_presentation, or its Drive file ID)." });
        let theme = json!({ "type": "string", "enum": theme::NAMES,
            "description": "The deck's look (default minimal). minimal: white, dark covers, blue accent, clean sans; suits most business decks. midnight: dark throughout; product and tech talks. editorial: warm paper with serif headings; stories and strategy. forest: soft green with serif headings; calm, people and sustainability topics. bold: white with vivid violet covers; pitches and launches." });
        let accent = json!({ "type": "string",
            "description": "A brand colour to use as the accent instead of the theme's own, like #E11D48." });
        let footer = json!({ "type": "string",
            "description": "A short line at the foot of every content slide, like a company name or \"Confidential\"." });
        match self.op {
            "create_presentation" => json!({ "type": "object", "properties": {
                "title": { "type": "string", "description": "The presentation's name in Drive." },
                "theme": theme, "accent": accent, "footer": footer,
                "slides": slides_schema(),
            }, "required": ["title", "slides"] }),
            "append_slides" => json!({ "type": "object", "properties": {
                "presentationId": id,
                "slides": slides_schema(),
                "insertAt": { "type": "integer", "minimum": 1,
                    "description": "The position the first new slide takes (1 puts it first). Default: after the last slide." },
                "theme": theme, "accent": accent, "footer": footer,
            }, "required": ["presentationId", "slides"] }),
            "get_presentation" => json!({ "type": "object", "properties": {
                "presentationId": id,
            }, "required": ["presentationId"] }),
            _ => unreachable!("not a Slides tool: {}", self.op),
        }
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let t = ctx.timeout;
        let out = match self.op {
            "create_presentation" => self.create(input, t).await?,
            "append_slides" => self.append(input, t).await?,
            "get_presentation" => {
                let id = presentation_id(input)?;
                render_outline(&self.api.get(&[id], &[("fields", outline_fields())], t).await?)
            }
            _ => unreachable!("not a Slides tool: {}", self.op),
        };
        Ok(truncate_output(
            &serde_json::to_string(&without_empty(out))?,
            ctx.max_output,
        ))
    }
}

fn slides_schema() -> Value {
    let text = |description: &str| json!({ "type": "string", "description": description });
    json!({ "type": "array", "minItems": 1, "maxItems": deck::MAX_SLIDES,
        "description": "The slides, in order.",
        "items": { "type": "object", "properties": {
            "layout": { "type": "string", "enum": deck::LAYOUTS,
                "description": "title: the opening slide (title, subtitle, meta, kicker). section: a divider opening a part of the deck (title, subtitle); sections are numbered. bullets: title with bullets and/or body. columns: 2 to 4 side by side (columns). stats: 1 to 4 big numbers (stats, caption). quote: quote and attribution. table: table and caption. image: imageUrl, alone or beside bullets or body, with caption. steps: 2 to 6 steps of a process or timeline (steps). closing: the last slide (title, subtitle, meta). Left out, it follows from what the slide holds." },
            "title": text("The slide's heading; every layout but quote and image needs one. Keep it to one line."),
            "subtitle": text("For title, section and closing: a line under the title."),
            "kicker": text("For title and section: a short label above the title (an occasion, a client, \"Appendix\")."),
            "meta": text("For title and closing: a small line at the foot, like the presenter and date, or contact details."),
            "body": text("A short paragraph; on a bullets slide it leads into the bullets."),
            "bullets": { "type": "array", "description": "Short points. Each is a string, or {text, sub: [...]} for points under it.",
                "items": { "anyOf": [
                    { "type": "string" },
                    { "type": "object", "properties": { "text": { "type": "string" },
                        "sub": { "type": "array", "items": { "type": "string" } } }, "required": ["text"] },
                ] } },
            "columns": { "type": "array", "minItems": 2, "maxItems": 4,
                "items": { "type": "object", "properties": {
                    "heading": { "type": "string" },
                    "bullets": { "type": "array", "items": { "type": "string" } },
                    "body": { "type": "string" },
                } } },
            "stats": { "type": "array", "minItems": 1, "maxItems": 4,
                "items": { "type": "object", "properties": {
                    "value": text("The number as it should read, with its unit: \"$4.2M\", \"118%\", \"3×\"."),
                    "label": text("What it counts, in a few words."),
                }, "required": ["value", "label"] } },
            "quote": text("The quotation, without quotation marks."),
            "attribution": text("Who said it."),
            "table": { "type": "object", "description": "At most 8 columns and 12 rows; keep cells short.",
                "properties": {
                    "headers": { "type": "array", "items": { "type": "string" } },
                    "rows": { "type": "array", "items": { "type": "array", "items": { "type": "string" } } },
                }, "required": ["rows"] },
            "imageUrl": text("A public https link to a PNG, JPEG or GIF. Google fetches it itself, so a file in the workspace can't be used."),
            "caption": text("A small line under a table, image or row of stats: a source, a unit, a caveat."),
            "steps": { "type": "array", "minItems": 2, "maxItems": 6,
                "items": { "type": "object", "properties": {
                    "title": { "type": "string" },
                    "text": { "type": "string" },
                }, "required": ["title"] } },
            "notes": text("Speaker notes: what to say with this slide."),
        } } })
}

fn outline_fields() -> String {
    format!(
        "presentationId,title,slides(objectId,pageElements(shape({TEXT}),table(tableRows(tableCells({TEXT})))))"
    )
}

/// A presentation's ID as given, refused if it could be anything but one path segment's worth.
fn presentation_id(input: &Value) -> Result<&str> {
    let id = required_str(input, "presentationId")?;
    // People paste links; take the ID out of one.
    let id = id
        .split_once("/presentation/d/")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(rest))
        .unwrap_or(id);
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        bail!("presentationId is not a presentation's ID");
    }
    Ok(id)
}

fn link(id: &str) -> String {
    format!("https://docs.google.com/presentation/d/{id}/edit")
}

/// The theme a call asks for, with its accent.
fn theme_of(input: &Value) -> Result<Theme> {
    let theme = Theme::named(str_arg(input, "theme").unwrap_or(theme::DEFAULT))?;
    match str_arg(input, "accent") {
        Some(accent) => theme.with_accent(Color::parse(accent)?),
        None => Ok(theme),
    }
}

/// A prefix for one call's object IDs, which must be unique in a presentation.
fn id_prefix() -> String {
    format!("c{}", &Uuid::new_v4().simple().to_string()[..10])
}

/// The slides' requests as batches of about [`MAX_BATCH`], with `tail` ending the first.
fn batches(slides: Vec<Vec<Value>>, tail: Vec<Value>) -> Vec<Vec<Value>> {
    let mut out: Vec<Vec<Value>> = Vec::new();
    for slide in slides {
        match out.last_mut() {
            Some(batch) if batch.len() + slide.len() <= MAX_BATCH => batch.extend(slide),
            _ => out.push(slide),
        }
    }
    if let Some(first) = out.first_mut() {
        first.extend(tail);
    }
    out
}

impl SlidesTool {
    async fn batch_update(&self, id: &str, requests: Vec<Value>, t: Duration) -> Result<Value> {
        self.api
            .post(&[&format!("{id}:batchUpdate")], &json!({ "requests": requests }), t)
            .await
    }

    /// Draw the slides, then write their speaker notes. Returns warnings: the layout's own, and
    /// one if the notes couldn't be written (the slides are there by then).
    async fn draw(&self, id: &str, r: Rendered, tail: Vec<Value>, t: Duration) -> Result<Vec<String>> {
        let mut warnings = r.warnings;
        for requests in batches(r.slides, tail) {
            self.batch_update(id, requests, t).await.map_err(|e| {
                let mut message = e.to_string();
                if message.contains("createImage") || message.contains("image") {
                    message.push_str(
                        " (an image must be a public https link to a PNG, JPEG or GIF under \
                         50 MB that Google can fetch)",
                    );
                }
                anyhow!(message)
            })?;
        }
        if !r.notes.is_empty() {
            if let Err(e) = self.write_notes(id, &r.notes, t).await {
                warnings.push(format!("the speaker notes were not written: {e}"));
            }
        }
        Ok(warnings)
    }

    /// A slide's notes box is made by Slides with the slide, so its ID is looked up first.
    async fn write_notes(&self, id: &str, notes: &[(String, String)], t: Duration) -> Result<()> {
        let pages = self.api.get(&[id], &[("fields", NOTES_FIELDS.to_string())], t).await?;
        let boxes: HashMap<&str, &str> = pages["slides"]
            .as_array()
            .map(|slides| {
                slides
                    .iter()
                    .filter_map(|s| {
                        let notes = &s["slideProperties"]["notesPage"]["notesProperties"];
                        Some((s["objectId"].as_str()?, notes["speakerNotesObjectId"].as_str()?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let requests: Vec<Value> = notes
            .iter()
            .filter_map(|(slide, text)| {
                let object = boxes.get(slide.as_str())?;
                Some(json!({ "insertText": { "objectId": object, "insertionIndex": 0, "text": text } }))
            })
            .collect();
        if requests.len() < notes.len() {
            bail!("Slides gave no notes box for {} of the slides", notes.len() - requests.len());
        }
        self.batch_update(id, requests, t).await?;
        Ok(())
    }

    async fn create(&self, input: &Value, t: Duration) -> Result<Value> {
        let title: String = required_str(input, "title")?.chars().take(200).collect();
        // Everything that can be refused is, before anything is made in the user's Drive.
        let theme = theme_of(input)?;
        let deck = Deck::parse(input, true)?;
        let rendered = render::render(&deck, &theme, &id_prefix(), None);
        let count = rendered.slides.len();

        let created = self
            .api
            .post(
                &[],
                &json!({ "title": title, "pageSize": {
                    "width": { "magnitude": PAGE_EMU.0, "unit": "EMU" },
                    "height": { "magnitude": PAGE_EMU.1, "unit": "EMU" },
                } }),
                t,
            )
            .await?;
        let id = created["presentationId"]
            .as_str()
            .ok_or_else(|| anyhow!("Slides API: the new presentation has no ID"))?
            .to_string();
        // A new presentation comes with an empty slide of its own, which goes once ours exist.
        let defaults: Vec<Value> = created["slides"]
            .as_array()
            .map(|slides| {
                slides
                    .iter()
                    .filter_map(|s| s["objectId"].as_str())
                    .map(|id| json!({ "deleteObject": { "objectId": id } }))
                    .collect()
            })
            .unwrap_or_default();
        let warnings = self.draw(&id, rendered, defaults, t).await.map_err(|e| {
            anyhow!(
                "{e}. An unfinished presentation named {title:?} is left in the user's Drive \
                 ({}); fix what the error names and make the deck again.",
                link(&id)
            )
        })?;
        Ok(json!({
            "presentationId": id,
            "title": created["title"].as_str().unwrap_or(&title),
            "link": link(&id),
            "slideCount": count,
            "theme": theme.name,
            "warnings": warnings,
        }))
    }

    async fn append(&self, input: &Value, t: Duration) -> Result<Value> {
        let id = presentation_id(input)?;
        let theme = theme_of(input)?;
        let deck = Deck::parse(input, false)?;
        let insert_at = match &input["insertAt"] {
            Value::Null => None,
            v => match v.as_u64() {
                Some(n) if n >= 1 => Some(n as usize - 1),
                _ => bail!("insertAt is a slide position, counted from 1"),
            },
        };
        let rendered = render::render(&deck, &theme, &id_prefix(), insert_at);
        let count = rendered.slides.len();
        let warnings = self.draw(id, rendered, Vec::new(), t).await?;
        Ok(json!({
            "presentationId": id,
            "link": link(id),
            "slidesAdded": count,
            "theme": theme.name,
            "warnings": warnings,
        }))
    }
}

/// The text of one text body, as one line.
fn body_text(text: &Value) -> String {
    let joined: String = text["textElements"]
        .as_array()
        .map(|els| els.iter().filter_map(|e| e["textRun"]["content"].as_str()).collect())
        .unwrap_or_default();
    joined
        .split('\n')
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" / ")
}

fn shorten(s: String) -> String {
    if s.chars().count() <= OUTLINE_CHARS {
        return s;
    }
    let mut cut: String = s.chars().take(OUTLINE_CHARS).collect();
    cut.push('…');
    cut
}

fn render_outline(p: &Value) -> Value {
    let slides: Vec<Value> = p["slides"]
        .as_array()
        .map(|slides| {
            slides
                .iter()
                .enumerate()
                .map(|(i, slide)| {
                    let mut texts = Vec::new();
                    for el in slide["pageElements"].as_array().into_iter().flatten() {
                        let shape = body_text(&el["shape"]["text"]);
                        if !shape.is_empty() {
                            texts.push(shorten(shape));
                        }
                        for row in el["table"]["tableRows"].as_array().into_iter().flatten() {
                            let cells: Vec<String> = row["tableCells"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .map(|c| body_text(&c["text"]))
                                .collect();
                            if cells.iter().any(|c| !c.is_empty()) {
                                texts.push(shorten(cells.join(" | ")));
                            }
                        }
                    }
                    json!({ "position": i + 1, "objectId": slide["objectId"], "text": texts })
                })
                .collect()
        })
        .unwrap_or_default();
    let id = p["presentationId"].as_str().unwrap_or_default();
    json!({
        "presentationId": id,
        "title": p["title"],
        "link": link(id),
        "slideCount": slides.len(),
        "slides": slides,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_ids_are_taken_from_links_and_checked() {
        let id = |v: &str| presentation_id(&json!({ "presentationId": v })).map(str::to_string);
        assert_eq!(id("1AbC-d_9").unwrap(), "1AbC-d_9");
        assert_eq!(
            id("https://docs.google.com/presentation/d/1AbC-d_9/edit#slide=id.p").unwrap(),
            "1AbC-d_9"
        );
        assert!(id("1AbC:batchUpdate").is_err());
        assert!(id("../drive/v3/files").is_err());
        assert!(id("https://docs.google.com/presentation/d//edit").is_err());
        assert!(presentation_id(&json!({})).is_err());
    }

    #[test]
    fn a_slide_is_never_split_over_two_batches() {
        let slide = |n: usize| vec![json!({ "x": 1 }); n];
        let out = batches(vec![slide(250), slide(100), slide(100), slide(390)], vec![json!("tail")]);
        let sizes: Vec<usize> = out.iter().map(Vec::len).collect();
        // The tail (removing the presentation's own first slide) ends the first batch.
        assert_eq!(sizes, [351, 100, 390]);
        assert_eq!(out[0].last().unwrap(), "tail");
        assert_eq!(batches(vec![slide(3)], vec![json!("tail")])[0].len(), 4);
        // A slide larger than a batch still goes, alone.
        assert_eq!(batches(vec![slide(500), slide(1)], Vec::new()).len(), 2);
    }

    #[test]
    fn themes_and_accents_come_from_the_call() {
        assert_eq!(theme_of(&json!({})).unwrap().name, "minimal");
        let t = theme_of(&json!({ "theme": "Forest", "accent": "#B4532A" })).unwrap();
        assert_eq!((t.name, t.accent), ("forest", Color(0xB4532A)));
        assert!(theme_of(&json!({ "theme": "neon" })).unwrap_err().to_string().contains("minimal"));
        assert!(theme_of(&json!({ "accent": "red" })).is_err());
        let ids = (id_prefix(), id_prefix());
        assert!(ids.0 != ids.1 && ids.0.len() == 11 && ids.0.starts_with('c'));
    }

    #[test]
    fn an_outline_shows_each_slides_text() {
        let run = |s: &str| json!({ "text": { "textElements": [{ "paragraphMarker": {} }, { "textRun": { "content": s } }] } });
        let out = render_outline(&json!({ "presentationId": "p1", "title": "Q3", "slides": [
            { "objectId": "s1", "pageElements": [{ "shape": run("Q3 review\n") }, { "shape": {} }] },
            { "objectId": "s2", "pageElements": [
                { "shape": run("One\nTwo\n") },
                { "table": { "tableRows": [{ "tableCells": [run("Region\n"), run("Revenue\n")] }] } },
                { "shape": run(&"long ".repeat(100)) },
            ] },
        ] }));
        assert_eq!(out["link"], "https://docs.google.com/presentation/d/p1/edit");
        assert_eq!(out["slideCount"], 2);
        assert_eq!(out["slides"][0], json!({ "position": 1, "objectId": "s1", "text": ["Q3 review"] }));
        assert_eq!(out["slides"][1]["text"][0], "One / Two");
        assert_eq!(out["slides"][1]["text"][1], "Region | Revenue");
        assert_eq!(out["slides"][1]["text"][2].as_str().unwrap().chars().count(), OUTLINE_CHARS + 1);
    }
}
