//! Laying a deck out: every slide becomes the Slides API requests that draw it on a blank
//! 16:9 page, positioned on one grid and coloured from one [`Theme`].
//!
//! The API can't shrink text to fit a box, so sizes are chosen here from an estimate of how the
//! text wraps; where even the smallest size doesn't fit, the slide is flagged so the model can
//! split it.

use super::deck::{Block, Column, Content, Deck, Rich, Slide, Stat, Step, Table};
use super::theme::{Color, Theme};
use serde_json::{json, Value};

/// The page, in points: 10 by 5.625 inches.
pub const PAGE_W: f64 = 720.0;
pub const PAGE_H: f64 = 405.0;
/// The left and right margin, which every slide's text starts at.
const MARGIN: f64 = 48.0;
const CONTENT_W: f64 = PAGE_W - 2.0 * MARGIN;
/// Where the body of a content slide starts, under its title.
const BODY_TOP: f64 = 124.0;
/// The padding a Slides text box keeps around its text, on every side.
const INSET: f64 = 7.2;
/// How far Slides indents a bullet's text, per level.
const BULLET_INDENT: f64 = 36.0;
const LINE_HEIGHT: f64 = 1.2;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { x, y, w, h }
}

#[derive(Clone, Copy)]
enum Align {
    Start,
    Center,
}

#[derive(Clone, Copy)]
enum Anchor {
    Top,
    Middle,
    Bottom,
}

#[derive(Clone, Copy)]
struct Style<'a> {
    font: &'a str,
    size: f64,
    bold: bool,
    color: Color,
    align: Align,
    anchor: Anchor,
    /// Line spacing, as a multiple of single spacing.
    spacing: f64,
    /// Space under each paragraph, in points.
    gap: f64,
}

