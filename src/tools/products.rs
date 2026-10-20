//! `search_products`: find products to buy in any category, from any shop, and recommend the
//! best ones.
//!
//! 1. **Search.** Two Brave web searches, one for reviews and roundups ("best trail running shoes
//!    review") and one for shops ("trail running shoes buy online"), and, with a Best Buy key,
//!    Best Buy's catalog.
//! 2. **Read.** The most promising pages are fetched (through the same public-addresses-only
//!    client as `fetch_url`) and read for the schema.org product data shops embed: name, photos,
//!    price, stock, and the customer rating with its review count (see [`super::product_page`]).
//!    Review articles are kept as text.
//! 3. **Reason.** The run's own model gets that evidence in one tool-free request and picks the
//!    best options for the request, with why, pros, cons, and who each suits.
//! 4. **Check.** The reply is held to the evidence: links and photos only come from the pages
//!    and search results, a price is shown as checked only when a store page states it, and a
//!    rating only when it comes with its review count. If the model can't be asked, or its reply
//!    can't be used, the products found are ranked by their ratings instead.
//!
//! The chat front ends draw the result as a card (chat/static/product-view.js, and the iOS
//! app's ProductsCardView).

use super::bestbuy::{self, BestBuy};
use super::fetch::WebClient;
use super::product_page::{self, PageFacts};
use super::web_search::{clean, BraveSearch};
use super::{Tool, ToolContext};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use futures_util::future::join_all;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_COUNT: usize = 4;
const MAX_COUNT: usize = 6;
const MAX_QUERY_CHARS: usize = 160;
const MAX_NEEDS_CHARS: usize = 600;
/// Results asked for per search.
const REVIEW_RESULTS: u64 = 8;
const SHOP_RESULTS: u64 = 10;
/// Pages read per call, at most this many shop pages and this many reviews among them.
const MAX_PAGES: usize = 8;
const MAX_SHOP_PAGES: usize = 5;
const MAX_REVIEW_PAGES: usize = 3;
const PAGE_BYTES: usize = 1_500_000;
const PAGE_TIMEOUT: Duration = Duration::from_secs(12);
/// Products kept from all the pages, for the model to choose from.
const MAX_CANDIDATES: usize = 30;
/// A page's text, for the model to read reviews in.
const EXCERPT_CHARS: usize = 1800;
/// The evidence sent to the model, in characters, before the instructions.
const EVIDENCE_CHARS: usize = 32_000;
const MAX_TEXT_CHARS: usize = 200;
const MAX_REASON_CHARS: usize = 500;
const MAX_PROS: usize = 4;
const MAX_COMPARABLE: usize = 3;
const MAX_CITED: usize = 4;
/// Fewer reviews than this and an average says little.
const FEW_REVIEWS: u64 = 5;
/// The adjusted rating shrinks the average toward `PRIOR_MEAN` as if `PRIOR_REVIEWS` typical
/// reviews were added, so a 5.0 from three reviews ranks below a 4.6 from a thousand.
const PRIOR_MEAN: f64 = 4.0;
const PRIOR_REVIEWS: f64 = 15.0;
/// Hosts whose pages hold nothing to read without a browser.
const SKIP_HOSTS: &[&str] = &["youtube.com", "youtu.be", "tiktok.com", "instagram.com"];
/// The labels the model may give an option, and the badge each gets on the card.
const LABELS: &[(&str, &str)] = &[
    ("top_pick", "Top pick"),
    ("best_value", "Best value"),
    ("premium", "Premium pick"),
    ("budget", "Budget pick"),
    ("alternative", ""),
];
const DISCLAIMER: &str = "Found with a web search and read from the stores' pages at the time of \
                          the search; prices, stock and ratings change. The picks and their \
                          reasons are the model's reading of those pages and reviews, so check \
                          the store before you buy.";

const SYSTEM: &str = "You are a careful, independent shopping advisor. You recommend products \
    only from the evidence you are given: product data read from store pages, search results, \
    and the text of review articles. Never invent a product, price, rating, link or fact the \
    evidence does not support. Prefer products that fit the request and the stated needs, that \
    reviewers recommend, and whose customer ratings rest on many reviews; weigh the price \
    against what it buys. Say plainly when the evidence is thin. Reply with JSON only.";

const INSTRUCTION: &str = "Pick the {count} best options for this request (fewer if the \
    evidence supports fewer), best first. Reply with one JSON object and nothing else:
{\"summary\": \"2-3 sentences: which one to buy and why, and what to watch out for\",
 \"options\": [{\"name\": \"the product as it is sold\",
   \"candidate\": \"C3, or null when the product only appears in an article or search result\",
   \"sources\": [\"S1\", \"S4\"],
   \"price\": 129.99, \"currency\": \"USD\",
   \"label\": \"top_pick | best_value | premium | budget | alternative\",
   \"why\": \"1-2 sentences on why it fits this request\",
   \"pros\": [\"...\"], \"cons\": [\"...\"],
   \"best_for\": \"who it suits, in a few words\",
   \"compare_with\": [2, 3]}]}
Rules: the first option is your top pick. Use a product's C id whenever it has one. Cite the S \
ids of the sources behind what you say about it. Give a price only if a cited source or the \
product data states it, else null. compare_with holds the numbers (1-based) of the other \
options that are real alternatives to it. At most 4 pros and 4 cons, each a few words.";

/// A product as a page or a catalog describes it. Every field but the name and the link may be
/// missing.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Candidate {
    pub name: String,
    pub url: String,
    /// Who sells it: the shop's host name, or the catalog's name.
    pub store: String,
    pub brand: Option<String>,
    pub model: Option<String>,
    pub description: Option<String>,
    pub images: Vec<String>,
    pub price: Option<f64>,
    /// Only when it is above `price`.
    pub regular_price: Option<f64>,
    /// A three-letter code.
    pub currency: Option<String>,
    pub in_stock: Option<bool>,
    pub in_store: Option<bool>,
    pub free_shipping: Option<bool>,
    /// Out of 5, and only with the number of reviews behind it.
    pub rating: Option<f64>,
    pub review_count: Option<u64>,
    pub review_snippets: Vec<String>,
    pub features: Vec<String>,
    pub specs: Vec<(String, String)>,
    pub category: Option<String>,
}

/// Finds products with a web search (and Best Buy's catalog, given a key), reads the pages, and
/// asks the run's model to pick the best for the request. See the module docs.
pub struct SearchProducts {
    brave: Option<BraveSearch>,
    bestbuy: Option<BestBuy>,
    web: Arc<WebClient>,
}

impl SearchProducts {
    /// At least one of `brave` and `bestbuy` is needed for there to be anything to search.
    pub fn new(brave: Option<BraveSearch>, bestbuy: Option<BestBuy>, web: Arc<WebClient>) -> Self {
        Self {
            brave,
            bestbuy,
            web,
        }
    }

