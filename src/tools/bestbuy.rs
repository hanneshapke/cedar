//! Best Buy's Products API as a source for `search_products` (see [`super::products`]): its
//! catalog lists electronics, appliances and home gear with prices, photos, availability and
//! customer ratings. It needs a free developer key, and is used beside the web search when one is
//! set (in the environment or saved by an admin).

use super::products::Candidate;
use anyhow::{anyhow, bail, Context, Result};
use reqwest::StatusCode;
use serde_json::Value;
use std::time::Duration;

/// Products asked for per search.
const POOL_SIZE: usize = 8;
const MAX_WORDS: usize = 8;
const MAX_IMAGES: usize = 6;
const MAX_FEATURES: usize = 6;
const MAX_SPECS: usize = 8;
/// Names, brands, features, and other short fields.
const MAX_TEXT_CHARS: usize = 200;
const MAX_DESCRIPTION_CHARS: usize = 400;
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
pub(crate) const STORE: &str = "Best Buy";
const USER_AGENT: &str = concat!(
    "cedar/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/hanneshapke/cedar)"
);

/// A client for Best Buy's Products API.
pub struct BestBuy {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl BestBuy {
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

    async fn send(&self, filter: &str, timeout: Duration) -> Result<reqwest::Response> {
        self.http
            .get(format!("{}({filter})", self.base_url))
            .header("Accept", "application/json")
            .query(&[
                ("apiKey", self.api_key.as_str()),
                ("format", "json"),
                ("show", SHOW),
                ("sort", "bestSellingRank.asc"),
                ("pageSize", &POOL_SIZE.to_string()),
            ])
            .timeout(timeout)
            .send()
            .await
            .context("sending the product search")
    }