/// What a deck becomes.
#[derive(Debug, Default)]
pub struct Rendered {
    /// One group of requests per slide, in order; a group is never split over two batches.
    pub slides: Vec<Vec<Value>>,
    /// The object ID of each slide, for finding its notes page.
    pub slide_ids: Vec<String>,
    /// Speaker notes by slide ID. They go in a second pass: a notes box only exists once its
    /// slide does.
    pub notes: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

/// Lay `deck` out. `prefix` starts every object ID (IDs must be unique in a presentation, so it
/// differs per call); `insert_at` is the zero-based position of the first slide, or `None` to
/// add them at the end.
pub fn render(deck: &Deck, theme: &Theme, prefix: &str, insert_at: Option<usize>) -> Rendered {
    let mut out = Rendered::default();
    let mut sections = 0;
    for (i, slide) in deck.slides.iter().enumerate() {
        let id = format!("{prefix}_s{:02}", i + 1);
        let mut create = json!({ "objectId": id,
            "slideLayoutReference": { "predefinedLayout": "BLANK" } });
        if let Some(at) = insert_at {
            create["insertionIndex"] = json!(at + i);
        }
        let mut page = Page {
            theme,
            id: id.clone(),
            elements: 0,
            requests: vec![json!({ "createSlide": create })],
            warnings: Vec::new(),
        };
        if matches!(slide.content, Content::Section { .. }) {
            sections += 1;
        }
        page.slide(slide, deck.footer.as_deref(), sections);
        out.warnings
            .extend(page.warnings.into_iter().map(|w| format!("slide {}: {w}", i + 1)));
        out.slides.push(page.requests);
        if let Some(notes) = &slide.notes {
            out.notes.push((id.clone(), notes.clone()));
        }
        out.slide_ids.push(id);
    }
    out
}

/// How wide `text` runs on one line at `size`, estimated from the shapes of its characters. A
/// little generous, so a line that is said to fit does.
fn text_width(text: &str, size: f64, bold: bool) -> f64 {
    let em: f64 = text
        .chars()
        .map(|c| match c {
            ' ' | 'i' | 'j' | 'l' | 'I' | '.' | ',' | ':' | ';' | '\'' | '!' | '|' => 0.29,
            'f' | 't' | 'r' | '(' | ')' | '[' | ']' | '-' | '"' | '/' => 0.38,
            'm' | 'w' | 'M' | 'W' | '%' | '@' => 0.88,
            c if c.is_ascii_uppercase() => 0.68,
            c if c.is_ascii_digit() => 0.6,
            c if c.is_ascii() => 0.55,
            // CJK and emoji fill the em square.
            c if c as u32 >= 0x2E80 => 1.0,
            _ => 0.58,
        })
        .sum();
    em * size * if bold { 1.06 } else { 1.0 }
}

/// How many lines `text` takes when wrapped at `width`, word by word, one or more per paragraph.
fn lines(text: &str, size: f64, bold: bool, width: f64) -> usize {
    let space = text_width(" ", size, bold);
    text.split('\n')
        .map(|paragraph| {
            let mut count = 1;
            let mut used = 0.0;
            for word in paragraph.split_whitespace() {
                let w = text_width(word, size, bold);
                if used > 0.0 && used + space + w > width {
                    count += 1;
                    used = 0.0;
                }
                if used > 0.0 {
                    used += space;
                }
                used += w;
                // A word longer than the line breaks inside itself.
                while used > width && width > 0.0 {
                    count += 1;
                    used -= width;
                }
            }
            count
        })
        .sum()
}

fn text_height(text: &str, size: f64, bold: bool, width: f64, spacing: f64) -> f64 {
    lines(text, size, bold, width) as f64 * size * LINE_HEIGHT * spacing
}

/// The space under a paragraph of body text.
fn paragraph_gap(size: f64) -> f64 {
    (size * 0.5).round()
}

const BODY_SPACING: f64 = 1.15;

/// How tall a block of running text is at `size` in a column `width` wide.
fn block_height(block: &Block, size: f64, width: f64) -> f64 {
    let gap = paragraph_gap(size);
    let mut h = 0.0;
    if let Some(p) = &block.paragraph {
        let paragraphs = p.text.split('\n').count() as f64;
        h += text_height(&p.text, size, false, width, BODY_SPACING) + paragraphs * gap;
    }
    for b in &block.bullets {
        let w = width - BULLET_INDENT * (f64::from(b.level) + 1.0);
        h += text_height(&b.text.text, size, false, w, BODY_SPACING) + gap;
    }
    (h - gap).max(0.0)
}

/// The first of `sizes` (largest first) at which `height(size)` is within `room`, else the last.
fn fit(sizes: &[f64], room: f64, height: impl Fn(f64) -> f64) -> (f64, bool) {
    for &size in sizes {
        if height(size) <= room {
            return (size, true);
        }
    }
    (*sizes.last().expect("sizes are given"), false)
}

fn pt(v: f64) -> Value {
    json!({ "magnitude": (v * 100.0).round() / 100.0, "unit": "PT" })
}

fn solid(color: Color) -> Value {
    json!({ "solidFill": { "color": { "rgbColor": color.rgb() } } })
}

fn units(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Whether a table cell reads as a number ("1,200", "-4.5%", "$3.2M"), to line a column of them
/// up on the right.
fn numeric(cell: &str) -> bool {
    let core: String = cell
        .chars()
        .filter(|c| !matches!(c, ' ' | ',' | '%' | '$' | '€' | '£' | '+' | '-' | '−' | '(' | ')'))
        .collect();
    let core = core.trim_end_matches(['k', 'K', 'M', 'B', 'x', '×']);
    !core.is_empty()
        && core.chars().any(|c| c.is_ascii_digit())
        && core.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// One slide being drawn.
struct Page<'a> {
    theme: &'a Theme,
    id: String,
    elements: usize,
    requests: Vec<Value>,
    warnings: Vec<String>,
}

impl<'a> Page<'a> {
    fn next_id(&mut self) -> String {
        self.elements += 1;
        format!("{}_e{:02}", self.id, self.elements)
    }

    fn element(&self, r: Rect) -> Value {
        json!({
            "pageObjectId": self.id,
            "size": { "width": pt(r.w), "height": pt(r.h) },
            "transform": { "scaleX": 1, "scaleY": 1,
                "translateX": (r.x * 100.0).round() / 100.0,
                "translateY": (r.y * 100.0).round() / 100.0, "unit": "PT" },
        })
    }

    fn background(&mut self, color: Color) {
        self.requests.push(json!({ "updatePageProperties": {
            "objectId": self.id,
            "pageProperties": { "pageBackgroundFill": solid(color) },
            "fields": "pageBackgroundFill.solidFill.color",
        } }));
    }

    /// A filled shape with no outline: a card, a rule, a dot.
    fn shape(&mut self, kind: &str, r: Rect, fill: Color) -> String {
        let id = self.next_id();
        self.requests.push(json!({ "createShape": {
            "objectId": id, "shapeType": kind, "elementProperties": self.element(r),
        } }));
        self.requests.push(json!({ "updateShapeProperties": {
            "objectId": id,
            "shapeProperties": {
                "shapeBackgroundFill": solid(fill),
                "outline": { "propertyState": "NOT_RENDERED" },
            },
            "fields": "shapeBackgroundFill.solidFill.color,outline.propertyState",
        } }));
        id
    }

    fn fill(&mut self, r: Rect, color: Color) {
        self.shape("RECTANGLE", r, color);
    }

    fn text_style(style: &Style, color: Color, bold: bool) -> Value {
        json!({
            "fontFamily": style.font,
            "fontSize": pt(style.size),
            "bold": bold,
            "foregroundColor": { "opaqueColor": { "rgbColor": color.rgb() } },
        })
    }

    /// A text box whose text sits exactly in `r`: the box itself is larger by Slides' padding,
    /// so text lines up with the shapes around it. Returns the box's ID; nothing is drawn for
    /// empty text, which the API refuses.
    fn text(&mut self, r: Rect, text: &str, style: Style) -> Option<String> {
        if text.trim().is_empty() {
            return None;
        }
        let id = self.next_id();
        let outer = rect(r.x - INSET, r.y - INSET, r.w + 2.0 * INSET, r.h + 2.0 * INSET);
        self.requests.push(json!({ "createShape": {
            "objectId": id, "shapeType": "TEXT_BOX", "elementProperties": self.element(outer),
        } }));
        self.requests
            .push(json!({ "insertText": { "objectId": id, "insertionIndex": 0, "text": text } }));
        self.requests.push(json!({ "updateTextStyle": {
            "objectId": id, "textRange": { "type": "ALL" },
            "style": Self::text_style(&style, style.color, style.bold),
            "fields": "fontFamily,fontSize,bold,foregroundColor",
        } }));
        self.requests.push(json!({ "updateParagraphStyle": {
            "objectId": id, "textRange": { "type": "ALL" },
            "style": {
                "alignment": match style.align {
                    Align::Start => "START",
                    Align::Center => "CENTER",
                },
                "lineSpacing": (style.spacing * 100.0).round(),
                "spaceAbove": pt(0.0),
                "spaceBelow": pt(style.gap),
            },
            "fields": "alignment,lineSpacing,spaceAbove,spaceBelow",
        } }));
        self.requests.push(json!({ "updateShapeProperties": {
            "objectId": id,
            "shapeProperties": { "contentAlignment": match style.anchor {
                Anchor::Top => "TOP",
                Anchor::Middle => "MIDDLE",
                Anchor::Bottom => "BOTTOM",
            } },
            "fields": "contentAlignment",
        } }));
        Some(id)
    }

    fn style_range(&mut self, id: &str, range: (usize, usize), style: Value, fields: &str) {
        if range.1 <= range.0 {
            return;
        }
        self.requests.push(json!({ "updateTextStyle": {
            "objectId": id,
            "textRange": { "type": "FIXED_RANGE", "startIndex": range.0, "endIndex": range.1 },
            "style": style, "fields": fields,
        } }));
    }

    /// Running text in `r`: the paragraph, then the bullets as a real bulleted list (so the
    /// deck's owner can go on editing it as one), sub-points indented and quieter.
    fn block(&mut self, r: Rect, block: &Block, size: f64, color: Color, sub: Color) {
        let mut text = String::new();
        let mut bold: Vec<(usize, usize)> = Vec::new();
        let mut quiet: Vec<(usize, usize)> = Vec::new();
        let mut at = 0;
        let mut bullets_from = None;
        let mut push = |piece: &Rich, tabs: usize, text: &mut String| -> (usize, usize) {
            if !text.is_empty() {
                text.push('\n');
                at += 1;
            }
            let start = at;
            text.push_str(&"\t".repeat(tabs));
            text.push_str(&piece.text);
            let from = start + tabs;
            bold.extend(piece.bold.iter().map(|(a, b)| (from + a, from + b)));
            at = from + units(&piece.text);
            (start, at)
        };
        if let Some(p) = &block.paragraph {
            push(p, 0, &mut text);
        }
        for b in &block.bullets {
            // Leading tabs are how the API is told a bullet's level; it removes them when it
            // makes the list, which is why that request comes last.
            let (start, end) = push(&b.text, usize::from(b.level), &mut text);
            bullets_from.get_or_insert(start);
            if b.level > 0 {
                quiet.push((start, end));
            }
        }
        let total = at;
        let style = Style {
            font: self.theme.body_font,
            size,
            bold: false,
            color,
            align: Align::Start,
            anchor: Anchor::Top,
            spacing: BODY_SPACING,
            gap: paragraph_gap(size),
        };
        let Some(id) = self.text(r, &text, style) else {
            return;
        };
        for range in quiet {
            let style = json!({ "foregroundColor": { "opaqueColor": { "rgbColor": sub.rgb() } } });
            self.style_range(&id, range, style, "foregroundColor");
        }
        for range in bold {
            self.style_range(&id, range, json!({ "bold": true }), "bold");
        }
        if let Some(start) = bullets_from {
            self.requests.push(json!({ "createParagraphBullets": {
                "objectId": id,
                "textRange": { "type": "FIXED_RANGE", "startIndex": start, "endIndex": total },
                "bulletPreset": "BULLET_DISC_CIRCLE_SQUARE",
            } }));
        }
    }

    fn heading(&self, size: f64, color: Color, anchor: Anchor) -> Style<'a> {
        Style {
            font: self.theme.heading_font,
            size,
            bold: self.theme.heading_bold,
            color,
            align: Align::Start,
            anchor,
            spacing: 1.0,
            gap: 0.0,
        }
    }

    fn body(&self, size: f64, color: Color) -> Style<'a> {
        Style {
            font: self.theme.body_font,
            size,
            bold: false,
            color,
            align: Align::Start,
            anchor: Anchor::Top,
            spacing: BODY_SPACING,
            gap: 0.0,
        }
    }