    async fn read_page(&self, url: &str, timeout: Duration) -> Result<PageFacts> {
        let page = self
            .web
            .get(url, PAGE_BYTES, true, timeout.min(PAGE_TIMEOUT))
            .await?;
        let html = page.content_type.contains("html")
            || (page.content_type.is_empty() && page.body.trim_ascii_start().starts_with(b"<"));
        if !html {
            bail!("{url} is not a web page");
        }
        let text = String::from_utf8_lossy(&page.body);
        Ok(product_page::extract(&page.final_url, &text, EXCERPT_CHARS))
    }

    /// Search, read the pages, and collect what they say.
    async fn gather(&self, req: &Request, timeout: Duration) -> Evidence {
        let mut ev = Evidence::default();
        if let Some(brave) = &self.brave {
            let (review_q, shop_q) = queries(req);
            // One search after the other: Brave's free plan allows one a second.
            for (q, kind, count) in [
                (review_q, Kind::Review, REVIEW_RESULTS),
                (shop_q, Kind::Shop, SHOP_RESULTS),
            ] {
                match brave.results(&q, count, timeout).await {
                    Ok(results) => {
                        for r in &results {
                            if let Some(src) = source_from_hit(r, kind) {
                                if !ev.sources.iter().any(|s| s.url == src.url) {
                                    ev.sources.push(src);
                                }
                            }
                        }
                    }
                    Err(e) => ev.notes.push(format!("the search {q:?} failed: {e:#}")),
                }
                ev.queries.push(q);
            }
            let order = reading_order(&ev.sources);
            let pages = join_all(
                order
                    .iter()
                    .map(|&i| self.read_page(&ev.sources[i].url, timeout)),
            )
            .await;
            for (i, page) in order.into_iter().zip(pages) {
                match page {
                    Ok(facts) => {
                        let src = &mut ev.sources[i];
                        src.read = true;
                        src.image = facts.image;
                        src.excerpt = facts.excerpt;
                        if src.title.is_empty() {
                            src.title = facts.title.unwrap_or_default();
                        }
                        if src.snippet.is_empty() {
                            src.snippet = facts.description.unwrap_or_default();
                        }
                        for c in facts.products {
                            add_candidate(&mut ev.candidates, c, i);
                        }
                    }
                    Err(e) => {
                        tracing::debug!(url = %ev.sources[i].url, error = %format!("{e:#}"), "product page not read")
                    }
                }
            }
        }
        if let Some(bb) = &self.bestbuy {
            match bb
                .candidates(&req.query, req.min_price, req.max_price, timeout)
                .await
            {
                Ok(found) => {
                    for c in found {
                        ev.sources.push(Source {
                            url: c.url.clone(),
                            title: format!("{} at {}", c.name, bestbuy::STORE),
                            host: "bestbuy.com".into(),
                            kind: Kind::Catalog,
                            read: true,
                            ..Source::default()
                        });
                        let at = ev.sources.len() - 1;
                        add_candidate(&mut ev.candidates, c, at);
                    }
                }
                Err(e) => ev.notes.push(format!("{}: {e:#}", bestbuy::STORE)),
            }
        }
        ev
    }
}

#[async_trait]
impl Tool for SearchProducts {
    fn name(&self) -> &str {
        "search_products"
    }

    fn description(&self) -> &str {
        "Find and compare products to buy, in any category and from any shop: shoes and \
         clothing, electronics, home and kitchen, outdoor and sports gear, beauty, toys, and \
         more. It searches the web for reviews and store pages, reads the product data on those \
         pages (photos, price, stock, and customer ratings with how many reviews they rest on), \
         weighs that evidence against the request to pick the best options, and shows them to \
         the user as a card: photos, prices, a top pick with its reasons, pros and cons, who \
         each suits, and comparable alternatives. Give query as what the user wants, e.g. \
         \"trail running shoes for wide feet\" or \"carry-on suitcase\", needs for what matters \
         to them (use, size, style, must-haves, brands to avoid), and the budget. The card shows \
         the details, so afterwards answer in a sentence or two: your recommendation and \
         anything to check before buying."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "query": { "type": "string",
                               "description": "What to look for, e.g. \"waterproof hiking boots for women\"." },
                    "needs": { "type": "string",
                               "description": "What matters to the user: how they'll use it, size or fit, style, must-haves, things to avoid." },
                    "min_price": { "type": "number", "minimum": 0, "description": "The least it should cost." },
                    "max_price": { "type": "number", "minimum": 0, "description": "The most it should cost." },
                    "count": { "type": "integer", "minimum": 1, "maximum": MAX_COUNT,
                               "description": "How many options to recommend, default 4." }
                },
                "required": ["query"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let req = parse_input(input)?;
        let ev = self.gather(&req, ctx.timeout).await;
        if ev.sources.is_empty() {
            let why = if ev.notes.is_empty() {
                "nothing was found".to_string()
            } else {
                ev.notes.join("; ")
            };
            bail!(
                "no products found for {:?} ({why}); try other words",
                req.query
            );
        }

        // Ask the model to weigh the evidence; fall back on the ratings if that doesn't work.
        let asked = match &ctx.model {
            None => Err(anyhow!("no model is available to weigh the options")),
            Some(model) => {
                let prompt = prompt(&req, &ev);
                model
                    .complete(SYSTEM, &prompt)
                    .await
                    .and_then(|reply| parse_reply(&reply))
            }
        };
        let (method, summary, mut picks, note) = match asked {
            Ok(reply) => {
                let picks = assemble(&req, &ev, &reply.options);
                if picks.is_empty() {
                    let picks = by_ratings(&req, &ev);
                    (
                        "ratings",
                        None,
                        picks,
                        Some("the model named no product found in the evidence".to_string()),
                    )
                } else {
                    ("model", reply.summary, picks, None)
                }
            }
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "search_products: falling back on ratings");
                (
                    "ratings",
                    None,
                    by_ratings(&req, &ev),
                    Some(format!("{e:#}")),
                )
            }
        };
        if picks.is_empty() {
            bail!(
                "no products with a store page were found for {:?}; try other or fewer words",
                req.query
            );
        }
        link_comparable(&mut picks);
        Ok(result(&req, &ev, method, summary, note, &picks).to_string())
    }
}

// -- input ------------------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
struct Request {
    query: String,
    needs: Option<String>,
    min_price: Option<f64>,
    max_price: Option<f64>,
    count: usize,
}

