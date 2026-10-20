use super::{Tool, ToolContext};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use reqwest::StatusCode;
use serde_json::{json, Map, Value};
use std::time::Duration;

const DEFAULT_COUNT: usize = 5;
const MAX_COUNT: usize = 8;
/// Products fetched per search: the options shown, plus the pool their comparable alternatives
/// are picked from.
const POOL_SIZE: usize = 24;
const MAX_QUERY_CHARS: usize = 120;
const MAX_WORDS: usize = 8;
const MAX_IMAGES: usize = 6;
const MAX_FEATURES: usize = 6;
const MAX_SPECS: usize = 8;
const MAX_COMPARABLE: usize = 4;
/// Names, brands, features, and other short fields.
const MAX_TEXT_CHARS: usize = 200;
const MAX_DESCRIPTION_CHARS: usize = 400;
/// Fewer reviews than this and an average says little.
const FEW_REVIEWS: u64 = 5;
/// The adjusted rating shrinks the average toward `PRIOR_MEAN` as if `PRIOR_REVIEWS` typical
/// reviews were added, so a 5.0 from three reviews ranks below a 4.6 from a thousand.
const PRIOR_MEAN: f64 = 4.0;
const PRIOR_REVIEWS: f64 = 15.0;
/// The orders a search can come back in, with Best Buy's name for each.
const SORTS: &[(&str, &str)] = &[
    ("best_selling", "bestSellingRank.asc"),
    ("rating", "customerReviewAverage.desc"),
    ("price_low", "salePrice.asc"),
    ("price_high", "salePrice.desc"),
];
const DEFAULT_SORT: &str = "best_selling";
/// Words that narrow a keyword search without describing the product.
const STOP_WORDS: &[&str] = &[
    "a",
    "an",
    "the",
    "for",
    "with",
    "and",
    "or",
    "of",
    "to",
    "in",
    "on",
    "best",
    "good",
    "top",
    "cheap",
    "cheapest",
    "affordable",
    "budget",
    "new",
    "buy",
    "product",
    "products",
    "option",
    "options",
    "recommend",
    "recommended",
];
/// The product attributes asked for, so a search doesn't carry the whole record.
const SHOW: &str = "sku,name,manufacturer,modelNumber,color,condition,shortDescription,url,image,\
                    images,salePrice,regularPrice,customerReviewAverage,customerReviewCount,\
                    onlineAvailability,inStoreAvailability,freeShipping,features,details,\
                    categoryPath,bestSellingRank,class";
/// Best Buy allows a few requests a second on a developer key; one retry after this pause is
/// enough.
const RATE_LIMIT_PAUSE: Duration = Duration::from_secs(1);
const STORE: &str = "Best Buy";
const DISCLAIMER: &str = "Prices, sale savings, availability and ratings are Best Buy's (US) at \
                          the time of the search and can change. The adjusted rating is a \
                          statistical reading of the average and the number of reviews behind \
                          it, not a judgment of the product.";
const USER_AGENT: &str = concat!(
    "cedar/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/hanneshapke/cedar)"
);

/// Searches a store's catalog for products and shows the user the options as a card drawn by the
/// chat front end: each product's photos, price and sale savings, customer rating with the number
/// of reviews behind it, availability, key features, and comparable alternatives.
///
/// The products come from Best Buy's Products API, which needs a free developer key. The tool
/// runs in the cedar process, since the sandbox has no network and the key must stay out of it.
/// It reads the reviews itself: an *adjusted* rating shrinks each product's average toward a
/// typical one in proportion to how few reviews it rests on, and a verdict (`excellent`, `good`,
/// `mixed`, `poor`, `few`, `none`) and a confidence say what the reviews amount to. The result is
/// JSON: `{"query", "store", "currency", "total", "as_of", "sort", "filters", "options":
/// [{"sku", "name", "brand", "price", "regular_price", "savings_pct", "images", "url",
/// "description", "features", "specs", "category", "availability", "reviews": {"average",
/// "count", "adjusted", "verdict", "confidence", "summary"}, "comparable": [sku], ...}],
/// "others": [...], "picks": {"best_rated", "best_value", "most_popular"}, "disclaimer"}`.
pub struct SearchProducts {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl SearchProducts {
    /// `base_url` is the Products API's collection endpoint, which the search filter goes after
    /// in parentheses (see `DEFAULT_PRODUCTS_API_URL`).
    pub fn new(api_key: String, base_url: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent(USER_AGENT)
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            http,
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    /// Check a key with one tiny search, before it is saved. Being rate limited counts as
    /// accepted: Best Buy only says so to a key it knows.
    pub async fn check_key(&self, timeout: Duration) -> Result<()> {
        let resp = self
            .http
            .get(format!("{}(search=tv)", self.base_url))
            .header("Accept", "application/json")
            .query(&[
                ("apiKey", self.api_key.as_str()),
                ("format", "json"),
                ("show", "sku"),
                ("pageSize", "1"),
            ])
            .timeout(timeout)
            .send()
            .await
            .context("sending a test search to Best Buy")?;
        let status = resp.status();
        match status {
            s if s.is_success() || s == StatusCode::TOO_MANY_REQUESTS => Ok(()),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                bail!("{STORE} rejected the API key ({status})")
            }
            _ => {
                let text = resp.text().await.unwrap_or_default();
                bail!("{STORE} error {status}: {}", error_message(&text))
            }
        }
    }