    fn overflows(&mut self, what: &str) {
        self.warnings.push(format!(
            "{what} may run past its space even at the smallest size; shorten it or split the slide"
        ));
    }

    fn slide(&mut self, slide: &Slide, footer: Option<&str>, section: usize) {
        match &slide.content {
            Content::Title { kicker, subtitle, meta } => {
                self.cover(&slide.title, kicker.as_deref(), subtitle.as_deref(), meta.as_deref())
            }
            Content::Closing { subtitle, meta } => {
                self.cover(&slide.title, None, subtitle.as_deref(), meta.as_deref())
            }
            Content::Section { kicker, subtitle } => {
                let number = format!("{section:02}");
                self.section(&slide.title, kicker.as_deref().unwrap_or(&number), subtitle.as_deref())
            }
            content => {
                let body = self.frame(&slide.title, footer);
                match content {
                    Content::Bullets { block } => self.bullets(body, block),
                    Content::Columns { columns } => self.columns(body, columns),
                    Content::Stats { stats, caption } => self.stats(body, stats, caption.as_deref()),
                    Content::Quote { quote, attribution } => {
                        self.quote(body, quote, attribution.as_deref())
                    }
                    Content::Table { table, caption } => self.table(body, table, caption.as_deref()),
                    Content::Image { url, caption, block } => {
                        self.image(body, url, caption.as_deref(), block)
                    }
                    Content::Steps { steps } => self.steps(body, steps),
                    Content::Title { .. } | Content::Closing { .. } | Content::Section { .. } => {
                        unreachable!("covers are drawn above")
                    }
                }
            }
        }
    }

    /// The title and closing slides: the deck's name large on the cover colour, low on the page,
    /// with a short accent rule between it and the line under it.
    fn cover(&mut self, title: &str, kicker: Option<&str>, subtitle: Option<&str>, meta: Option<&str>) {
        let t = self.theme;
        self.background(t.cover_bg);
        let room = 132.0;
        let (size, fits) = fit(&[46.0, 40.0, 34.0, 28.0, 24.0], room, |s| {
            text_height(title, s, t.heading_bold, CONTENT_W, 1.0)
        });
        if !fits {
            self.overflows("the title");
        }
        if let Some(kicker) = kicker {
            let style = Style { bold: true, ..self.body(11.0, t.cover_accent) };
            self.text(rect(MARGIN, 92.0, CONTENT_W, 16.0), &kicker.to_uppercase(), style);
        }
        let style = self.heading(size, t.cover_text, Anchor::Bottom);
        self.text(rect(MARGIN, 116.0, CONTENT_W, room), title, style);
        self.fill(rect(MARGIN, 262.0, 48.0, 4.0), t.cover_accent);
        if let Some(subtitle) = subtitle {
            let (size, _) = fit(&[18.0, 16.0, 14.0], 50.0, |s| {
                text_height(subtitle, s, false, 560.0, BODY_SPACING)
            });
            let style = self.body(size, t.cover_muted);
            self.text(rect(MARGIN, 282.0, 560.0, 50.0), subtitle, style);
        }
        if let Some(meta) = meta {
            let style = self.body(11.0, t.cover_muted);
            self.text(rect(MARGIN, 354.0, CONTENT_W, 16.0), meta, style);
        }
    }