fn parse_input(input: &Value) -> Result<Request> {
    let query: String = input["query"]
        .as_str()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .ok_or_else(|| {
            anyhow!("give query: what to look for, e.g. \"trail running shoes for wide feet\"")
        })?
        .chars()
        .take(MAX_QUERY_CHARS)
        .collect();
    let needs = text(&input["needs"], MAX_NEEDS_CHARS);
    let money = |key: &str| -> Result<Option<f64>> {
        match &input[key] {
            Value::Null => Ok(None),
            v => {
                let n = v.as_f64().ok_or_else(|| anyhow!("{key} is a number"))?;
                if !n.is_finite() || n < 0.0 {
                    bail!("{key} can't be negative");
                }
                Ok(Some((n * 100.0).round() / 100.0))
            }
        }
    };
    let min_price = money("min_price")?;
    let max_price = money("max_price")?;
    if let (Some(lo), Some(hi)) = (min_price, max_price) {
        if lo > hi {
            bail!("min_price ({lo}) is above max_price ({hi})");
        }
    }
    let count = match &input["count"] {
        Value::Null => DEFAULT_COUNT,
        v => v
            .as_u64()
            .filter(|n| *n >= 1)
            .ok_or_else(|| anyhow!("count is a whole number from 1 to {MAX_COUNT}"))?
            .min(MAX_COUNT as u64) as usize,
    };
    Ok(Request {
        query,
        needs,
        min_price,
        max_price,
        count,
    })
}

/// The two searches: one for reviews and roundups, one for shops.
fn queries(req: &Request) -> (String, String) {
    let budget = match (req.min_price, req.max_price) {
        (_, Some(hi)) => format!(" under {}", plain_number(hi)),
        (Some(lo), None) => format!(" over {}", plain_number(lo)),
        (None, None) => String::new(),
    };
    let q = req.query.trim();
    let review = if q.to_lowercase().starts_with("best ") {
        format!("{q}{budget} review")
    } else {
        format!("best {q}{budget} review")
    };
    (review, format!("{q}{budget} buy online"))
}

fn plain_number(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n:.2}")
    }
}

// -- evidence ---------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum Kind {
    /// From the search for reviews and roundups.
    #[default]
    Review,
    /// From the search for shops.
    Shop,
    /// A catalog's product page.
    Catalog,
}

#[derive(Debug, Clone, Default)]
struct Source {
    url: String,
    title: String,
    host: String,
    snippet: String,
    /// Brave's thumbnail for the result (served by Brave).
    thumbnail: Option<String>,
    kind: Kind,
    /// The page's Open Graph picture, once read.
    image: Option<String>,
    excerpt: String,
    read: bool,
}

#[derive(Debug, Default)]
struct Evidence {
    sources: Vec<Source>,
    /// Each product with the index of the source it came from.
    candidates: Vec<(Candidate, usize)>,
    queries: Vec<String>,
    notes: Vec<String>,
}

/// `value` as trimmed text cut to `max` characters, or `None` if it is missing or blank.
fn text(value: &Value, max: usize) -> Option<String> {
    value
        .as_str()
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(max).collect())
}

fn http_url(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|u| {
            (u.starts_with("https://") || u.starts_with("http://"))
                && !u.contains(char::is_whitespace)
        })
        .map(String::from)
}

/// A Brave web result as a source.
fn source_from_hit(r: &Value, kind: Kind) -> Option<Source> {
    let url = http_url(&r["url"])?;
    let host = product_page::host(&url);
    if SKIP_HOSTS
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")))
    {
        return None;
    }
    let mut snippet = clean(r["description"].as_str().unwrap_or_default());
    for extra in r["extra_snippets"].as_array().into_iter().flatten().take(3) {
        if let Some(e) = extra.as_str() {
            snippet.push_str(" … ");
            snippet.push_str(&clean(e));
        }
    }
    Some(Source {
        title: clean(r["title"].as_str().unwrap_or_default())
            .chars()
            .take(MAX_TEXT_CHARS)
            .collect(),
        snippet: snippet.chars().take(600).collect(),
        thumbnail: http_url(&r["thumbnail"]["src"]).filter(|u| u.starts_with("https://")),
        url,
        host,
        kind,
        ..Source::default()
    })
}

/// Which sources to read, by index: shops first, then a few reviews, then the rest.
fn reading_order(sources: &[Source]) -> Vec<usize> {
    let shops = sources
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == Kind::Shop)
        .map(|(i, _)| i)
        .take(MAX_SHOP_PAGES);
    let reviews = sources
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == Kind::Review)
        .map(|(i, _)| i)
        .take(MAX_REVIEW_PAGES);
    let mut order: Vec<usize> = shops.chain(reviews).collect();
    for (i, s) in sources.iter().enumerate() {
        if order.len() >= MAX_PAGES {
            break;
        }
        if !order.contains(&i) && s.kind != Kind::Catalog {
            order.push(i);
        }
    }
    order.truncate(MAX_PAGES);
    order
}

/// Add `c` unless the same product from the same store is there already, keeping whichever
/// says more.
fn add_candidate(list: &mut Vec<(Candidate, usize)>, c: Candidate, source: usize) {
    let detail = |c: &Candidate| {
        c.price.is_some() as u8 * 2 + c.rating.is_some() as u8 * 2 + !c.images.is_empty() as u8
    };
    let key = |c: &Candidate| (c.name.to_lowercase(), c.store.clone());
    if let Some(at) = list.iter().position(|(o, _)| key(o) == key(&c)) {
        if detail(&c) > detail(&list[at].0) {
            list[at] = (c, source);
        }
        return;
    }
    if list.len() < MAX_CANDIDATES {
        list.push((c, source));
    }
}

fn price_text(price: f64, currency: Option<&str>) -> String {
    match currency {
        Some(c) => format!("{price:.2} {c}"),
        None => format!("{price:.2}"),
    }
}

fn within_budget(req: &Request, price: Option<f64>) -> bool {
    match price {
        None => true,
        Some(p) => req.min_price.is_none_or(|lo| p >= lo) && req.max_price.is_none_or(|hi| p <= hi),
    }
}