    async fn send(&self, req: &Request, timeout: Duration) -> Result<reqwest::Response> {
        let sort = SORTS
            .iter()
            .find(|(name, _)| *name == req.sort)
            .map(|(_, by)| *by)
            .unwrap_or(SORTS[0].1);
        self.http
            .get(format!("{}({})", self.base_url, filter(req)))
            .header("Accept", "application/json")
            .query(&[
                ("apiKey", self.api_key.as_str()),
                ("format", "json"),
                ("show", SHOW),
                ("sort", sort),
                ("pageSize", &POOL_SIZE.to_string()),
            ])
            .timeout(timeout)
            .send()
            .await
            .context("sending the product search")
    }

    /// The products matching `req`, in the order asked for, and how many matched in all.
    async fn search(&self, req: &Request, timeout: Duration) -> Result<(Vec<Product>, u64)> {
        let mut resp = self.send(req, timeout).await?;
        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            tokio::time::sleep(RATE_LIMIT_PAUSE).await;
            resp = self.send(req, timeout).await?;
        }
        let status = resp.status();
        let text = resp
            .text()
            .await
            .context("reading the product search response")?;
        match status {
            s if s.is_success() => {}
            StatusCode::BAD_REQUEST => {
                bail!("{STORE} rejected the search: {}", error_message(&text))
            }
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                bail!("{STORE} rejected the API key ({status})")
            }
            StatusCode::TOO_MANY_REQUESTS => {
                bail!("{STORE}'s API is rate limited; wait a moment before searching again")
            }
            _ => bail!("{STORE} error {status}: {}", error_message(&text)),
        }
        let body: Value =
            serde_json::from_str(&text).context("parsing the product search response")?;
        let products = body["products"]
            .as_array()
            .ok_or_else(|| anyhow!("{STORE} sent no product list"))?
            .iter()
            .filter_map(parse_product)
            .collect();
        Ok((products, body["total"].as_u64().unwrap_or(0)))
    }
}

#[async_trait]
impl Tool for SearchProducts {
    fn name(&self) -> &str {
        "search_products"
    }