    /// A section divider: its number, its name, and a band of accent down the left edge.
    fn section(&mut self, title: &str, kicker: &str, subtitle: Option<&str>) {
        let t = self.theme;
        self.background(t.cover_bg);
        self.fill(rect(0.0, 0.0, 10.0, PAGE_H), t.cover_accent);
        let style = Style { bold: true, ..self.body(14.0, t.cover_accent) };
        self.text(rect(MARGIN, 128.0, CONTENT_W, 20.0), &kicker.to_uppercase(), style);
        let room = 96.0;
        let (size, fits) = fit(&[38.0, 32.0, 26.0, 22.0], room, |s| {
            text_height(title, s, t.heading_bold, CONTENT_W, 1.0)
        });
        if !fits {
            self.overflows("the title");
        }
        let height = text_height(title, size, t.heading_bold, CONTENT_W, 1.0).min(room);
        let style = self.heading(size, t.cover_text, Anchor::Top);
        self.text(rect(MARGIN, 158.0, CONTENT_W, room), title, style);
        if let Some(subtitle) = subtitle {
            let style = self.body(16.0, t.cover_muted);
            self.text(rect(MARGIN, 158.0 + height + 14.0, 560.0, 48.0), subtitle, style);
        }
    }

    /// What every content slide shares: the background, the title sitting on a short accent
    /// rule, and the footer. Returns the space left for the slide's content.
    fn frame(&mut self, title: &str, footer: Option<&str>) -> Rect {
        let t = self.theme;
        self.background(t.bg);
        let bottom = if footer.is_some() { 360.0 } else { PAGE_H - 36.0 };
        if let Some(footer) = footer {
            let style = Style { anchor: Anchor::Middle, ..self.body(9.0, t.muted) };
            self.text(rect(MARGIN, 374.0, CONTENT_W, 14.0), footer, style);
        }
        if title.is_empty() {
            return rect(MARGIN, 52.0, CONTENT_W, bottom - 52.0);
        }
        let room = 60.0;
        let (size, fits) = fit(&[28.0, 24.0, 20.0], room, |s| {
            // One line at the large size reads better than two; two are fine below it.
            let h = text_height(title, s, t.heading_bold, CONTENT_W, 1.0);
            if s == 28.0 && h > 28.0 * LINE_HEIGHT { f64::MAX } else { h }
        });
        if !fits {
            self.overflows("the title");
        }
        let style = self.heading(size, t.text, Anchor::Bottom);
        self.text(rect(MARGIN, 32.0, CONTENT_W, room), title, style);
        self.fill(rect(MARGIN, 101.0, 36.0, 3.0), t.accent);
        rect(MARGIN, BODY_TOP, CONTENT_W, bottom - BODY_TOP)
    }

    fn bullets(&mut self, body: Rect, block: &Block) {
        let t = self.theme;
        let (size, fits) =
            fit(&[20.0, 18.0, 16.0, 14.0, 12.0], body.h, |s| block_height(block, s, body.w));
        if !fits {
            self.overflows("the text");
        }
        self.block(body, block, size, t.text, t.muted);
    }

    /// Columns as cards side by side, each with an accent edge on top, sharing one text size.
    fn columns(&mut self, body: Rect, columns: &[Column]) {
        let t = self.theme;
        let n = columns.len() as f64;
        let gap = 16.0;
        let pad = 16.0;
        let w = (body.w - gap * (n - 1.0)) / n;
        let inner = w - 2.0 * pad;
        let heading_size = 15.0;
        let heading_h = columns
            .iter()
            .filter_map(|c| c.heading.as_deref())
            .map(|h| text_height(h, heading_size, true, inner, 1.0))
            .fold(0.0, f64::max);
        let top = pad + 3.0 + if heading_h > 0.0 { heading_h + 10.0 } else { 0.0 };
        let room = body.h - top - pad;
        let (size, fits) = fit(&[14.0, 13.0, 12.0, 11.0, 10.0], room, |s| {
            columns.iter().map(|c| block_height(&c.block, s, inner)).fold(0.0, f64::max)
        });
        if !fits {
            self.overflows("a column's text");
        }
        for (i, column) in columns.iter().enumerate() {
            let x = body.x + (w + gap) * i as f64;
            self.fill(rect(x, body.y, w, body.h), t.surface);
            self.fill(rect(x, body.y, w, 3.0), t.accent);
            if let Some(heading) = &column.heading {
                let style = Style {
                    bold: true,
                    spacing: 1.0,
                    anchor: Anchor::Bottom,
                    ..self.body(heading_size, t.text)
                };
                self.text(rect(x + pad, body.y + pad + 3.0, inner, heading_h), heading, style);
            }
            self.block(rect(x + pad, body.y + top, inner, room), &column.block, size, t.text, t.muted);
        }
    }