/// The request and the evidence, as the model reads them.
fn prompt(req: &Request, ev: &Evidence) -> String {
    let mut out = format!("Shopping request: {}\n", req.query);
    out.push_str(&format!(
        "What matters to them: {}\n",
        req.needs.as_deref().unwrap_or("(not stated)")
    ));
    let budget = match (req.min_price, req.max_price) {
        (Some(lo), Some(hi)) => format!("{} to {}", plain_number(lo), plain_number(hi)),
        (None, Some(hi)) => format!("at most {}", plain_number(hi)),
        (Some(lo), None) => format!("at least {}", plain_number(lo)),
        (None, None) => "(not stated)".into(),
    };
    out.push_str(&format!("Budget: {budget}\n"));
    out.push_str(&format!("Today: {}\n\n", Utc::now().format("%Y-%m-%d")));

    out.push_str("PRODUCTS FOUND ON STORE PAGES AND IN CATALOGS\n");
    if ev.candidates.is_empty() {
        out.push_str("(none: no page that was read had product data)\n");
    }
    for (i, (c, src)) in ev.candidates.iter().enumerate() {
        let mut line = format!("[C{}] {}", i + 1, c.name);
        if let Some(b) = &c.brand {
            line.push_str(&format!(" | brand: {b}"));
        }
        if let Some(p) = c.price {
            line.push_str(&format!(
                " | price: {}",
                price_text(p, c.currency.as_deref())
            ));
            if let Some(r) = c.regular_price {
                line.push_str(&format!(" (was {r:.2})"));
            }
            if !within_budget(req, Some(p)) {
                line.push_str(" (outside the budget)");
            }
        }
        if let (Some(r), Some(n)) = (c.rating, c.review_count) {
            line.push_str(&format!(
                " | rating: {r:.1}/5 from {} reviews",
                thousands(n)
            ));
        }
        match c.in_stock {
            Some(true) => line.push_str(" | in stock"),
            Some(false) => line.push_str(" | out of stock"),
            None => {}
        }
        line.push_str(&format!(" | store: {} (S{})\n", c.store, src + 1));
        if !c.review_snippets.is_empty() {
            line.push_str(&format!(
                "    reviews: {}\n",
                c.review_snippets
                    .iter()
                    .map(|r| format!("\"{r}\""))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
        if !c.features.is_empty() {
            line.push_str(&format!("    features: {}\n", c.features.join("; ")));
        }
        if let Some(d) = &c.description {
            line.push_str(&format!(
                "    about: {}\n",
                d.chars().take(240).collect::<String>()
            ));
        }
        out.push_str(&line);
    }

    out.push_str("\nSEARCH RESULTS AND PAGES\n");
    for (i, s) in ev.sources.iter().enumerate() {
        let kind = match s.kind {
            Kind::Review => "review search",
            Kind::Shop => "shop search",
            Kind::Catalog => "catalog",
        };
        let mut entry = format!("[S{}] {} ({}, {kind})\n", i + 1, s.title, s.host);
        if !s.snippet.is_empty() {
            entry.push_str(&format!("    snippet: {}\n", s.snippet));
        }
        if !s.excerpt.is_empty() && out.len() + entry.len() + s.excerpt.len() < EVIDENCE_CHARS {
            entry.push_str(&format!(
                "    page text: {}\n",
                s.excerpt.replace('\n', " / ")
            ));
        }
        if out.len() + entry.len() > EVIDENCE_CHARS {
            break;
        }
        out.push_str(&entry);
    }
    out.push_str("\n---\n");
    out.push_str(&INSTRUCTION.replace("{count}", &req.count.to_string()));
    out
}

// -- the model's reply --------------------------------------------------------------------------

#[derive(Debug)]
struct Reply {
    summary: Option<String>,
    options: Vec<Value>,
}

/// The JSON object in the model's reply, which may sit in a code fence or among other text.
fn parse_reply(reply: &str) -> Result<Reply> {
    let start = reply.find('{');
    let end = reply.rfind('}');
    let value: Value = match (start, end) {
        (Some(s), Some(e)) if e > s => serde_json::from_str(&reply[s..=e])
            .map_err(|e| anyhow!("the model's reply is not valid JSON: {e}"))?,
        _ => bail!("the model's reply holds no JSON"),
    };
    let options = value["options"]
        .as_array()
        .cloned()
        .ok_or_else(|| anyhow!("the model's reply has no options"))?;
    Ok(Reply {
        summary: text(&value["summary"], 800),
        options,
    })
}

/// A recommended product, as the card shows it.
#[derive(Debug, Clone, Default, PartialEq)]
struct Pick {
    name: String,
    /// Its index in the evidence's candidates, when it has product data.
    candidate: Option<usize>,
    url: String,
    store: String,
    images: Vec<String>,
    price: Option<f64>,
    currency: Option<String>,
    /// The price comes from a store page or catalog, not from text.
    price_checked: bool,
    label: Option<&'static str>,
    why: Option<String>,
    pros: Vec<String>,
    cons: Vec<String>,
    best_for: Option<String>,
    /// Indexes of the sources behind it.
    sources: Vec<usize>,
    /// The model's 1-based option numbers it is compared with, then (see [`link_comparable`])
    /// the indexes of those picks.
    comparable: Vec<usize>,
    /// Its 1-based number in the model's list.
    rank: usize,
}

/// An id like `C3` (or `S3`, or a plain 3), as a 0-based index below `len`.
fn index(v: &Value, prefix: char, len: usize) -> Option<usize> {
    let n = match v {
        Value::Number(n) => n.as_u64()?,
        Value::String(s) => {
            let s = s.trim();
            let s = s
                .strip_prefix(prefix)
                .or_else(|| s.strip_prefix(prefix.to_ascii_lowercase()))
                .unwrap_or(s);
            s.parse().ok()?
        }
        _ => return None,
    };
    let n = usize::try_from(n).ok()?;
    (1..=len).contains(&n).then(|| n - 1)
}

fn strings(v: &Value, max_items: usize, max_chars: usize) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| text(s, max_chars))
        .take(max_items)
        .collect()
}

/// Whether `text` mentions `price` as a number of its own ("129", "129.95", "1,299.00").
fn mentions_price(text: &str, price: f64) -> bool {
    let plain = text.replace(',', "");
    let forms = [format!("{price:.2}"), plain_number(price)];
    forms.iter().any(|f| {
        plain.match_indices(f.as_str()).any(|(i, _)| {
            let before = plain[..i].chars().next_back();
            let after = plain[i + f.len()..].chars().next();
            !before.is_some_and(|c| c.is_ascii_digit() || c == '.')
                && !after.is_some_and(|c| c.is_ascii_digit())
        })
    })
}

/// The model's options, held to the evidence: each needs a link from a source or its product
/// data, photos only come from those, and a price only from product data (checked) or the text
/// of a source it cites. Options that can't be placed are left out.
fn assemble(req: &Request, ev: &Evidence, options: &[Value]) -> Vec<Pick> {
    let mut picks: Vec<Pick> = Vec::new();
    for (i, o) in options.iter().enumerate().take(MAX_COUNT * 2) {
        if picks.len() >= req.count {
            break;
        }
        let Some(name) = text(&o["name"], MAX_TEXT_CHARS) else {
            continue;
        };
        let candidate = index(&o["candidate"], 'C', ev.candidates.len());
        let mut sources: Vec<usize> = Vec::new();
        if let Some((_, src)) = candidate.map(|c| &ev.candidates[c]) {
            sources.push(*src);
        }
        for s in o["sources"].as_array().into_iter().flatten() {
            if let Some(s) = index(s, 'S', ev.sources.len()) {
                if !sources.contains(&s) && sources.len() < MAX_CITED {
                    sources.push(s);
                }
            }
        }
        let cand = candidate.map(|c| &ev.candidates[c].0);
        let shop = sources
            .iter()
            .find(|&&s| ev.sources[s].kind != Kind::Review)
            .or(sources.first())
            .copied();
        let url = match (cand, shop) {
            (Some(c), _) => c.url.clone(),
            (None, Some(s)) => ev.sources[s].url.clone(),
            (None, None) => continue,
        };
        if picks.iter().any(|p| {
            (candidate.is_some() && p.candidate == candidate)
                || (p.url == url && p.name.eq_ignore_ascii_case(&name))
        }) {
            continue;
        }
        let mut images: Vec<String> = cand.map(|c| c.images.clone()).unwrap_or_default();
        if images.is_empty() {
            // Only a shop page's picture is a picture of the product; an article's is its own.
            for &s in &sources {
                let src = &ev.sources[s];
                if src.kind != Kind::Review {
                    images.extend(src.image.clone());
                    images.extend(src.thumbnail.clone());
                }
            }
            images.dedup();
        }
        let (price, currency, price_checked) = match cand.and_then(|c| c.price) {
            Some(p) => (Some(p), cand.and_then(|c| c.currency.clone()), true),
            None => {
                let stated = o["price"].as_f64().filter(|p| p.is_finite() && *p > 0.0);
                let seen = stated.filter(|&p| {
                    sources.iter().any(|&s| {
                        let src = &ev.sources[s];
                        mentions_price(&src.snippet, p) || mentions_price(&src.excerpt, p)
                    })
                });
                let cur = o["currency"]
                    .as_str()
                    .map(|c| c.trim().to_ascii_uppercase())
                    .filter(|c| c.len() == 3 && c.chars().all(|c| c.is_ascii_alphabetic()));
                (seen, seen.and(cur), false)
            }
        };
        let label = o["label"].as_str().and_then(|l| {
            let l = l.trim().to_ascii_lowercase();
            LABELS.iter().map(|(k, _)| *k).find(|k| *k == l)
        });
        picks.push(Pick {
            store: cand
                .map(|c| c.store.clone())
                .unwrap_or_else(|| product_page::host(&url)),
            name,
            candidate,
            url,
            images,
            price,
            currency,
            price_checked,
            label,
            why: text(&o["why"], MAX_REASON_CHARS),
            pros: strings(&o["pros"], MAX_PROS, 120),
            cons: strings(&o["cons"], MAX_PROS, 120),
            best_for: text(&o["best_for"], 160),
            sources,
            comparable: o["compare_with"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64)
                .map(|n| n as usize)
                .collect(),
            rank: i + 1,
        });
    }
    picks
}

/// The products found, ranked by their adjusted rating, for when the model can't weigh them.
fn by_ratings(req: &Request, ev: &Evidence) -> Vec<Pick> {
    let mut ranked: Vec<(usize, &Candidate, Reviews)> = ev
        .candidates
        .iter()
        .enumerate()
        .filter(|(_, (c, _))| {
            (c.price.is_some() || c.rating.is_some()) && within_budget(req, c.price)
        })
        .map(|(i, (c, _))| (i, c, review_digest(c.rating, c.review_count.unwrap_or(0))))
        .collect();
    ranked.sort_by(|a, b| {
        let score = |r: &Reviews| r.adjusted.unwrap_or(0.0);
        score(&b.2)
            .total_cmp(&score(&a.2))
            .then(b.2.count.cmp(&a.2.count))
            .then(b.1.images.is_empty().cmp(&a.1.images.is_empty()).reverse())
    });
    ranked
        .into_iter()
        .take(req.count)
        .enumerate()
        .map(|(n, (i, c, _))| Pick {
            name: c.name.clone(),
            candidate: Some(i),
            url: c.url.clone(),
            store: c.store.clone(),
            images: c.images.clone(),
            price: c.price,
            currency: c.currency.clone(),
            price_checked: c.price.is_some(),
            sources: vec![ev.candidates[i].1],
            rank: n + 1,
            ..Pick::default()
        })
        .collect()
}

/// Turn each pick's `comparable` from the model's option numbers into indexes of the picks;
/// where the model named none, use the other picks closest in price.
fn link_comparable(picks: &mut [Pick]) {
    let ranks: Vec<usize> = picks.iter().map(|p| p.rank).collect();
    let prices: Vec<Option<f64>> = picks.iter().map(|p| p.price).collect();
    for (i, p) in picks.iter_mut().enumerate() {
        let mut linked: Vec<usize> = Vec::new();
        for n in &p.comparable {
            if let Some(j) = ranks.iter().position(|r| r == n) {
                if j != i && !linked.contains(&j) {
                    linked.push(j);
                }
            }
        }
        if linked.is_empty() {
            let mut others: Vec<usize> = (0..ranks.len()).filter(|&j| j != i).collect();
            if let Some(own) = prices[i] {
                others.sort_by(|&a, &b| {
                    let gap = |j: usize| prices[j].map_or(f64::MAX, |q| (q - own).abs());
                    gap(a).total_cmp(&gap(b))
                });
            }
            linked = others;
        }
        linked.truncate(MAX_COMPARABLE);
        p.comparable = linked;
    }
}

// -- reviews ----------------------------------------------------------------------------------

/// What a product's reviews amount to.
#[derive(Debug, Clone, PartialEq)]
struct Reviews {
    average: Option<f64>,
    count: u64,
    /// The average shrunk toward a typical rating by how few reviews there are, 0–5.
    adjusted: Option<f64>,
    verdict: &'static str,
    confidence: &'static str,
    summary: String,
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Reads an average rating and its review count: the adjusted rating, a verdict, how much to
/// trust it, and a sentence saying so.
fn review_digest(average: Option<f64>, count: u64) -> Reviews {
    let Some(avg) = average.filter(|_| count > 0) else {
        return Reviews {
            average: None,
            count: 0,
            adjusted: None,
            verdict: "none",
            confidence: "low",
            summary: "No customer rating found.".into(),
        };
    };
    let n = count as f64;
    let adjusted =
        ((n * avg + PRIOR_REVIEWS * PRIOR_MEAN) / (n + PRIOR_REVIEWS) * 10.0).round() / 10.0;
    let confidence = if count >= 100 {
        "high"
    } else if count >= 20 {
        "medium"
    } else {
        "low"
    };
    let reviews = if count == 1 {
        "1 review".to_string()
    } else {
        format!("{} reviews", thousands(count))
    };
    if count < FEW_REVIEWS {
        return Reviews {
            average: Some(avg),
            count,
            adjusted: Some(adjusted),
            verdict: "few",
            confidence,
            summary: format!("{avg:.1} stars, but from only {reviews}: too few to judge."),
        };
    }
    let (verdict, reading) = if adjusted >= 4.5 {
        ("excellent", "consistently well reviewed")
    } else if adjusted >= 4.0 {
        ("good", "well reviewed")
    } else if adjusted >= 3.4 {
        ("mixed", "mixed reviews")
    } else {
        ("poor", "poorly reviewed")
    };
    Reviews {
        average: Some(avg),
        count,
        adjusted: Some(adjusted),
        verdict,
        confidence,
        summary: format!(
            "{avg:.1} stars from {reviews} ({adjusted:.1} adjusted for how many there are): {reading}."
        ),
    }
}

// -- the result -------------------------------------------------------------------------------

fn pick_id(i: usize) -> String {
    format!("P{}", i + 1)
}

fn option_json(i: usize, p: &Pick, ev: &Evidence, badges: Vec<&str>) -> Value {
    let cand = p.candidate.map(|c| &ev.candidates[c].0);
    let reviews = review_digest(
        cand.and_then(|c| c.rating),
        cand.and_then(|c| c.review_count).unwrap_or(0),
    );
    let mut v = Map::new();
    v.insert("sku".into(), json!(pick_id(i)));
    v.insert("name".into(), json!(p.name));
    v.insert("price".into(), json!(p.price));
    v.insert("currency".into(), json!(p.currency));
    v.insert("price_checked".into(), json!(p.price_checked));
    v.insert("url".into(), json!(p.url));
    v.insert("store".into(), json!(p.store));
    v.insert("images".into(), json!(p.images));
    if let Some(c) = cand {
        if let Some(b) = &c.brand {
            v.insert("brand".into(), json!(b));
        }
        if let Some(m) = &c.model {
            v.insert("model".into(), json!(m));
        }
        if let (Some(price), Some(regular)) = (c.price, c.regular_price) {
            if regular > price && p.price == Some(price) {
                v.insert("regular_price".into(), json!(regular));
                v.insert(
                    "savings_pct".into(),
                    json!(((1.0 - price / regular) * 1000.0).round() / 10.0),
                );
            }
        }
        if let Some(d) = &c.description {
            v.insert("description".into(), json!(d));
        }
        v.insert("features".into(), json!(c.features));
        v.insert(
            "specs".into(),
            Value::Array(
                c.specs
                    .iter()
                    .map(|(n, val)| json!({ "name": n, "value": val }))
                    .collect(),
            ),
        );
        if let Some(cat) = &c.category {
            v.insert("category".into(), json!(cat));
        }
        let mut avail = Map::new();
        for (key, known) in [
            ("online", c.in_stock),
            ("in_store", c.in_store),
            ("free_shipping", c.free_shipping),
        ] {
            if let Some(k) = known {
                avail.insert(key.into(), json!(k));
            }
        }
        if !avail.is_empty() {
            v.insert("availability".into(), Value::Object(avail));
        }
    }
    v.insert(
        "reviews".into(),
        json!({
            "average": reviews.average, "count": reviews.count, "adjusted": reviews.adjusted,
            "verdict": reviews.verdict, "confidence": reviews.confidence, "summary": reviews.summary,
        }),
    );
    v.insert("badges".into(), json!(badges));
    if let Some(l) = p.label {
        v.insert("label".into(), json!(l));
    }
    if let Some(w) = &p.why {
        v.insert("why".into(), json!(w));
    }
    v.insert("pros".into(), json!(p.pros));
    v.insert("cons".into(), json!(p.cons));
    if let Some(b) = &p.best_for {
        v.insert("best_for".into(), json!(b));
    }
    v.insert(
        "sources".into(),
        Value::Array(p.sources.iter().map(|&s| json!(s + 1)).collect()),
    );
    v.insert(
        "comparable".into(),
        Value::Array(p.comparable.iter().map(|&j| json!(pick_id(j))).collect()),
    );
    Value::Object(v)
}

fn result(
    req: &Request,
    ev: &Evidence,
    method: &str,
    summary: Option<String>,
    note: Option<String>,
    picks: &[Pick],
) -> Value {
    let reviews: Vec<Reviews> = picks
        .iter()
        .map(|p| {
            let c = p.candidate.map(|c| &ev.candidates[c].0);
            review_digest(
                c.and_then(|c| c.rating),
                c.and_then(|c| c.review_count).unwrap_or(0),
            )
        })
        .collect();
    let best_rated = reviews
        .iter()
        .enumerate()
        .filter(|(_, r)| r.count >= FEW_REVIEWS && r.adjusted.is_some())
        .max_by(|(_, a), (_, b)| {
            a.adjusted
                .unwrap_or(0.0)
                .total_cmp(&b.adjusted.unwrap_or(0.0))
                .then(a.count.cmp(&b.count))
        })
        .map(|(i, _)| i);
    let mut picks_json = Map::new();
    if method == "model" {
        picks_json.insert("top_pick".into(), json!(pick_id(0)));
    }
    if let Some(i) = picks.iter().position(|p| p.label == Some("best_value")) {
        picks_json.insert("best_value".into(), json!(pick_id(i)));
    }
    if let Some(i) = best_rated {
        picks_json.insert("best_rated".into(), json!(pick_id(i)));
    }

    let options: Vec<Value> = picks
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut badges: Vec<&str> = Vec::new();
            if method == "model" && i == 0 {
                badges.push("Top pick");
            }
            if let Some(l) = p.label.filter(|l| *l != "top_pick") {
                if let Some((_, badge)) = LABELS.iter().find(|(k, _)| *k == l) {
                    if !badge.is_empty() {
                        badges.push(badge);
                    }
                }
            }
            if best_rated == Some(i) {
                badges.push("Best rated");
            }
            option_json(i, p, ev, badges)
        })
        .collect();

    // The sources the picks cite, numbered as the model saw them.
    let mut cited: Vec<usize> = picks.iter().flat_map(|p| p.sources.clone()).collect();
    cited.sort_unstable();
    cited.dedup();
    let sources: Vec<Value> = cited
        .iter()
        .map(|&s| {
            let src = &ev.sources[s];
            json!({ "n": s + 1, "title": src.title, "url": src.url, "host": src.host })
        })
        .collect();

    let stores = if ev.sources.iter().any(|s| s.kind != Kind::Catalog) {
        "the web"
    } else {
        bestbuy::STORE
    };
    let mut out = Map::new();
    out.insert("query".into(), json!(req.query));
    if let Some(n) = &req.needs {
        out.insert("needs".into(), json!(n));
    }
    out.insert("store".into(), json!(stores));
    out.insert(
        "currency".into(),
        json!(picks
            .iter()
            .find_map(|p| p.currency.clone())
            .unwrap_or_else(|| "USD".into())),
    );
    out.insert("total".into(), json!(ev.candidates.len().max(picks.len())));
    out.insert(
        "considered".into(),
        json!({
            "pages": ev.sources.iter().filter(|s| s.read).count(),
            "products": ev.candidates.len(),
            "searches": ev.queries,
        }),
    );
    out.insert(
        "as_of".into(),
        json!(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)),
    );
    out.insert("method".into(), json!(method));
    if let Some(s) = summary {
        out.insert("summary".into(), json!(s));
    }
    let mut notes = ev.notes.clone();
    if let Some(n) = note {
        notes.insert(
            0,
            format!("Ranked by customer ratings, because the options could not be weighed: {n}."),
        );
    }
    if !notes.is_empty() {
        out.insert("note".into(), json!(notes.join(" ")));
    }
    out.insert("picks".into(), Value::Object(picks_json));
    out.insert("options".into(), Value::Array(options));
    out.insert("others".into(), json!([]));
    out.insert("sources".into(), Value::Array(sources));
    out.insert("disclaimer".into(), json!(DISCLAIMER));
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(name: &str, store: &str, price: Option<f64>, rating: Option<(f64, u64)>) -> Candidate {
        Candidate {
            name: name.into(),
            url: format!("https://{store}/{}", name.replace(' ', "-")),
            store: store.into(),
            images: vec![format!("https://{store}/img.jpg")],
            price,
            currency: price.map(|_| "USD".into()),
            rating: rating.map(|r| r.0),
            review_count: rating.map(|r| r.1),
            ..Candidate::default()
        }
    }

    fn evidence() -> Evidence {
        let src = |url: &str, kind: Kind, snippet: &str| Source {
            url: url.into(),
            title: format!("title of {url}"),
            host: product_page::host(url),
            snippet: snippet.into(),
            kind,
            read: true,
            ..Source::default()
        };
        let mut ev = Evidence {
            sources: vec![
                src(
                    "https://reviews.example/best-shoes",
                    Kind::Review,
                    "Our pick, the Cloud 5, costs $139.95.",
                ),
                src("https://shop.example/trail-5", Kind::Shop, ""),
                src("https://outlet.example/speed", Kind::Shop, ""),
            ],
            ..Evidence::default()
        };
        ev.sources[2].image = Some("https://outlet.example/og.jpg".into());
        add_candidate(
            &mut ev.candidates,
            cand(
                "Trail Runner 5",
                "shop.example",
                Some(129.95),
                Some((4.6, 1204)),
            ),
            1,
        );
        add_candidate(
            &mut ev.candidates,
            cand("Speedgoat", "outlet.example", Some(99.0), Some((5.0, 3))),
            2,
        );
        ev
    }

    fn req(count: usize) -> Request {
        parse_input(&json!({ "query": "trail running shoes", "count": count, "max_price": 150 }))
            .unwrap()
    }

    #[test]
    fn input_is_read_and_checked() {
        let r = parse_input(
            &json!({ "query": "  rain jacket ", "needs": " light,\n packable ",
                                     "min_price": 50, "max_price": 200.456 }),
        )
        .unwrap();
        assert_eq!(r.query, "rain jacket");
        assert_eq!(r.needs.as_deref(), Some("light, packable"));
        assert_eq!((r.min_price, r.max_price), (Some(50.0), Some(200.46)));
        assert_eq!(r.count, DEFAULT_COUNT);
        assert_eq!(
            parse_input(&json!({ "query": "x", "count": 50 }))
                .unwrap()
                .count,
            MAX_COUNT
        );
        let err = |v: Value| format!("{:#}", parse_input(&v).unwrap_err());
        assert!(err(json!({})).starts_with("give query"));
        assert!(err(json!({ "query": "x", "min_price": 9, "max_price": 1 })).contains("above"));
        assert!(err(json!({ "query": "x", "count": 0 })).contains("count"));
        assert_eq!(
            queries(&req(4)),
            (
                "best trail running shoes under 150 review".to_string(),
                "trail running shoes under 150 buy online".to_string()
            )
        );
        let best = parse_input(&json!({ "query": "Best carry-on suitcase" })).unwrap();
        assert_eq!(queries(&best).0, "Best carry-on suitcase review");
    }

    #[test]
    fn brave_hits_become_sources_and_pages_are_read_shops_first() {
        let hit = json!({ "title": "<strong>Trail</strong> shoes &amp; more", "url": "https://www.shop.example/t",
                          "description": "Cushioned.", "extra_snippets": ["Free returns."],
                          "thumbnail": { "src": "https://imgs.search.brave.com/x.jpg" } });
        let s = source_from_hit(&hit, Kind::Shop).unwrap();
        assert_eq!(s.title, "Trail shoes & more");
        assert_eq!(s.host, "shop.example");
        assert_eq!(s.snippet, "Cushioned. … Free returns.");
        assert_eq!(
            s.thumbnail.as_deref(),
            Some("https://imgs.search.brave.com/x.jpg")
        );
        assert!(source_from_hit(
            &json!({ "url": "https://www.youtube.com/watch?v=1" }),
            Kind::Review
        )
        .is_none());
        assert!(source_from_hit(&json!({ "url": "ftp://x" }), Kind::Review).is_none());

        let mut sources: Vec<Source> = (0..12)
            .map(|i| Source {
                url: format!("https://s{i}.example"),
                kind: if i % 2 == 0 { Kind::Review } else { Kind::Shop },
                ..Source::default()
            })
            .collect();
        sources.push(Source {
            kind: Kind::Catalog,
            ..Source::default()
        });
        assert_eq!(reading_order(&sources), [1, 3, 5, 7, 9, 0, 2, 4]);
    }

    #[test]
    fn the_same_product_from_one_store_is_kept_once_with_the_most_detail() {
        let mut list = Vec::new();
        add_candidate(&mut list, cand("Shoe", "a.example", None, None), 0);
        add_candidate(&mut list, cand("shoe", "a.example", Some(10.0), None), 1);
        add_candidate(&mut list, cand("Shoe", "b.example", Some(12.0), None), 2);
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].0.price, list[0].1), (Some(10.0), 1));
    }

    #[test]
    fn the_prompt_holds_the_request_the_products_and_the_sources() {
        let p = prompt(&req(3), &evidence());
        assert!(
            p.starts_with("Shopping request: trail running shoes\n"),
            "{p}"
        );
        assert!(p.contains("Budget: at most 150"));
        assert!(p.contains(
            "[C1] Trail Runner 5 | price: 129.95 USD | rating: 4.6/5 from 1,204 reviews | store: shop.example (S2)"
        ), "{p}");
        assert!(p.contains(
            "[S1] title of https://reviews.example/best-shoes (reviews.example, review search)"
        ));
        assert!(p.contains("Pick the 3 best options"));
    }

    #[test]
    fn the_models_options_are_held_to_the_evidence() {
        let ev = evidence();
        let options = json!([
            { "name": "Trail Runner 5", "candidate": "C1", "sources": ["S1", "S9"], "price": 99,
              "label": "top_pick", "why": "Grippy and cushioned.", "pros": ["grip", "", "comfort"],
              "cons": ["narrow"], "best_for": "muddy trails", "compare_with": [4, 3, 2] },
            { "name": "Made-up Shoe", "candidate": null, "sources": [] },
            { "name": "Cloud 5", "candidate": null, "sources": ["S1"], "price": 139.95, "currency": "usd",
              "label": "premium" },
            { "name": "Speedgoat", "candidate": 2, "label": "best_value" },
            { "name": "Trail Runner 5", "candidate": "C1" },
            { "name": "Ghost", "sources": ["S3"], "price": 80 }
        ]);
        let picks = assemble(&req(4), &ev, options.as_array().unwrap());
        let names: Vec<&str> = picks.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["Trail Runner 5", "Cloud 5", "Speedgoat", "Ghost"],
            "an option without a source is dropped, and a repeat too"
        );
        let first = &picks[0];
        assert_eq!(first.url, "https://shop.example/Trail-Runner-5");
        assert_eq!(
            (first.price, first.price_checked),
            (Some(129.95), true),
            "the page's price wins"
        );
        assert_eq!(
            first.sources,
            [1, 0],
            "its own page first; unknown ids dropped"
        );
        assert_eq!(first.pros, ["grip", "comfort"]);
        assert_eq!(first.label, Some("top_pick"));
        let cloud = &picks[1];
        assert_eq!(
            cloud.url, "https://reviews.example/best-shoes",
            "only the article to link to"
        );
        assert!(
            cloud.images.is_empty(),
            "an article's picture isn't the product's"
        );
        assert_eq!(
            (cloud.price, cloud.currency.as_deref(), cloud.price_checked),
            (Some(139.95), Some("USD"), false),
            "a price the cited text states is kept, unchecked"
        );
        let ghost = &picks[3];
        assert_eq!(ghost.price, None, "a price no source states is dropped");
        assert_eq!(
            ghost.images,
            ["https://outlet.example/og.jpg"],
            "a shop page's picture is used"
        );

        let mut picks = picks;
        link_comparable(&mut picks);
        assert_eq!(
            picks[0].comparable,
            [2, 1],
            "the model's numbers, as indexes of the picks; a dropped option's is skipped"
        );
        assert_eq!(picks[3].comparable, [0, 1, 2], "without a price, in order");
        assert_eq!(
            picks[2].comparable,
            [0, 1, 3],
            "else the closest in price first"
        );
    }

    #[test]
    fn without_the_model_the_products_are_ranked_by_their_ratings() {
        let picks = by_ratings(&req(4), &evidence());
        let names: Vec<&str> = picks.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["Trail Runner 5", "Speedgoat"],
            "1,204 reviews beat three perfect ones"
        );
        assert!(picks.iter().all(|p| p.price_checked && p.label.is_none()));
        let over = parse_input(&json!({ "query": "x", "max_price": 100 })).unwrap();
        assert_eq!(
            by_ratings(&over, &evidence()).len(),
            1,
            "over the budget is left out"
        );
    }

    #[test]
    fn replies_and_prices_are_read_carefully() {
        let r = parse_reply(
            "Sure!\n```json\n{\"summary\": \" Buy  it. \", \"options\": [{\"name\": \"A\"}]}\n```",
        )
        .unwrap();
        assert_eq!(r.summary.as_deref(), Some("Buy it."));
        assert_eq!(r.options.len(), 1);
        assert!(parse_reply("no json here").is_err());
        assert!(parse_reply("{\"summary\": \"x\"}").is_err());
        assert!(mentions_price("Now $1,299.00 at the shop", 1299.0));
        assert!(mentions_price("costs 129 dollars", 129.0));
        assert!(!mentions_price("model 1290", 129.0));
        assert!(!mentions_price("v2.129", 129.0));
        assert_eq!(index(&json!("C2"), 'C', 3), Some(1));
        assert_eq!(index(&json!("s3"), 'S', 3), Some(2));
        assert_eq!(index(&json!(4), 'C', 3), None);
        assert_eq!(index(&json!(null), 'C', 3), None);
    }

    #[test]
    fn the_result_carries_badges_reviews_and_cited_sources() {
        let ev = evidence();
        let options = json!([
            { "name": "Trail Runner 5", "candidate": "C1", "sources": ["S1"], "why": "Grippy." },
            { "name": "Speedgoat", "candidate": "C2", "label": "best_value" }
        ]);
        let mut picks = assemble(&req(4), &ev, options.as_array().unwrap());
        link_comparable(&mut picks);
        let r = result(
            &req(4),
            &ev,
            "model",
            Some("Get the Trail Runner.".into()),
            None,
            &picks,
        );
        assert_eq!(r["method"], "model");
        assert_eq!(r["store"], "the web");
        assert_eq!(r["currency"], "USD");
        assert_eq!(r["summary"], "Get the Trail Runner.");
        assert_eq!(
            r["picks"],
            json!({ "top_pick": "P1", "best_value": "P2", "best_rated": "P1" })
        );
        let first = &r["options"][0];
        assert_eq!(first["sku"], "P1");
        assert_eq!(first["badges"], json!(["Top pick", "Best rated"]));
        assert_eq!(first["reviews"]["verdict"], "excellent");
        assert_eq!(first["sources"], json!([2, 1]));
        assert_eq!(first["comparable"], json!(["P2"]));
        assert_eq!(r["options"][1]["badges"], json!(["Best value"]));
        assert_eq!(r["options"][1]["reviews"]["verdict"], "few");
        let cited: Vec<u64> = r["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["n"].as_u64().unwrap())
            .collect();
        assert_eq!(cited, [1, 2, 3]);

        let fallback = result(
            &req(4),
            &ev,
            "ratings",
            None,
            Some("the model timed out".into()),
            &picks,
        );
        assert!(fallback["picks"].get("top_pick").is_none());
        assert_eq!(fallback["options"][0]["badges"], json!(["Best rated"]));
        assert!(fallback["note"]
            .as_str()
            .unwrap()
            .starts_with("Ranked by customer ratings"));
    }

    #[test]
    fn reviews_are_read_with_the_count_in_mind() {
        assert_eq!(review_digest(None, 0).verdict, "none");
        let few = review_digest(Some(5.0), 3);
        assert_eq!((few.verdict, few.confidence), ("few", "low"));
        let ten = review_digest(Some(5.0), 10);
        assert_eq!((ten.verdict, ten.adjusted), ("good", Some(4.4)));
        let many = review_digest(Some(4.6), 1234);
        assert_eq!((many.verdict, many.confidence), ("excellent", "high"));
        assert!(many.summary.starts_with("4.6 stars from 1,234 reviews"));
        assert_eq!(review_digest(Some(3.0), 20).verdict, "mixed");
        assert_eq!(review_digest(Some(2.5), 100).verdict, "poor");
    }
}