    /// The best-selling products in the catalog that match `query` and the price range, as
    /// candidates. An empty list when the query has no words to search for.
    pub(crate) async fn candidates(
        &self,
        query: &str,
        min_price: Option<f64>,
        max_price: Option<f64>,
        timeout: Duration,
    ) -> Result<Vec<Candidate>> {
        let mut max_price = max_price;
        let words = keywords(query, &mut max_price);
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let filter = filter(&words, min_price, max_price);
        let mut resp = self.send(&filter, timeout).await?;
        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            tokio::time::sleep(RATE_LIMIT_PAUSE).await;
            resp = self.send(&filter, timeout).await?;
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
            StatusCode::TOO_MANY_REQUESTS => bail!("{STORE}'s API is rate limited"),
            _ => bail!("{STORE} error {status}: {}", error_message(&text)),
        }
        let body: Value =
            serde_json::from_str(&text).context("parsing the product search response")?;
        Ok(body["products"]
            .as_array()
            .ok_or_else(|| anyhow!("{STORE} sent no product list"))?
            .iter()
            .filter_map(parse_product)
            .collect())
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

/// The search as the Products API wants it in the path: `(search=a&search=b)&(salePrice<=300)`.
/// The words hold letters and digits only (see [`keywords`]) and the numbers are checked, so
/// nothing in it needs escaping.
fn filter(words: &[String], min_price: Option<f64>, max_price: Option<f64>) -> String {
    let words: Vec<String> = words.iter().map(|w| format!("search={w}")).collect();
    let mut parts = vec![format!("({})", words.join("&"))];
    if let Some(p) = min_price {
        parts.push(format!("(salePrice>={p})"));
    }
    if let Some(p) = max_price {
        parts.push(format!("(salePrice<={p})"));
    }
    parts.push("(onlineAvailability=true)".into());
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

/// A product record as the Products API returns it. `None` without a name, a link, and a price.
fn parse_product(v: &Value) -> Option<Candidate> {
    let name = text(&v["name"], MAX_TEXT_CHARS)?;
    let url = https(&v["url"])?;
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
    let category = v["categoryPath"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| text(&c["name"], 80))
        .rfind(|n| n != STORE);
    let count = v["customerReviewCount"].as_u64().unwrap_or(0);

    Some(Candidate {
        name,
        url,
        store: STORE.to_string(),
        brand: text(&v["manufacturer"], 80),
        model: text(&v["modelNumber"], 80),
        description: text(&v["shortDescription"], MAX_DESCRIPTION_CHARS),
        images,
        price: Some(price),
        regular_price: v["regularPrice"]
            .as_f64()
            .filter(|p| p.is_finite() && *p > price),
        currency: Some("USD".into()),
        in_stock: v["onlineAvailability"].as_bool(),
        in_store: v["inStoreAvailability"].as_bool(),
        free_shipping: v["freeShipping"].as_bool(),
        rating: v["customerReviewAverage"]
            .as_f64()
            .filter(|a| (0.0..=5.0).contains(a) && count > 0),
        review_count: (count > 0).then_some(count),
        review_snippets: Vec::new(),
        features,
        specs,
        category,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_query_becomes_search_words_and_a_budget() {
        let mut max = None;
        let words = keywords(
            "Best wireless noise-cancelling headphones under $300",
            &mut max,
        );
        assert_eq!(words, ["wireless", "noise", "cancelling", "headphones"]);
        assert_eq!(max, Some(300.0));
        assert_eq!(
            filter(&words, None, max),
            "(search=wireless&search=noise&search=cancelling&search=headphones)&(salePrice<=300)&(onlineAvailability=true)"
        );
        // A cap given outright wins over one in the words.
        let mut max = Some(700.0);
        assert_eq!(keywords("laptop under 500", &mut max), ["laptop"]);
        assert_eq!(max, Some(700.0));
        assert_eq!(
            filter(&["tv".to_string()], Some(800.0), None),
            "(search=tv)&(salePrice>=800)&(onlineAvailability=true)"
        );
        assert!(keywords("the best", &mut None).is_empty());
    }

    #[test]
    fn a_product_record_becomes_a_candidate_with_its_pictures_first_primary() {
        let v = json!({
            "sku": 42, "name": "  WH-1000XM5 Wireless Headphones ", "manufacturer": "Sony",
            "modelNumber": "WH1000XM5", "salePrice": 299.99, "regularPrice": 399.99,
            "customerReviewAverage": 4.7, "customerReviewCount": 2345,
            "url": "https://www.bestbuy.com/site/42.p", "image": "https://img.example/42.jpg",
            "images": [
                { "href": "https://img.example/42-side.jpg", "primary": false },
                { "href": "http://img.example/plain.jpg", "primary": true },
                { "href": "https://img.example/42-front.jpg", "primary": true },
                { "href": "https://img.example/42.jpg" },
            ],
            "onlineAvailability": true, "inStoreAvailability": false, "freeShipping": true,
            "features": [{ "feature": "30-hour battery" }, { "feature": "" }, "Multipoint"],
            "details": [{ "name": "Weight", "value": "250 g" }, { "name": "Color" }],
            "categoryPath": [{ "name": "Best Buy" }, { "name": "Audio" }, { "name": "Headphones" }],
        });
        let c = parse_product(&v).unwrap();
        assert_eq!(c.name, "WH-1000XM5 Wireless Headphones");
        assert_eq!(c.store, "Best Buy");
        assert_eq!(
            c.images,
            [
                "https://img.example/42.jpg",
                "https://img.example/42-front.jpg",
                "https://img.example/42-side.jpg"
            ],
            "the main photo, then the primary, then the rest; http is dropped"
        );
        assert_eq!((c.price, c.regular_price), (Some(299.99), Some(399.99)));
        assert_eq!((c.rating, c.review_count), (Some(4.7), Some(2345)));
        assert_eq!(
            (c.in_stock, c.in_store, c.free_shipping),
            (Some(true), Some(false), Some(true))
        );
        assert_eq!(c.features, ["30-hour battery", "Multipoint"]);
        assert_eq!(c.specs, [("Weight".to_string(), "250 g".to_string())]);
        assert_eq!(c.category.as_deref(), Some("Headphones"));
        assert_eq!(
            parse_product(&json!({ "name": "x", "url": "https://x" })),
            None,
            "a product needs a price"
        );
        let unrated = parse_product(&json!({ "name": "x", "url": "https://x", "salePrice": 1,
                                             "customerReviewAverage": 5, "customerReviewCount": 0 }))
        .unwrap();
        assert_eq!((unrated.rating, unrated.review_count), (None, None));
    }

    #[test]
    fn error_bodies_are_summarized() {
        assert_eq!(
            error_message(r#"{"error": {"message": "Invalid query"}}"#),
            "Invalid query"
        );
        assert_eq!(error_message(r#"{"errorMessage": "bad key"}"#), "bad key");
        assert_eq!(error_message("  "), "no details");
    }
}