    /// A few numbers, each large in the accent colour on its own card with what it counts.
    fn stats(&mut self, body: Rect, stats: &[Stat], caption: Option<&str>) {
        let t = self.theme;
        let n = stats.len() as f64;
        let gap = 16.0;
        let pad = 18.0;
        let w = (body.w - gap * (n - 1.0)) / n;
        let inner = w - 2.0 * pad;
        let card_h = 176.0;
        let caption_h = if caption.is_some() { 44.0 } else { 0.0 };
        let y = body.y + ((body.h - caption_h - card_h) / 2.0).clamp(0.0, 28.0);
        // One size for every number, set by the longest.
        let (size, _) = fit(&[52.0, 46.0, 40.0, 34.0, 28.0, 24.0], inner, |s| {
            stats.iter().map(|st| text_width(&st.value, s, t.heading_bold)).fold(0.0, f64::max)
        });
        // And one size for every label, set by the longest.
        let room = card_h - 104.0 - 12.0;
        let (label_size, fits) = fit(&[13.0, 12.0, 11.0, 10.0], room, |s| {
            stats
                .iter()
                .map(|st| text_height(&st.label, s, false, inner, BODY_SPACING))
                .fold(0.0, f64::max)
        });
        if !fits {
            self.overflows("a stat's label");
        }
        for (i, stat) in stats.iter().enumerate() {
            let x = body.x + (w + gap) * i as f64;
            self.fill(rect(x, y, w, card_h), t.surface);
            let style = self.heading(size, t.accent, Anchor::Bottom);
            self.text(rect(x + pad, y + 14.0, inner, 70.0), &stat.value, style);
            self.fill(rect(x + pad, y + 92.0, 24.0, 2.0), t.accent);
            let style = self.body(label_size, t.muted);
            self.text(rect(x + pad, y + 104.0, inner, room), &stat.label, style);
        }
        if let Some(caption) = caption {
            let style = self.body(13.0, t.muted);
            self.text(rect(body.x, y + card_h + 18.0, body.w, caption_h - 8.0), caption, style);
        }
    }

    /// A quotation set large beside an accent bar, with who said it under it.
    fn quote(&mut self, body: Rect, quote: &str, attribution: Option<&str>) {
        let t = self.theme;
        let quoted = if quote.starts_with(['"', '“', '«', '„']) {
            quote.to_string()
        } else {
            format!("“{quote}”")
        };
        let attribution_h = if attribution.is_some() { 34.0 } else { 0.0 };
        let x = body.x + 24.0;
        let w = body.w - 24.0 - 36.0;
        let room = (body.h - attribution_h).min(190.0);
        let (size, fits) = fit(&[28.0, 24.0, 20.0, 17.0, 15.0], room, |s| {
            text_height(&quoted, s, false, w, BODY_SPACING)
        });
        if !fits {
            self.overflows("the quote");
        }
        let height = text_height(&quoted, size, false, w, BODY_SPACING).min(room);
        let y = body.y + ((body.h - attribution_h - height) / 2.0).max(0.0);
        self.fill(rect(body.x, y + 3.0, 4.0, height - 4.0), t.accent);
        let style = Style {
            bold: false,
            spacing: BODY_SPACING,
            ..self.heading(size, t.text, Anchor::Top)
        };
        self.text(rect(x, y, w, height), &quoted, style);
        if let Some(attribution) = attribution {
            let line = format!("— {}", attribution.trim_start_matches(['—', '–', '-', ' ']));
            let style = self.body(13.0, t.muted);
            self.text(rect(x, y + height + 14.0, w, 20.0), &line, style);
        }
    }

    /// A table ruled only between its rows, the header set off by a heavier rule, numbers lined
    /// up on the right.
    fn table(&mut self, body: Rect, table: &Table, caption: Option<&str>) {
        let t = self.theme;
        let has_header = !table.headers.is_empty();
        let all: Vec<&Vec<String>> = has_header
            .then_some(&table.headers)
            .into_iter()
            .chain(&table.rows)
            .collect();
        let (rows, cols) = (all.len(), table.columns);
        let caption_h = if caption.is_some() { 26.0 } else { 0.0 };
        let row_h = ((body.h - caption_h) / rows as f64).clamp(17.0, 30.0);
        let size = if row_h >= 26.0 { 12.0 } else if row_h >= 21.0 { 11.0 } else { 10.0 };
        fn cell(row: &[String], c: usize) -> &str {
            row.get(c).map(String::as_str).unwrap_or("")
        }
        // Columns share the width by how much they hold, none squeezed under a floor.
        let need: Vec<f64> = (0..cols)
            .map(|c| {
                all.iter()
                    .map(|row| text_width(cell(row, c), size, true))
                    .fold(0.0, f64::max)
                    .clamp(36.0, 320.0)
                    + 16.0
            })
            .collect();
        let total: f64 = need.iter().sum();
        let widths: Vec<f64> = need.iter().map(|n| n / total * body.w).collect();
        let right: Vec<bool> = (0..cols)
            .map(|c| {
                let cells: Vec<&str> =
                    table.rows.iter().map(|r| cell(r, c)).filter(|s| !s.is_empty()).collect();
                !cells.is_empty() && cells.iter().all(|s| numeric(s))
            })
            .collect();
        if all.iter().any(|row| {
            (0..cols).any(|c| text_width(cell(row, c), size, false) > widths[c] - 12.0)
        }) {
            self.warnings.push(
                "some table cells wrap, so the table may run past the slide; shorten them or use \
                 fewer columns"
                    .into(),
            );
        }

        let id = self.next_id();
        let whole = json!({ "location": { "rowIndex": 0, "columnIndex": 0 },
            "rowSpan": rows, "columnSpan": cols });
        self.requests.push(json!({ "createTable": {
            "objectId": id, "rows": rows, "columns": cols,
            "elementProperties": self.element(rect(body.x, body.y, body.w, row_h * rows as f64)),
        } }));
        for (c, width) in widths.iter().enumerate() {
            self.requests.push(json!({ "updateTableColumnProperties": {
                "objectId": id, "columnIndices": [c],
                "tableColumnProperties": { "columnWidth": pt(*width) },
                "fields": "columnWidth",
            } }));
        }
        self.requests.push(json!({ "updateTableRowProperties": {
            "objectId": id, "rowIndices": (0..rows).collect::<Vec<_>>(),
            "tableRowProperties": { "minRowHeight": pt(row_h) },
            "fields": "minRowHeight",
        } }));
        self.requests.push(json!({ "updateTableCellProperties": {
            "objectId": id, "tableRange": whole,
            "tableCellProperties": { "contentAlignment": "MIDDLE" },
            "fields": "contentAlignment",
        } }));
        let border = |range: &Value, position: &str, color: Color, weight: f64| {
            json!({ "updateTableBorderProperties": {
                "objectId": id, "tableRange": range, "borderPosition": position,
                "tableBorderProperties": {
                    "tableBorderFill": solid(color), "weight": pt(weight), "dashStyle": "SOLID",
                },
                "fields": "tableBorderFill.solidFill.color,weight,dashStyle",
            } })
        };
        // Every border goes the colour of the page, then the rules between rows come back.
        self.requests.push(border(&whole, "ALL", t.bg, 1.0));
        if rows > 1 {
            self.requests.push(border(&whole, "INNER_HORIZONTAL", t.rule, 0.75));
        }
        self.requests.push(border(&whole, "BOTTOM", t.rule, 0.75));
        if has_header {
            let header = json!({ "location": { "rowIndex": 0, "columnIndex": 0 },
                "rowSpan": 1, "columnSpan": cols });
            self.requests.push(border(&header, "BOTTOM", t.text, 1.25));
        }
        for (r, row) in all.iter().enumerate() {
            let is_header = has_header && r == 0;
            for (c, &numbers) in right.iter().enumerate() {
                let text = cell(row, c);
                if text.is_empty() {
                    continue;
                }
                let at = json!({ "rowIndex": r, "columnIndex": c });
                self.requests.push(json!({ "insertText": {
                    "objectId": id, "cellLocation": at, "insertionIndex": 0, "text": text,
                } }));
                let style = self.body(size, t.text);
                self.requests.push(json!({ "updateTextStyle": {
                    "objectId": id, "cellLocation": at, "textRange": { "type": "ALL" },
                    "style": Self::text_style(&style, t.text, is_header),
                    "fields": "fontFamily,fontSize,bold,foregroundColor",
                } }));
                if numbers {
                    self.requests.push(json!({ "updateParagraphStyle": {
                        "objectId": id, "cellLocation": at, "textRange": { "type": "ALL" },
                        "style": { "alignment": "END" }, "fields": "alignment",
                    } }));
                }
            }
        }
        if let Some(caption) = caption {
            let style = self.body(10.0, t.muted);
            let y = (body.y + row_h * rows as f64 + 10.0).min(body.y + body.h - 14.0);
            self.text(rect(body.x, y, body.w, 14.0), caption, style);
        }
    }