    fn description(&self) -> &str {
        "Search Best Buy's catalog for products and show the user the options as a card in the \
         chat: each product's photos, price and sale savings, customer rating with how many \
         reviews it rests on, availability, key features, and comparable alternatives. Use it \
         when the user wants to buy, compare, or choose a product: electronics, phones, laptops, \
         TVs, headphones, cameras, games, appliances, and home gear. Give query as the words a \
         shop's search box would take, e.g. \"noise cancelling headphones\" or \"65 inch OLED \
         TV\", and the filters when the user named a budget or a minimum rating. The tool reads \
         the reviews for you: each product's adjusted rating discounts a high average that rests \
         on few reviews, with a verdict and a confidence. Afterwards, review the options in a few \
         sentences: which one you would pick and why, weighing the ratings and the number of \
         reviews behind them, the price against the comparable options, and the features; say \
         when the reviews are too few to trust. Don't repeat the prices and ratings the card \
         already shows."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "query": { "type": "string",
                               "description": "What to look for, as search words, e.g. \"wireless noise cancelling headphones\"." },
                    "count": { "type": "integer", "minimum": 1, "maximum": MAX_COUNT,
                               "description": "How many options to show, default 5." },
                    "min_price": { "type": "number", "minimum": 0, "description": "Only products at or above this price, in US dollars." },
                    "max_price": { "type": "number", "minimum": 0, "description": "Only products at or below this price, in US dollars." },
                    "min_rating": { "type": "number", "minimum": 1, "maximum": 5,
                                    "description": "Only products whose customer rating is at least this, out of 5." },
                    "in_stock": { "type": "boolean", "description": "Only products that can be ordered online now, default true." },
                    "sort": { "type": "string", "enum": SORTS.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
                              "description": "The order the options come in: best_selling (default), rating, price_low, or price_high." }
                },
                "required": ["query"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let req = parse_input(input)?;
        let (pool, total) = self.search(&req, ctx.timeout).await?;
        if pool.is_empty() {
            bail!(
                "no products found for {:?}; try fewer or different words, or looser filters",
                req.query
            );
        }
        let rated: Vec<(Product, Reviews)> = pool
            .into_iter()
            .map(|p| {
                let reviews = review_digest(p.average, p.count);
                (p, reviews)
            })
            .collect();
        let shown = req.count.min(rated.len());
        let (options, others) = rated.split_at(shown);

        let mut result = Map::new();
        result.insert("query".into(), json!(req.query));
        result.insert("store".into(), json!(STORE));
        result.insert("currency".into(), json!("USD"));
        result.insert("total".into(), json!(total.max(rated.len() as u64)));
        result.insert(
            "as_of".into(),
            json!(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)),
        );
        result.insert("sort".into(), json!(req.sort));
        result.insert("filters".into(), req.filters_json());
        result.insert(
            "options".into(),
            Value::Array(
                options
                    .iter()
                    .map(|(p, r)| {
                        let mut v = product_json(p, r, true);
                        v["comparable"] = json!(comparable(p, &rated));
                        v
                    })
                    .collect(),
            ),
        );
        result.insert(
            "others".into(),
            Value::Array(
                others
                    .iter()
                    .map(|(p, r)| product_json(p, r, false))
                    .collect(),
            ),
        );
        result.insert("picks".into(), picks(options));
        result.insert("disclaimer".into(), json!(DISCLAIMER));
        Ok(Value::Object(result).to_string())
    }
}

// -- input ------------------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
struct Request {
    query: String,
    /// The keywords searched for, every one of which a product must match.
    words: Vec<String>,
    count: usize,
    min_price: Option<f64>,
    max_price: Option<f64>,
    min_rating: Option<f64>,
    in_stock: bool,
    sort: &'static str,
}

impl Request {
    fn filters_json(&self) -> Value {
        let mut f = Map::new();
        if let Some(p) = self.min_price {
            f.insert("min_price".into(), json!(p));
        }
        if let Some(p) = self.max_price {
            f.insert("max_price".into(), json!(p));
        }
        if let Some(r) = self.min_rating {
            f.insert("min_rating".into(), json!(r));
        }
        f.insert("in_stock".into(), json!(self.in_stock));
        Value::Object(f)
    }
}

/// The search words in `query`: lower-cased runs of letters and digits, without the stop words.
/// "under 300" (or "below $300") becomes the price cap instead, when none is set.
fn keywords(query: &str, max_price: &mut Option<f64>) -> Vec<String> {
    let tokens: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '$' && c != '.')
        .map(|t| t.trim_matches(|c: char| c == '$' || c == '.').to_string())
        .filter(|t| !t.is_empty())
        .collect();
    let mut words = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let t = &tokens[i];
        if matches!(t.as_str(), "under" | "below" | "max") {
            if let Some(price) = tokens.get(i + 1).and_then(|n| n.parse::<f64>().ok()) {
                if max_price.is_none() && price > 0.0 {
                    *max_price = Some(price);
                }
                i += 2;
                continue;
            }
        }
        if !STOP_WORDS.contains(&t.as_str())
            && t.chars().all(char::is_alphanumeric)
            && !words.contains(t)
        {
            words.push(t.clone());
        }
        i += 1;
    }
    words.truncate(MAX_WORDS);
    words
}

