//! What the model hands over: a deck as an outline, one entry per slide, with no positions, sizes
//! or colours in it. Parsing is lenient about shape (a bullet may be a string or an object, the
//! layout may be left out) and strict about what can't be laid out well.

use anyhow::{anyhow, bail, Result};
use serde_json::Value;

pub const MAX_SLIDES: usize = 60;
const MAX_BULLETS: usize = 14;
const MAX_COLUMNS: usize = 4;
const MAX_STATS: usize = 4;
const MAX_STEPS: usize = 6;
const MAX_TABLE_COLUMNS: usize = 8;
const MAX_TABLE_ROWS: usize = 12;
/// Longest text taken for one field; more than this is not a slide.
const MAX_TEXT: usize = 2000;

pub const LAYOUTS: &[&str] = &[
    "title", "section", "bullets", "columns", "stats", "quote", "table", "image", "steps",
    "closing",
];

/// Text with its `**bold**` spans, as UTF-16 ranges (what the Slides API counts in).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rich {
    pub text: String,
    pub bold: Vec<(usize, usize)>,
}

impl Rich {
    /// `**bold**` becomes a span; a marker left open is dropped.
    pub fn parse(s: &str) -> Self {
        let mut text = String::new();
        let mut bold = Vec::new();
        let mut open: Option<usize> = None;
        let mut units = 0;
        let mut rest = s;
        while let Some(at) = rest.find("**") {
            text.push_str(&rest[..at]);
            units += rest[..at].encode_utf16().count();
            match open.take() {
                Some(start) if units > start => bold.push((start, units)),
                Some(_) => {}
                None => open = Some(units),
            }
            rest = &rest[at + 2..];
        }
        text.push_str(rest);
        Self { text, bold }
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bullet {
    pub text: Rich,
    /// 0 for a point, 1 for a point under it.
    pub level: u8,
}

/// A slide's running text: a paragraph, bullets, or a paragraph leading into bullets.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Block {
    pub paragraph: Option<Rich>,
    pub bullets: Vec<Bullet>,
}

impl Block {
    pub fn is_empty(&self) -> bool {
        self.paragraph.is_none() && self.bullets.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub heading: Option<String>,
    pub block: Block,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stat {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub title: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// Empty when the table has no header row.
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub columns: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Title { kicker: Option<String>, subtitle: Option<String>, meta: Option<String> },
    Section { kicker: Option<String>, subtitle: Option<String> },
    Bullets { block: Block },
    Columns { columns: Vec<Column> },
    Stats { stats: Vec<Stat>, caption: Option<String> },
    Quote { quote: String, attribution: Option<String> },
    Table { table: Table, caption: Option<String> },
    Image { url: String, caption: Option<String>, block: Block },
    Steps { steps: Vec<Step> },
    Closing { subtitle: Option<String>, meta: Option<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Slide {
    pub title: String,
    pub content: Content,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Deck {
    pub slides: Vec<Slide>,
    /// A line at the foot of every content slide (a company name, "Confidential").
    pub footer: Option<String>,
}

impl Deck {
    /// The `slides` and `footer` of a tool call. `first_is_cover` lets a first slide with only a
    /// title be the title slide when it names no layout.
    pub fn parse(input: &Value, first_is_cover: bool) -> Result<Self> {
        let items = match &input["slides"] {
            Value::Array(items) if !items.is_empty() => items,
            Value::Array(_) | Value::Null => bail!("slides is required: one entry per slide"),
            _ => bail!("slides must be an array, one entry per slide"),
        };
        if items.len() > MAX_SLIDES {
            bail!(
                "{} slides is more than the {MAX_SLIDES} one call takes; add the rest with \
                 another call",
                items.len()
            );
        }
        let slides = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                parse_slide(item, first_is_cover && i == 0)
                    .map_err(|e| anyhow!("slide {}: {e}", i + 1))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { slides, footer: opt(input, &["footer"]) })
    }
}

/// One line of clean text: control characters out, whitespace runs collapsed.
fn line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Clean text that keeps its paragraph breaks.
fn paragraphs(s: &str) -> String {
    s.lines().map(line).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

/// A value as text: a string, or a number or boolean as written.
fn as_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// The first of `keys` that holds text, as one line; `None` when empty or missing.
fn opt(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|k| as_text(&v[*k]))
        .map(|s| line(&s).chars().take(MAX_TEXT).collect::<String>())
        .find(|s| !s.is_empty())
}

/// Like [`opt`], keeping paragraph breaks.
fn opt_paragraphs(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|k| as_text(&v[*k]))
        .map(|s| paragraphs(&s).chars().take(MAX_TEXT).collect::<String>())
        .find(|s| !s.is_empty())
}

/// A list marker the model wrote itself ("- ", "• ", "1. "), which the slide's own bullets replace.
fn without_marker(s: &str) -> &str {
    let s = s.trim_start();
    for marker in ["- ", "• ", "* ", "– ", "· "] {
        if let Some(rest) = s.strip_prefix(marker) {
            return rest.trim_start();
        }
    }
    s
}

fn bullets(v: &Value) -> Result<Vec<Bullet>> {
    let items = match v {
        Value::Null => return Ok(Vec::new()),
        Value::Array(items) => items.clone(),
        // One string with a point per line.
        Value::String(s) => s.lines().map(|l| Value::String(l.to_string())).collect(),
        _ => bail!("bullets must be an array of strings"),
    };
    let mut out = Vec::new();
    let mut push = |text: &str, level: u8| {
        let text = Rich::parse(&line(without_marker(text)));
        if !text.is_empty() {
            out.push(Bullet { text, level });
        }
    };
    for item in &items {
        match item {
            Value::Object(_) => {
                if let Some(text) = as_text(&item["text"]) {
                    push(&text, 0);
                }
                for key in ["sub", "children", "bullets"] {
                    if let Value::Array(subs) = &item[key] {
                        for sub in subs.iter().filter_map(as_text) {
                            push(&sub, 1);
                        }
                    }
                }
            }
            other => match as_text(other) {
                // Two or more leading spaces or a tab make a point a sub-point.
                Some(s) => {
                    let nested = s.starts_with("  ") || s.starts_with('\t');
                    push(&s, u8::from(nested));
                }
                None => bail!("bullets must be strings, or {{text, sub: [...]}}"),
            },
        }
    }
    // A sub-point needs a point above it.
    if let Some(first) = out.first_mut() {
        first.level = 0;
    }
    if out.len() > MAX_BULLETS {
        bail!(
            "{} bullets don't fit on one slide (at most {MAX_BULLETS}, and 3 to 5 read best); \
             split them over two slides",
            out.len()
        );
    }
    Ok(out)
}

fn block(v: &Value) -> Result<Block> {
    Ok(Block {
        paragraph: opt_paragraphs(v, &["body", "text"]).map(|s| Rich::parse(&s)),
        bullets: bullets(&v["bullets"])?,
    })
}

fn columns(v: &Value) -> Result<Vec<Column>> {
    let Value::Array(items) = v else {
        bail!("a columns slide needs columns: [{{heading, bullets or body}}, ...]");
    };
    let columns = items
        .iter()
        .map(|c| match c {
            Value::Object(_) => {
                Ok(Column { heading: opt(c, &["heading", "title"]), block: block(c)? })
            }
            other => match as_text(other) {
                Some(s) => Ok(Column {
                    heading: None,
                    block: Block {
                        paragraph: Some(Rich::parse(&paragraphs(&s))),
                        bullets: Vec::new(),
                    },
                }),
                None => bail!("each column is {{heading, bullets or body}}"),
            },
        })
        .collect::<Result<Vec<_>>>()?;
    if !(2..=MAX_COLUMNS).contains(&columns.len()) {
        bail!("a columns slide takes 2 to {MAX_COLUMNS} columns, not {}", columns.len());
    }
    if columns.iter().any(|c| c.heading.is_none() && c.block.is_empty()) {
        bail!("a column is empty; give each a heading, bullets or body");
    }
    Ok(columns)
}

fn stats(v: &Value) -> Result<Vec<Stat>> {
    let Value::Array(items) = v else {
        bail!("a stats slide needs stats: [{{value, label}}, ...]");
    };
    let stats = items
        .iter()
        .map(|s| {
            let value = opt(s, &["value", "number"])
                .ok_or_else(|| anyhow!("each stat needs a value (like \"42%\") and a label"))?;
            if value.chars().count() > 14 {
                bail!("the stat value {value:?} is too long to show large; keep it to a number and its unit");
            }
            Ok(Stat { value, label: opt(s, &["label", "caption", "text"]).unwrap_or_default() })
        })
        .collect::<Result<Vec<_>>>()?;
    if !(1..=MAX_STATS).contains(&stats.len()) {
        bail!("a stats slide takes 1 to {MAX_STATS} stats, not {}", stats.len());
    }
    Ok(stats)
}

fn steps(v: &Value) -> Result<Vec<Step>> {
    let Value::Array(items) = v else {
        bail!("a steps slide needs steps: [{{title, text}}, ...]");
    };
    let steps = items
        .iter()
        .map(|s| match s {
            Value::Object(_) => {
                let title = opt(s, &["title", "heading", "name"])
                    .ok_or_else(|| anyhow!("each step needs a title"))?;
                Ok(Step { title, text: opt(s, &["text", "body", "description"]).unwrap_or_default() })
            }
            other => match as_text(other).map(|t| line(&t)).filter(|t| !t.is_empty()) {
                Some(title) => Ok(Step { title, text: String::new() }),
                None => bail!("each step is {{title, text}}"),
            },
        })
        .collect::<Result<Vec<_>>>()?;
    if !(2..=MAX_STEPS).contains(&steps.len()) {
        bail!("a steps slide takes 2 to {MAX_STEPS} steps, not {}", steps.len());
    }
    Ok(steps)
}

fn table(v: &Value) -> Result<Table> {
    let cells = |row: &Value| -> Result<Vec<String>> {
        match row {
            Value::Array(cells) => Ok(cells
                .iter()
                .map(|c| line(&as_text(c).unwrap_or_default()).replace("**", ""))
                .collect()),
            _ => bail!("a table row is an array of cells"),
        }
    };
    let headers = match &v["headers"] {
        Value::Null => Vec::new(),
        h => cells(h)?,
    };
    let rows = match &v["rows"] {
        Value::Array(rows) => rows.iter().map(cells).collect::<Result<Vec<_>>>()?,
        Value::Null => Vec::new(),
        _ => bail!("a table's rows is an array of rows, each an array of cells"),
    };
    if rows.is_empty() {
        bail!("a table slide needs table: {{headers: [...], rows: [[...], ...]}}");
    }
    let columns = rows.iter().map(Vec::len).chain([headers.len()]).max().unwrap_or(0);
    if !(1..=MAX_TABLE_COLUMNS).contains(&columns) {
        bail!("a table takes 1 to {MAX_TABLE_COLUMNS} columns, not {columns}");
    }
    if rows.len() > MAX_TABLE_ROWS {
        bail!(
            "{} rows don't fit on one slide (at most {MAX_TABLE_ROWS}); show the rows that \
             matter, or split the table over two slides",
            rows.len()
        );
    }
    Ok(Table { headers, rows, columns })
}

fn image_url(v: &Value) -> Result<Option<String>> {
    let Some(url) = opt(v, &["imageUrl", "image_url", "image"]) else {
        return Ok(None);
    };
    let ok = reqwest::Url::parse(&url)
        .is_ok_and(|u| u.scheme() == "https" && u.host_str().is_some_and(|h| h.contains('.')));
    if !ok || url.len() > 2000 {
        bail!(
            "imageUrl must be a public https link to a PNG, JPEG or GIF (Google fetches it \
             itself, so a file in the workspace won't do)"
        );
    }
    Ok(Some(url))
}

fn parse_slide(v: &Value, may_be_cover: bool) -> Result<Slide> {
    if !v.is_object() {
        bail!("a slide is an object with a layout and its content");
    }
    let title = opt(v, &["title", "heading"]).unwrap_or_default();
    let subtitle = opt(v, &["subtitle"]);
    let block = block(v)?;
    let image = image_url(v)?;
    let layout = match opt(v, &["layout", "type"]) {
        Some(l) => l.to_ascii_lowercase().replace(['-', ' '], "_"),
        // No layout named: what the slide holds says which it is.
        None => if !v["quote"].is_null() {
            "quote"
        } else if !v["stats"].is_null() {
            "stats"
        } else if !v["table"].is_null() {
            "table"
        } else if !v["steps"].is_null() {
            "steps"
        } else if !v["columns"].is_null() {
            "columns"
        } else if image.is_some() {
            "image"
        } else if may_be_cover && block.is_empty() {
            "title"
        } else {
            "bullets"
        }
        .to_string(),
    };
    let need_title = |what: &str| {
        if title.is_empty() {
            bail!("a {what} slide needs a title");
        }
        Ok(())
    };
    let content = match layout.as_str() {
        "title" | "cover" => {
            need_title("title")?;
            Content::Title { kicker: opt(v, &["kicker"]), subtitle, meta: opt(v, &["meta"]) }
        }
        "section" | "divider" => {
            need_title("section")?;
            Content::Section { kicker: opt(v, &["kicker"]), subtitle }
        }
        "closing" | "end" | "thanks" => {
            need_title("closing")?;
            Content::Closing { subtitle, meta: opt(v, &["meta"]) }
        }
        "bullets" | "content" | "text" | "agenda" => {
            need_title("bullets")?;
            if block.is_empty() {
                bail!("a bullets slide needs bullets or body; for a slide that is only a heading use the section layout");
            }
            Content::Bullets { block }
        }
        "columns" | "two_columns" | "comparison" => {
            need_title("columns")?;
            Content::Columns { columns: columns(&v["columns"])? }
        }
        "stats" | "numbers" | "metrics" => {
            need_title("stats")?;
            Content::Stats {
                stats: stats(&v["stats"])?,
                caption: opt(v, &["caption", "body", "text"]),
            }
        }
        "quote" => Content::Quote {
            quote: opt(v, &["quote", "body", "text"])
                .map(|q| q.replace("**", ""))
                .ok_or_else(|| anyhow!("a quote slide needs quote"))?,
            attribution: opt(v, &["attribution", "author", "source"]),
        },
        "table" => {
            need_title("table")?;
            Content::Table { table: table(&v["table"])?, caption: opt(v, &["caption"]) }
        }
        "image" | "picture" => Content::Image {
            url: image.ok_or_else(|| anyhow!("an image slide needs imageUrl"))?,
            caption: opt(v, &["caption"]),
            block,
        },
        "steps" | "process" | "timeline" => {
            need_title("steps")?;
            Content::Steps { steps: steps(&v["steps"])? }
        }
        other => bail!("unknown layout {other:?}; the layouts are {}", LAYOUTS.join(", ")),
    };
    Ok(Slide { title, content, notes: opt_paragraphs(v, &["notes", "speakerNotes"]) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn one(slide: Value) -> Result<Slide> {
        Deck::parse(&json!({ "slides": [{ "title": "Cover" }, slide] }), true)
            .map(|mut d| d.slides.remove(1))
    }

    #[test]
    fn bold_spans_are_counted_in_utf16() {
        let r = Rich::parse("Up **18%** – naïve 🚀 **go**");
        assert_eq!(r.text, "Up 18% – naïve 🚀 go");
        // The rocket is two UTF-16 units.
        assert_eq!(r.bold, [(3, 6), (18, 20)]);
        assert_eq!(Rich::parse("a ** b").text, "a  b");
        assert!(Rich::parse("a ** b").bold.is_empty());
        assert!(Rich::parse("****").bold.is_empty());
    }

    #[test]
    fn layouts_are_inferred_from_what_a_slide_holds() {
        let deck = Deck::parse(
            &json!({ "footer": " Acme  Inc ", "slides": [
                { "title": "Q3 review", "subtitle": "Board meeting" },
                { "title": "Highlights", "bullets": ["- One", "• Two"] },
                { "quote": "Ship it.", "attribution": "Ana" },
                { "title": "Numbers", "stats": [{ "value": 42, "label": "customers" }] },
                { "title": "Plan", "steps": ["Build", { "title": "Ship", "text": "In May" }] },
                { "title": "Look", "imageUrl": "https://example.com/a.png" },
            ] }),
            true,
        )
        .unwrap();
        assert_eq!(deck.footer.as_deref(), Some("Acme Inc"));
        let kinds: Vec<&str> = deck
            .slides
            .iter()
            .map(|s| match s.content {
                Content::Title { .. } => "title",
                Content::Bullets { .. } => "bullets",
                Content::Quote { .. } => "quote",
                Content::Stats { .. } => "stats",
                Content::Steps { .. } => "steps",
                Content::Image { .. } => "image",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["title", "bullets", "quote", "stats", "steps", "image"]);
        let Content::Bullets { block } = &deck.slides[1].content else { unreachable!() };
        assert_eq!(block.bullets[0].text.text, "One");
        assert_eq!(block.bullets[1].text.text, "Two");
        let Content::Stats { stats, .. } = &deck.slides[3].content else { unreachable!() };
        assert_eq!(stats[0].value, "42");
        // A first slide with only a title is a cover only where a deck starts.
        let appended = Deck::parse(&json!({ "slides": [{ "title": "More" }] }), false);
        assert!(appended.unwrap_err().to_string().contains("needs bullets or body"));
    }

    #[test]
    fn bullets_nest_one_level() {
        let slide = one(json!({ "title": "T", "bullets": [
            "  orphan", { "text": "Parent", "sub": ["Child **one**", ""] }, "  indented", 7,
        ] }))
        .unwrap();
        let Content::Bullets { block } = slide.content else { unreachable!() };
        let seen: Vec<(&str, u8)> =
            block.bullets.iter().map(|b| (b.text.text.as_str(), b.level)).collect();
        assert_eq!(
            seen,
            [("orphan", 0), ("Parent", 0), ("Child one", 1), ("indented", 1), ("7", 0)]
        );
        assert_eq!(block.bullets[2].text.bold, [(6, 9)]);
    }

    #[test]
    fn what_cannot_be_laid_out_is_refused_with_the_slide_number() {
        let err = |slide: Value| one(slide).unwrap_err().to_string();
        assert_eq!(
            err(json!({ "layout": "poster", "title": "T" })),
            format!("slide 2: unknown layout \"poster\"; the layouts are {}", LAYOUTS.join(", "))
        );
        assert!(err(json!({ "layout": "bullets", "bullets": ["a"] })).contains("needs a title"));
        let many: Vec<String> = (0..15).map(|i| format!("point {i}")).collect();
        assert!(err(json!({ "title": "T", "bullets": many })).contains("split them"));
        assert!(err(json!({ "title": "T", "columns": [{ "heading": "A" }] })).contains("2 to 4"));
        assert!(err(json!({ "title": "T", "stats": [{ "label": "x" }] })).contains("needs a value"));
        assert!(err(json!({ "title": "T", "table": { "headers": ["a"] } })).contains("rows"));
        let rows: Vec<Value> = (0..13).map(|i| json!([i])).collect();
        assert!(err(json!({ "title": "T", "table": { "rows": rows } })).contains("at most 12"));
        for url in ["http://example.com/a.png", "/workspace/chart.png", "https://localhost/a.png"] {
            assert!(err(json!({ "title": "T", "imageUrl": url })).contains("public https"), "{url}");
        }
        assert!(Deck::parse(&json!({}), true).unwrap_err().to_string().contains("required"));
        let too_many: Vec<Value> = (0..61).map(|_| json!({ "title": "T" })).collect();
        assert!(Deck::parse(&json!({ "slides": too_many }), true).is_err());
    }

    #[test]
    fn tables_are_squared_up() {
        let slide = one(json!({ "title": "T", "table": {
            "headers": ["Region", "Revenue"], "rows": [["EMEA", 1200, "extra"], ["**APAC**"]],
        } }))
        .unwrap();
        let Content::Table { table, .. } = slide.content else { unreachable!() };
        assert_eq!(table.columns, 3);
        assert_eq!(table.rows[0], ["EMEA", "1200", "extra"]);
        assert_eq!(table.rows[1], ["APAC"]);
    }
}