    /// A picture: alone across the slide, or on the right with the text beside it.
    fn image(&mut self, body: Rect, url: &str, caption: Option<&str>, block: &Block) {
        let t = self.theme;
        let caption_h = if caption.is_some() { 22.0 } else { 0.0 };
        let frame = if block.is_empty() {
            rect(body.x, body.y, body.w, body.h - caption_h)
        } else {
            let gap = 28.0;
            let w = (body.w - gap) / 2.0;
            let (size, fits) =
                fit(&[18.0, 16.0, 14.0, 12.0], body.h, |s| block_height(block, s, w));
            if !fits {
                self.overflows("the text");
            }
            self.block(rect(body.x, body.y, w, body.h), block, size, t.text, t.muted);
            rect(body.x + w + gap, body.y, w, body.h - caption_h)
        };
        let id = self.next_id();
        // Slides scales the picture to fit the frame and centres it there.
        self.requests.push(json!({ "createImage": {
            "objectId": id, "url": url, "elementProperties": self.element(frame),
        } }));
        if let Some(caption) = caption {
            let style = self.body(10.0, t.muted);
            self.text(rect(frame.x, frame.y + frame.h + 8.0, frame.w, 14.0), caption, style);
        }
    }

    /// Steps in a row, numbered on a line that runs through them.
    fn steps(&mut self, body: Rect, steps: &[Step]) {
        let t = self.theme;
        let n = steps.len() as f64;
        let gap = 18.0;
        let w = (body.w - gap * (n - 1.0)) / n;
        let dot = 34.0;
        let y = body.y + 14.0;
        // The line runs from the first dot to the last, under them.
        self.fill(rect(body.x + dot / 2.0, y + dot / 2.0 - 1.0, (w + gap) * (n - 1.0), 2.0), t.rule);
        let title_size = if steps.len() > 4 { 13.0 } else { 15.0 };
        let title_h = steps
            .iter()
            .map(|s| text_height(&s.title, title_size, true, w, 1.0))
            .fold(0.0, f64::max);
        let text_y = y + dot + 18.0 + title_h + 8.0;
        let room = body.y + body.h - text_y;
        let (size, fits) = fit(&[13.0, 12.0, 11.0, 10.0], room, |s| {
            steps
                .iter()
                .map(|st| text_height(&st.text, s, false, w, BODY_SPACING))
                .fold(0.0, f64::max)
        });
        if !fits {
            self.overflows("a step's text");
        }
        for (i, step) in steps.iter().enumerate() {
            let x = body.x + (w + gap) * i as f64;
            self.shape("ELLIPSE", rect(x, y, dot, dot), t.accent);
            // The number sits in its own box over the dot: a dot's own text area is too small.
            let style = Style {
                bold: true,
                align: Align::Center,
                anchor: Anchor::Middle,
                spacing: 1.0,
                ..self.body(13.0, t.accent.on())
            };
            self.text(rect(x - 4.0, y + 8.0, dot + 8.0, dot - 16.0), &(i + 1).to_string(), style);
            let style = Style { bold: true, spacing: 1.0, ..self.body(title_size, t.text) };
            self.text(rect(x, y + dot + 18.0, w, title_h), &step.title, style);
            let style = self.body(size, t.muted);
            self.text(rect(x, text_y, w, room), &step.text, style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;

    fn deck(slides: Value) -> Deck {
        Deck::parse(&json!({ "slides": slides, "footer": "Acme" }), true).unwrap()
    }

    fn rendered(slides: Value) -> Rendered {
        render(&deck(slides), &Theme::named("minimal").unwrap(), "c1a2b3c4d", None)
    }

    fn every_layout() -> Value {
        json!([
            { "title": "Q3 business review", "subtitle": "Board meeting", "meta": "Ana · October 2026", "kicker": "Acme" },
            { "layout": "section", "title": "Where we are" },
            { "title": "Highlights", "body": "Three things **mattered**.", "bullets": [
                "Revenue up **18%**", { "text": "Churn down", "sub": ["Enterprise: 2%", "SMB: 5%"] }, "NPS at 61" ],
              "notes": "Open with the revenue number." },
            { "title": "Build or buy", "columns": [
                { "heading": "Build", "bullets": ["Full control", "Six months"] },
                { "heading": "Buy", "body": "Live in weeks." } ] },
            { "title": "The quarter in numbers", "caption": "All figures unaudited.", "stats": [
                { "value": "$4.2M", "label": "Revenue" }, { "value": "118%", "label": "Net retention" },
                { "value": "61", "label": "NPS" } ] },
            { "quote": "The best quarter we have had.", "attribution": "Ana, CEO" },
            { "title": "By region", "caption": "USD thousands", "table": { "headers": ["Region", "Revenue", "Growth"],
                "rows": [["EMEA", "1,200", "12%"], ["APAC", "950", "31%"], ["Americas", "2,050", ""]] } },
            { "title": "The new dashboard", "imageUrl": "https://example.com/shot.png", "caption": "Beta", "bullets": ["Faster", "Clearer"] },
            { "title": "Rollout", "steps": [{ "title": "Pilot", "text": "Two teams" }, { "title": "Launch", "text": "Everyone" }, "Review"] },
            { "layout": "closing", "title": "Thank you", "subtitle": "Questions?", "meta": "ana@example.com" },
        ])
    }

    fn all(r: &Rendered) -> Vec<&Value> {
        r.slides.iter().flatten().collect()
    }

    fn of<'a>(r: &'a Rendered, kind: &str) -> Vec<&'a Value> {
        all(r).into_iter().filter_map(|q| q.get(kind)).collect()
    }

    #[test]
    fn every_layout_is_drawn_with_valid_unique_ids() {
        let r = rendered(every_layout());
        assert_eq!(r.slides.len(), 10);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(r.notes, [("c1a2b3c4d_s03".to_string(), "Open with the revenue number.".to_string())]);
        let mut ids = HashSet::new();
        for q in all(&r) {
            let (kind, body) = q.as_object().unwrap().iter().next().unwrap();
            if kind.starts_with("create") && kind != "createParagraphBullets" {
                let id = body["objectId"].as_str().unwrap();
                assert!((5..=50).contains(&id.len()), "{id}");
                assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'), "{id}");
                assert!(ids.insert(id.to_string()), "{id} is used twice");
            }
        }
        // Each slide starts by creating itself, blank, and everything on it belongs to it.
        for (i, slide) in r.slides.iter().enumerate() {
            let created = &slide[0]["createSlide"];
            assert_eq!(created["objectId"], r.slide_ids[i].as_str());
            assert_eq!(created["slideLayoutReference"]["predefinedLayout"], "BLANK");
            assert!(created.get("insertionIndex").is_none());
            for q in &slide[1..] {
                let body = q.as_object().unwrap().values().next().unwrap();
                let page = &body["elementProperties"]["pageObjectId"];
                assert!(page.is_null() || page == r.slide_ids[i].as_str(), "{q}");
                assert!(body["objectId"].as_str().unwrap().starts_with(&r.slide_ids[i]), "{q}");
            }
        }
        assert_eq!(of(&r, "createTable").len(), 1);
        assert_eq!(of(&r, "createImage")[0]["url"], "https://example.com/shot.png");
        // Nothing is created empty, which the API refuses.
        assert!(of(&r, "insertText").iter().all(|t| !t["text"].as_str().unwrap().trim().is_empty()));
    }

    #[test]
    fn everything_stays_on_the_page() {
        for theme in super::super::theme::NAMES {
            let r = render(&deck(every_layout()), &Theme::named(theme).unwrap(), "c1a2b3c4d", None);
            for q in all(&r) {
                let body = q.as_object().unwrap().values().next().unwrap();
                let Some(e) = body.get("elementProperties") else { continue };
                let (x, y) = (e["transform"]["translateX"].as_f64().unwrap(), e["transform"]["translateY"].as_f64().unwrap());
                let (w, h) = (e["size"]["width"]["magnitude"].as_f64().unwrap(), e["size"]["height"]["magnitude"].as_f64().unwrap());
                assert!(w > 0.0 && h > 0.0, "{q}");
                assert!(x >= 0.0 && y >= 0.0 && x + w <= PAGE_W + 0.01 && y + h <= PAGE_H + 0.01, "{theme}: {q}");
            }
        }
    }

    #[test]
    fn slides_can_be_inserted_at_a_position() {
        let r = render(&deck(json!([{ "title": "A", "bullets": ["x"] }, { "title": "B", "bullets": ["y"] }])),
            &Theme::named("bold").unwrap(), "c9", Some(3));
        let at: Vec<&Value> = of(&r, "createSlide").iter().map(|s| &s["insertionIndex"]).collect();
        assert_eq!(at, [&json!(3), &json!(4)]);
    }

    #[test]
    fn bullets_become_a_list_last_with_styles_placed_before_the_tabs_go() {
        let r = rendered(json!([{ "title": "Cover" }, { "title": "T", "body": "Lead 🚀 in.", "bullets": [
            "One **bold**", { "text": "Two", "sub": ["Under **it**"] } ] }]));
        let slide = &r.slides[1];
        let inserted = slide.iter().filter_map(|q| q.get("insertText")).next_back().unwrap();
        let text = inserted["text"].as_str().unwrap();
        assert_eq!(text, "Lead 🚀 in.\nOne bold\nTwo\n\tUnder it");
        let id = inserted["objectId"].as_str().unwrap();
        let last = slide.last().unwrap();
        // The list starts after the lead paragraph and runs to the end of the text.
        assert_eq!(last["createParagraphBullets"], json!({
            "objectId": id, "bulletPreset": "BULLET_DISC_CIRCLE_SQUARE",
            "textRange": { "type": "FIXED_RANGE", "startIndex": 12, "endIndex": 34 },
        }));
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let ranged: Vec<(String, &str)> = slide
            .iter()
            .filter_map(|q| q.get("updateTextStyle"))
            .filter(|s| s["objectId"] == id && s["textRange"]["type"] == "FIXED_RANGE")
            .map(|s| {
                let (a, b) = (s["textRange"]["startIndex"].as_u64().unwrap() as usize, s["textRange"]["endIndex"].as_u64().unwrap() as usize);
                (String::from_utf16(&utf16[a..b]).unwrap(), s["fields"].as_str().unwrap())
            })
            .collect();
        assert_eq!(ranged, [
            ("\tUnder it".to_string(), "foregroundColor"),
            ("bold".to_string(), "bold"),
            ("it".to_string(), "bold"),
        ]);
    }

    #[test]
    fn text_is_sized_to_its_space_and_flagged_when_nothing_fits() {
        let size = |r: &Rendered, text: &str| -> f64 {
            let id = of(r, "insertText").into_iter().find(|t| t["text"].as_str().unwrap().starts_with(text)).unwrap()["objectId"].clone();
            of(r, "updateTextStyle").into_iter().find(|s| s["objectId"] == id).unwrap()["style"]["fontSize"]["magnitude"].as_f64().unwrap()
        };
        let short = rendered(json!([{ "title": "Cover" }, { "title": "Plan", "bullets": ["Alpha", "Beta", "Gamma"] }]));
        assert_eq!(size(&short, "Plan"), 28.0);
        assert_eq!(size(&short, "Alpha"), 20.0);
        let long_title = "A much longer title that cannot sit on a single line of the slide at full size";
        let point = "A point that goes on for quite a while, long enough to wrap onto a second line of the slide.";
        let dense = rendered(json!([{ "title": "Cover" }, { "title": long_title, "bullets": vec![point; 8] }]));
        assert_eq!(size(&dense, "A much"), 24.0);
        assert!(size(&dense, "A point") < 18.0);
        assert!(dense.warnings.is_empty(), "{:?}", dense.warnings);
        let essay = "word ".repeat(180);
        let crowded = rendered(json!([{ "title": "Cover" }, { "title": "T", "bullets": vec![essay.trim(); 6] }]));
        assert_eq!(crowded.warnings.len(), 1);
        assert!(crowded.warnings[0].starts_with("slide 2: the text may run past"), "{:?}", crowded.warnings);
    }

    #[test]
    fn sections_are_numbered_and_covers_use_the_cover_colours() {
        let r = rendered(json!([
            { "title": "Deck" }, { "layout": "section", "title": "One" },
            { "layout": "section", "title": "Two" }, { "layout": "section", "title": "Three", "kicker": "Appendix" },
        ]));
        let texts: Vec<&str> = of(&r, "insertText").iter().map(|t| t["text"].as_str().unwrap()).collect();
        assert_eq!(texts, ["Deck", "01", "One", "02", "Two", "APPENDIX", "Three"]);
        let theme = Theme::named("minimal").unwrap();
        for bg in of(&r, "updatePageProperties") {
            assert_eq!(bg["pageProperties"]["pageBackgroundFill"]["solidFill"]["color"]["rgbColor"], theme.cover_bg.rgb());
        }
    }

    #[test]
    fn tables_line_numbers_up_and_rule_only_between_rows() {
        let r = rendered(json!([{ "title": "Cover" }, { "title": "By region", "table": {
            "headers": ["Region", "Revenue"], "rows": [["EMEA", "$1,200"], ["APAC", "950"], ["Other", ""]] } }]));
        let table = &of(&r, "createTable")[0];
        assert_eq!((table["rows"].as_u64(), table["columns"].as_u64()), (Some(4), Some(2)));
        let widths: f64 = of(&r, "updateTableColumnProperties").iter()
            .map(|c| c["tableColumnProperties"]["columnWidth"]["magnitude"].as_f64().unwrap()).sum();
        assert!((widths - CONTENT_W).abs() < 0.1, "{widths}");
        let borders: Vec<&str> = of(&r, "updateTableBorderProperties").iter().map(|b| b["borderPosition"].as_str().unwrap()).collect();
        assert_eq!(borders, ["ALL", "INNER_HORIZONTAL", "BOTTOM", "BOTTOM"]);
        // The header is bold; the numbers, and the header over them, sit on the right.
        let cells = of(&r, "updateTextStyle");
        let bold: Vec<bool> = cells.iter().filter(|s| !s["cellLocation"].is_null()).map(|s| s["style"]["bold"].as_bool().unwrap()).collect();
        assert_eq!(bold, [true, true, false, false, false, false, false]);
        let right: Vec<u64> = of(&r, "updateParagraphStyle").iter().filter(|s| !s["cellLocation"].is_null())
            .map(|s| s["cellLocation"]["columnIndex"].as_u64().unwrap()).collect();
        assert_eq!(right, [1, 1, 1]);
        assert!(numeric("-4.5%") && numeric("$3.2M") && numeric("(1,200)") && !numeric("EMEA") && !numeric("Q3") && !numeric("-"));
    }

    #[test]
    fn wrapping_is_estimated_word_by_word() {
        assert_eq!(lines("short", 20.0, false, 600.0), 1);
        assert_eq!(lines("one\ntwo\n", 20.0, false, 600.0), 3);
        let sentence = "The quick brown fox jumps over the lazy dog and keeps on running.";
        let one_line = text_width(sentence, 20.0, false);
        assert_eq!(lines(sentence, 20.0, false, one_line + 1.0), 1);
        assert_eq!(lines(sentence, 20.0, false, one_line * 0.75), 2);
        assert!(lines(&"x".repeat(200), 20.0, false, 300.0) > 5);
        assert!(text_width("WWW", 20.0, false) > text_width("iii", 20.0, false) * 2.0);
        assert!(text_width("日本語", 20.0, false) >= 60.0);
    }
}