fn parse_input(input: &Value) -> Result<Request> {
    let query: String = input["query"]
        .as_str()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .ok_or_else(|| {
            anyhow!("give query: what to search for, e.g. \"noise cancelling headphones\"")
        })?
        .chars()
        .take(MAX_QUERY_CHARS)
        .collect();
    let money = |key: &str| -> Result<Option<f64>> {
        match &input[key] {
            Value::Null => Ok(None),
            v => {
                let n = v
                    .as_f64()
                    .ok_or_else(|| anyhow!("{key} is a number of dollars"))?;
                if !n.is_finite() || n < 0.0 {
                    bail!("{key} can't be negative");
                }
                Ok(Some((n * 100.0).round() / 100.0))
            }
        }
    };
    let min_price = money("min_price")?;
    let mut max_price = money("max_price")?;
    if let (Some(lo), Some(hi)) = (min_price, max_price) {
        if lo > hi {
            bail!("min_price ({lo}) is above max_price ({hi})");
        }
    }
    let words = keywords(&query, &mut max_price);
    if words.is_empty() {
        bail!("query {query:?} has no words to search for; name the product");
    }
    let count = match &input["count"] {
        Value::Null => DEFAULT_COUNT,
        v => v
            .as_u64()
            .filter(|n| *n >= 1)
            .ok_or_else(|| anyhow!("count is a whole number from 1 to {MAX_COUNT}"))?
            .min(MAX_COUNT as u64) as usize,
    };
    let min_rating = match &input["min_rating"] {
        Value::Null => None,
        v => {
            let r = v
                .as_f64()
                .filter(|r| (1.0..=5.0).contains(r))
                .ok_or_else(|| anyhow!("min_rating is a number from 1 to 5"))?;
            Some((r * 10.0).round() / 10.0)
        }
    };
    let in_stock = match &input["in_stock"] {
        Value::Null => true,
        v => v
            .as_bool()
            .ok_or_else(|| anyhow!("in_stock is true or false"))?,
    };
    let sort = match input["sort"]
        .as_str()
        .map(|s| s.trim().to_ascii_lowercase())
    {
        None => DEFAULT_SORT,
        Some(s) if s.is_empty() => DEFAULT_SORT,
        Some(s) => SORTS
            .iter()
            .map(|(name, _)| *name)
            .find(|name| *name == s)
            .ok_or_else(|| {
                anyhow!(
                    "sort {s:?} is not one of {}",
                    SORTS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
                )
            })?,
    };
    Ok(Request {
        query,
        words,
        count,
        min_price,
        max_price,
        min_rating,
        in_stock,
        sort,
    })
}

/// The search as the Products API wants it in the path: `(search=a&search=b)&(salePrice<=300)`.
/// The words hold letters and digits only (see [`keywords`]) and the numbers are checked, so
/// nothing in it needs escaping.
fn filter(req: &Request) -> String {
    let words: Vec<String> = req.words.iter().map(|w| format!("search={w}")).collect();
    let mut parts = vec![format!("({})", words.join("&"))];
    if let Some(p) = req.min_price {
        parts.push(format!("(salePrice>={p})"));
    }
    if let Some(p) = req.max_price {
        parts.push(format!("(salePrice<={p})"));
    }
    if let Some(r) = req.min_rating {
        parts.push(format!("(customerReviewAverage>={r})"));
    }
    if req.in_stock {
        parts.push("(onlineAvailability=true)".into());
    }
    parts.join("&")
}

/// The message in an error body, which the API sends as JSON when it can, else its start.
fn error_message(text: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        for m in [
            &v["error"]["message"],
            &v["errorMessage"],
            &v["message"],
            &v["error"],
        ] {
            if let Some(s) = m.as_str().filter(|s| !s.trim().is_empty()) {
                return s.trim().chars().take(MAX_TEXT_CHARS).collect();
            }
        }
    }
    let short: String = text.trim().chars().take(MAX_TEXT_CHARS).collect();
    if short.is_empty() {
        "no details".into()
    } else {
        short
    }
}

// -- products ---------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct Product {
    sku: u64,
    name: String,
    brand: Option<String>,
    model: Option<String>,
    color: Option<String>,
    condition: Option<String>,
    description: Option<String>,
    url: Option<String>,
    /// https addresses, the main photo first.
    images: Vec<String>,
    price: f64,
    regular_price: Option<f64>,
    average: Option<f64>,
    count: u64,
    online: bool,
    in_store: bool,
    free_shipping: bool,
    features: Vec<String>,
    specs: Vec<(String, String)>,
    /// The category path, without the store's root entry; the last one is the product's own.
    categories: Vec<String>,
    class: Option<String>,
    rank: Option<u64>,
}

impl Product {
    fn category(&self) -> Option<&str> {
        self.categories.last().map(String::as_str)
    }

    /// Percent off the regular price, when there is a sale.
    fn savings_pct(&self) -> Option<f64> {
        let regular = self.regular_price?;
        if regular > self.price && regular > 0.0 {
            Some(((1.0 - self.price / regular) * 1000.0).round() / 10.0)
        } else {
            None
        }
    }
}

/// `value` as trimmed text cut to `max` characters, or `None` if it is missing or blank.
fn text(value: &Value, max: usize) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(max).collect())
}

fn https(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|u| u.starts_with("https://") && !u.contains(char::is_whitespace))
        .map(String::from)
}

/// A product record as the Products API returns it. `None` without a sku, a name, and a price.
fn parse_product(v: &Value) -> Option<Product> {
    let sku = v["sku"]
        .as_u64()
        .or_else(|| v["sku"].as_str().and_then(|s| s.parse().ok()))?;
    let name = text(&v["name"], MAX_TEXT_CHARS)?;
    let price = v["salePrice"]
        .as_f64()
        .or_else(|| v["regularPrice"].as_f64())
        .filter(|p| p.is_finite() && *p >= 0.0)?;

    // The main photo, then the others: the record's `image` and every `images` entry that is a
    // picture, the one flagged primary first.
    let mut images: Vec<String> = Vec::new();
    let mut push = |u: Option<String>| {
        if let Some(u) = u {
            if !images.contains(&u) && images.len() < MAX_IMAGES {
                images.push(u);
            }
        }
    };
    push(https(&v["image"]));
    let listed: Vec<&Value> = v["images"].as_array().into_iter().flatten().collect();
    for entry in listed
        .iter()
        .filter(|e| e["primary"].as_bool() == Some(true))
        .chain(
            listed
                .iter()
                .filter(|e| e["primary"].as_bool() != Some(true)),
        )
    {
        push(https(&entry["href"]));
    }

    let features = v["features"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| text(&f["feature"], MAX_TEXT_CHARS).or_else(|| text(f, MAX_TEXT_CHARS)))
        .take(MAX_FEATURES)
        .collect();
    let specs = v["details"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| Some((text(&d["name"], 80)?, text(&d["value"], MAX_TEXT_CHARS)?)))
        .take(MAX_SPECS)
        .collect();
    let categories: Vec<String> = v["categoryPath"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| text(&c["name"], 80))
        .filter(|n| n != STORE)
        .collect();

    Some(Product {
        sku,
        name,
        brand: text(&v["manufacturer"], 80),
        model: text(&v["modelNumber"], 80),
        color: text(&v["color"], 60),
        condition: text(&v["condition"], 40),
        description: text(&v["shortDescription"], MAX_DESCRIPTION_CHARS),
        url: https(&v["url"]),
        images,
        price,
        regular_price: v["regularPrice"].as_f64().filter(|p| p.is_finite()),
        average: v["customerReviewAverage"]
            .as_f64()
            .filter(|a| (0.0..=5.0).contains(a)),
        count: v["customerReviewCount"].as_u64().unwrap_or(0),
        online: v["onlineAvailability"].as_bool().unwrap_or(false),
        in_store: v["inStoreAvailability"].as_bool().unwrap_or(false),
        free_shipping: v["freeShipping"].as_bool().unwrap_or(false),
        features,
        specs,
        categories,
        class: text(&v["class"], 80),
        rank: v["bestSellingRank"].as_u64().filter(|r| *r > 0),
    })
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
            summary: "No customer reviews yet.".into(),
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

/// Up to `MAX_COMPARABLE` other products from the pool that could stand in for `target`: those
/// in its own category first, then its class, then any, the closest in price (and then the
/// better reviewed) first.
fn comparable(target: &Product, pool: &[(Product, Reviews)]) -> Vec<u64> {
    let mut ranked: Vec<(u8, u64, i64, u64)> = pool
        .iter()
        .filter(|(p, _)| p.sku != target.sku)
        .map(|(p, r)| {
            let tier = if p.category().is_some() && p.category() == target.category() {
                0
            } else if p.class.is_some() && p.class == target.class {
                1
            } else {
                2
            };
            let gap = if target.price > 0.0 {
                ((p.price - target.price).abs() / target.price * 1000.0) as u64
            } else {
                (p.price * 100.0) as u64
            };
            let rating = -(r.adjusted.unwrap_or(0.0) * 10.0) as i64;
            (tier, gap, rating, p.sku)
        })
        .collect();
    ranked.sort();
    ranked
        .into_iter()
        .take(MAX_COMPARABLE)
        .map(|(_, _, _, sku)| sku)
        .collect()
}

/// Which of the shown options stands out: the best reviewed, the cheapest well-reviewed one, and
/// the best seller. A pick is left out when no option qualifies.
fn picks(options: &[(Product, Reviews)]) -> Value {
    let judged = |r: &Reviews| r.count >= FEW_REVIEWS && r.adjusted.is_some();
    let best_rated = options
        .iter()
        .filter(|(_, r)| judged(r))
        .max_by(|(_, a), (_, b)| {
            a.adjusted
                .partial_cmp(&b.adjusted)
                .unwrap()
                .then(a.count.cmp(&b.count))
        })
        .map(|(p, _)| p.sku);
    let best_value = options
        .iter()
        .filter(|(_, r)| judged(r) && r.adjusted.unwrap_or(0.0) >= 4.0)
        .min_by(|(a, _), (b, _)| a.price.partial_cmp(&b.price).unwrap())
        .map(|(p, _)| p.sku);
    let most_popular = options
        .iter()
        .filter(|(p, _)| p.rank.is_some())
        .min_by_key(|(p, _)| p.rank)
        .or_else(|| {
            options
                .iter()
                .filter(|(_, r)| r.count > 0)
                .max_by_key(|(_, r)| r.count)
        })
        .map(|(p, _)| p.sku);
    let mut out = Map::new();
    for (key, sku) in [
        ("best_rated", best_rated),
        ("best_value", best_value),
        ("most_popular", most_popular),
    ] {
        if let Some(sku) = sku {
            out.insert(key.into(), json!(sku));
        }
    }
    Value::Object(out)
}

/// A product as the result shows it: everything for a shown option, the essentials for one of
/// the others.
fn product_json(p: &Product, r: &Reviews, full: bool) -> Value {
    let mut v = Map::new();
    v.insert("sku".into(), json!(p.sku));
    v.insert("name".into(), json!(p.name));
    if let Some(b) = &p.brand {
        v.insert("brand".into(), json!(b));
    }
    v.insert("price".into(), json!(p.price));
    if let Some(regular) = p.regular_price.filter(|r| *r > p.price) {
        v.insert("regular_price".into(), json!(regular));
    }
    if let Some(s) = p.savings_pct() {
        v.insert("savings_pct".into(), json!(s));
    }
    if let Some(u) = &p.url {
        v.insert("url".into(), json!(u));
    }
    if let Some(c) = p.category() {
        v.insert("category".into(), json!(c));
    }
    v.insert(
        "availability".into(),
        json!({ "online": p.online, "in_store": p.in_store, "free_shipping": p.free_shipping }),
    );
    if let Some(rank) = p.rank {
        v.insert("rank".into(), json!(rank));
    }
    v.insert(
        "reviews".into(),
        json!({
            "average": r.average, "count": r.count, "adjusted": r.adjusted,
            "verdict": r.verdict, "confidence": r.confidence, "summary": r.summary,
        }),
    );
    if full {
        if let Some(m) = &p.model {
            v.insert("model".into(), json!(m));
        }
        if let Some(c) = &p.color {
            v.insert("color".into(), json!(c));
        }
        if let Some(c) = &p.condition {
            v.insert("condition".into(), json!(c));
        }
        if let Some(d) = &p.description {
            v.insert("description".into(), json!(d));
        }
        v.insert("images".into(), json!(p.images));
        v.insert("features".into(), json!(p.features));
        v.insert(
            "specs".into(),
            Value::Array(
                p.specs
                    .iter()
                    .map(|(n, val)| json!({ "name": n, "value": val }))
                    .collect(),
            ),
        );
        v.insert("categories".into(), json!(p.categories));
    } else if let Some(first) = p.images.first() {
        v.insert("image".into(), json!(first));
    }
    Value::Object(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(sku: u64, name: &str, price: f64, avg: Option<f64>, count: u64) -> Value {
        json!({
            "sku": sku, "name": name, "manufacturer": "Sony", "modelNumber": format!("M{sku}"),
            "salePrice": price, "regularPrice": price,
            "customerReviewAverage": avg, "customerReviewCount": count,
            "url": format!("https://shop.example/{sku}"),
            "image": format!("https://img.example/{sku}.jpg"),
            "onlineAvailability": true, "inStoreAvailability": false, "freeShipping": true,
            "categoryPath": [{ "name": "Best Buy" }, { "name": "Audio" }, { "name": "Headphones" }],
            "class": "HEADPHONES", "bestSellingRank": sku,
        })
    }

    #[test]
    fn the_query_becomes_search_words_and_a_budget() {
        let req = parse_input(
            &json!({ "query": "Best wireless noise-cancelling headphones under $300" }),
        )
        .unwrap();
        assert_eq!(req.words, ["wireless", "noise", "cancelling", "headphones"]);
        assert_eq!(req.max_price, Some(300.0));
        assert_eq!(req.count, DEFAULT_COUNT);
        assert!(req.in_stock);
        assert_eq!(req.sort, "best_selling");
        assert_eq!(
            filter(&req),
            "(search=wireless&search=noise&search=cancelling&search=headphones)&(salePrice<=300)&(onlineAvailability=true)"
        );

        let req = parse_input(&json!({
            "query": "65 inch OLED TV", "count": 20, "min_price": 800, "max_price": 1999.999,
            "min_rating": 4.25, "in_stock": false, "sort": "Price_Low",
        }))
        .unwrap();
        assert_eq!(req.words, ["65", "inch", "oled", "tv"]);
        assert_eq!(req.count, MAX_COUNT, "count is capped");
        assert_eq!((req.min_price, req.max_price), (Some(800.0), Some(2000.0)));
        assert_eq!(req.min_rating, Some(4.3));
        assert_eq!(req.sort, "price_low");
        assert_eq!(
            filter(&req),
            "(search=65&search=inch&search=oled&search=tv)&(salePrice>=800)&(salePrice<=2000)&(customerReviewAverage>=4.3)"
        );
        // A max_price given outright wins over one in the words.
        let req = parse_input(&json!({ "query": "laptop under 500", "max_price": 700 })).unwrap();
        assert_eq!(req.max_price, Some(700.0));
        assert_eq!(req.words, ["laptop"]);
    }

    #[test]
    fn bad_input_is_rejected() {
        let err = |input: Value| format!("{:#}", parse_input(&input).unwrap_err());
        assert!(err(json!({})).starts_with("give query"));
        assert!(err(json!({ "query": "the best" })).contains("no words to search for"));
        assert!(err(json!({ "query": "tv", "count": 0 })).contains("count is a whole number"));
        assert!(err(json!({ "query": "tv", "max_price": -1 })).contains("can't be negative"));
        assert!(
            err(json!({ "query": "tv", "min_price": 50, "max_price": 20 }))
                .contains("above max_price")
        );
        assert!(err(json!({ "query": "tv", "min_rating": 6 })).contains("from 1 to 5"));
        assert!(err(json!({ "query": "tv", "in_stock": "yes" })).contains("true or false"));
        assert!(err(json!({ "query": "tv", "sort": "newest" })).contains("not one of best_selling"));
    }

    #[test]
    fn a_product_record_is_read_with_its_pictures_first_primary() {
        let mut v = record(
            42,
            "  WH-1000XM5 Wireless Headphones ",
            299.99,
            Some(4.7),
            2345,
        );
        v["salePrice"] = json!(299.99);
        v["regularPrice"] = json!(399.99);
        v["images"] = json!([
            { "href": "https://img.example/42-side.jpg", "primary": false },
            { "href": "http://img.example/plain.jpg", "primary": true },
            { "href": "https://img.example/42-front.jpg", "primary": true },
            { "href": "https://img.example/42.jpg" },
        ]);
        v["features"] = json!([{ "feature": "30-hour battery" }, { "feature": "" }, "Multipoint"]);
        v["details"] = json!([{ "name": "Weight", "value": "250 g" }, { "name": "Color" }]);
        let p = parse_product(&v).unwrap();
        assert_eq!(p.name, "WH-1000XM5 Wireless Headphones");
        assert_eq!(
            p.images,
            [
                "https://img.example/42.jpg",
                "https://img.example/42-front.jpg",
                "https://img.example/42-side.jpg"
            ],
            "the main photo, then the primary, then the rest; http is dropped"
        );
        assert_eq!(p.savings_pct(), Some(25.0));
        assert_eq!(p.features, ["30-hour battery", "Multipoint"]);
        assert_eq!(p.specs, [("Weight".to_string(), "250 g".to_string())]);
        assert_eq!(p.categories, ["Audio", "Headphones"]);
        assert_eq!(p.category(), Some("Headphones"));
        assert_eq!(p.rank, Some(42));
        assert_eq!(
            parse_product(&json!({ "sku": 1, "name": "x" })),
            None,
            "a product needs a price"
        );
        assert_eq!(
            parse_product(&json!({ "name": "x", "salePrice": 1 })),
            None,
            "and a sku"
        );
    }

    #[test]
    fn reviews_are_read_with_the_count_in_mind() {
        let none = review_digest(None, 0);
        assert_eq!((none.verdict, none.adjusted), ("none", None));
        assert_eq!(none.summary, "No customer reviews yet.");
        let few = review_digest(Some(5.0), 3);
        assert_eq!((few.verdict, few.confidence), ("few", "low"));
        assert_eq!(
            few.summary,
            "5.0 stars, but from only 3 reviews: too few to judge."
        );
        let many = review_digest(Some(4.6), 1234);
        assert_eq!(
            (many.verdict, many.confidence, many.adjusted),
            ("excellent", "high", Some(4.6))
        );
        assert_eq!(
            many.summary,
            "4.6 stars from 1,234 reviews (4.6 adjusted for how many there are): consistently well reviewed."
        );
        // A perfect score from ten reviews is pulled below a 4.6 from a thousand.
        let ten = review_digest(Some(5.0), 10);
        assert_eq!(
            (ten.verdict, ten.confidence, ten.adjusted),
            ("good", "low", Some(4.4))
        );
        assert_eq!(review_digest(Some(3.0), 20).verdict, "mixed");
        assert_eq!(review_digest(Some(2.5), 100).verdict, "poor");
        assert_eq!(review_digest(Some(4.0), 500).verdict, "good");
    }

    #[test]
    fn picks_and_comparable_options_come_from_the_pool() {
        let pool: Vec<(Product, Reviews)> = [
            (1, "A", 300.0, Some(4.8), 900),
            (2, "B", 250.0, Some(4.6), 300),
            (3, "C", 120.0, Some(4.4), 40),
            (4, "D", 90.0, Some(5.0), 2),
        ]
        .into_iter()
        .map(|(sku, name, price, avg, count)| {
            let mut v = record(sku, name, price, avg, count);
            if sku == 4 {
                v["categoryPath"] =
                    json!([{ "name": "Best Buy" }, { "name": "Audio" }, { "name": "Earbuds" }]);
                v["bestSellingRank"] = Value::Null;
            }
            let p = parse_product(&v).unwrap();
            let r = review_digest(p.average, p.count);
            (p, r)
        })
        .collect();
        let picks = picks(&pool);
        assert_eq!(
            picks,
            json!({ "best_rated": 1, "best_value": 3, "most_popular": 1 })
        );
        // From B's point of view: A and C share its category (A closer in price), then D.
        assert_eq!(comparable(&pool[1].0, &pool), [1, 3, 4]);
        assert_eq!(
            comparable(&pool[3].0, &pool),
            [3, 2, 1],
            "same class, closest price first"
        );

        let full = product_json(&pool[0].0, &pool[0].1, true);
        assert_eq!(full["reviews"]["verdict"], "excellent");
        assert!(full["images"].is_array() && full.get("image").is_none());
        let brief = product_json(&pool[0].0, &pool[0].1, false);
        assert_eq!(brief["image"], "https://img.example/1.jpg");
        assert!(brief.get("images").is_none() && brief.get("features").is_none());
    }

    #[test]
    fn error_bodies_are_summarized() {
        assert_eq!(
            error_message(r#"{"error": {"message": "Invalid query"}}"#),
            "Invalid query"
        );
        assert_eq!(error_message(r#"{"errorMessage": "bad key"}"#), "bad key");
        assert_eq!(
            error_message("<html>Forbidden</html>"),
            "<html>Forbidden</html>"
        );
        assert_eq!(error_message("  "), "no details");
    }
}
