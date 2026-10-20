//! What a web page says about the products on it, for `search_products`: the schema.org
//! `Product` data shops embed as JSON-LD (name, brand, photos, offers with price, currency and
//! stock, the aggregate rating with its review count, a few reviews), else the Open Graph
//! product tags; and the page's text, for review articles that name and weigh products in prose.
//!
//! Everything here comes from a page anyone can write, so it is only read as data: text is
//! clipped, numbers are checked, and links and pictures are kept only as absolute http(s) URLs.

use super::fetch::{html_title, html_to_text};
use super::products::Candidate;
use reqwest::Url;
use serde_json::Value;

/// At most this many products are read from one page (a category or roundup page lists many).
const MAX_PRODUCTS: usize = 10;
const MAX_LD_BLOCKS: usize = 20;
const MAX_IMAGES: usize = 6;
const MAX_REVIEWS: usize = 3;
const MAX_FEATURES: usize = 6;
const MAX_SPECS: usize = 8;
const MAX_TEXT_CHARS: usize = 200;
const MAX_DESCRIPTION_CHARS: usize = 400;
const MAX_REVIEW_CHARS: usize = 280;

/// What one page says.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct PageFacts {
    pub title: Option<String>,
    pub description: Option<String>,
    /// The page's Open Graph picture.
    pub image: Option<String>,
    pub products: Vec<Candidate>,
    /// The page as plain text, at most the number of characters asked for.
    pub excerpt: String,
}

/// Read the page at `page_url` (its address after redirects), whose HTML is `html`.
pub(crate) fn extract(page_url: &str, html: &str, excerpt_chars: usize) -> PageFacts {
    let base = Url::parse(page_url).ok();
    let meta = meta_tags(html);
    let get = |key: &str| {
        meta.iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
    };
    let title = get("og:title")
        .or_else(|| html_title(html))
        .map(|t| clip(&t, MAX_TEXT_CHARS));
    let description = get("og:description")
        .or_else(|| get("description"))
        .map(|d| clip(&d, MAX_DESCRIPTION_CHARS));
    let image = get("og:image")
        .or_else(|| get("og:image:secure_url"))
        .and_then(|u| absolute(base.as_ref(), &u));

    let mut found = Vec::new();
    for block in ld_json_blocks(html) {
        collect_products(&block, &mut found, 0);
    }
    let mut products: Vec<Candidate> = Vec::new();
    for p in &found {
        if let Some(c) = product(p, base.as_ref(), page_url) {
            if !products.iter().any(|o| o.name == c.name && o.url == c.url) {
                products.push(c);
            }
        }
        if products.len() >= MAX_PRODUCTS {
            break;
        }
    }
    // A shop page without JSON-LD often still says it is a product, with a price, in its tags.
    let price = get("product:price:amount").or_else(|| get("og:price:amount"));
    let is_product =
        get("og:type").is_some_and(|t| t.eq_ignore_ascii_case("product")) || price.is_some();
    if products.is_empty() && is_product {
        if let Some(name) = title.clone() {
            products.push(Candidate {
                name,
                url: get("og:url")
                    .and_then(|u| absolute(base.as_ref(), &u))
                    .unwrap_or_else(|| page_url.to_string()),
                store: host(page_url),
                brand: get("product:brand").map(|b| clip(&b, 80)),
                description: description.clone(),
                images: image.iter().cloned().collect(),
                price: price.as_deref().and_then(number).filter(|p| *p >= 0.0),
                currency: get("product:price:currency")
                    .or_else(|| get("og:price:currency"))
                    .and_then(|c| currency(&c)),
                in_stock: get("product:availability")
                    .or_else(|| get("og:availability"))
                    .and_then(|a| stock(&a)),
                ..Candidate::default()
            });
        }
    }

    let excerpt = if excerpt_chars == 0 {
        String::new()
    } else {
        html_to_text(html.as_bytes())
            .map(|t| compact_text(&t, excerpt_chars))
            .unwrap_or_default()
    };
    PageFacts {
        title,
        description,
        image,
        products,
        excerpt,
    }
}

/// The host of `url` without `www.`, or the URL itself if it doesn't parse.
pub(crate) fn host(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| h.trim_start_matches("www.").to_string())
        })
        .unwrap_or_else(|| url.to_string())
}

/// `raw` resolved against the page, kept only as an http(s) URL without spaces.
fn absolute(base: Option<&Url>, raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with("data:") {
        return None;
    }
    let url = match Url::parse(raw) {
        Ok(u) => u,
        Err(_) => base?.join(raw).ok()?,
    };
    matches!(url.scheme(), "http" | "https")
        .then(|| url.to_string())
        .filter(|u| !u.contains(char::is_whitespace))
}

/// Whitespace collapsed, entities decoded, tags dropped, cut to `max` characters.
fn clip(s: &str, max: usize) -> String {
    let decoded = decode_entities(&strip_tags(s));
    let collapsed = decoded.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(max).collect()
}

/// A page's text as lines without the blank ones, cut to `max` characters.
fn compact_text(text: &str, max: usize) -> String {
    let joined = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    joined.chars().take(max).collect()
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest.find(';').filter(|e| *e <= 10);
        let decoded = end.and_then(|e| {
            let name = &rest[1..e];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ => name
                    .strip_prefix("#x")
                    .or_else(|| name.strip_prefix("#X"))
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            };
            c.map(|c| (c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The `<meta>` tags as (`property` or `name`, lowercased; `content`, decoded) pairs.
fn meta_tags(html: &str) -> Vec<(String, String)> {
    // ASCII lowercasing keeps every byte offset, so positions found in `lower` cut `html` too.
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(i) = lower[pos..].find("<meta") {
        let start = pos + i;
        let Some(len) = lower[start..].find('>') else {
            break;
        };
        let tag = &html[start + 5..start + len];
        let attrs = attributes(tag);
        let key = attrs
            .iter()
            .find(|(k, _)| k == "property" || k == "name" || k == "itemprop")
            .map(|(_, v)| v.to_ascii_lowercase());
        let content = attrs.iter().find(|(k, _)| k == "content").map(|(_, v)| v);
        if let (Some(key), Some(content)) = (key, content) {
            out.push((key, decode_entities(content).trim().to_string()));
        }
        pos = start + len;
        if out.len() >= 200 {
            break;
        }
    }
    out
}

/// The attributes of a tag's inside, names lowercased: `a="x" b='y' c=z d`.
fn attributes(tag: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = tag.char_indices().peekable();
    loop {
        while chars
            .next_if(|(_, c)| c.is_whitespace() || *c == '/')
            .is_some()
        {}
        let Some(&(start, _)) = chars.peek() else {
            break;
        };
        let mut end = start;
        while let Some((i, c)) = chars.next_if(|(_, c)| !c.is_whitespace() && *c != '=') {
            end = i + c.len_utf8();
        }
        let name = tag[start..end].to_ascii_lowercase();
        while chars.next_if(|(_, c)| c.is_whitespace()).is_some() {}
        let mut value = String::new();
        if chars.next_if(|(_, c)| *c == '=').is_some() {
            while chars.next_if(|(_, c)| c.is_whitespace()).is_some() {}
            match chars.peek().map(|(_, c)| *c) {
                Some(q @ ('"' | '\'')) => {
                    chars.next();
                    for (_, c) in chars.by_ref() {
                        if c == q {
                            break;
                        }
                        value.push(c);
                    }
                }
                _ => {
                    while let Some((_, c)) = chars.next_if(|(_, c)| !c.is_whitespace()) {
                        value.push(c);
                    }
                }
            }
        }
        if name.is_empty() {
            // A stray character that starts no attribute: skip it so the loop moves on.
            chars.next();
            continue;
        }
        out.push((name, value));
    }
    out
}

/// The parsed contents of every `<script type="application/ld+json">`.
fn ld_json_blocks(html: &str) -> Vec<Value> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(i) = lower[pos..].find("<script") {
        let start = pos + i;
        let Some(open_len) = lower[start..].find('>') else {
            break;
        };
        let body_start = start + open_len + 1;
        let Some(close_len) = lower[body_start..].find("</script") else {
            break;
        };
        let body_end = body_start + close_len;
        if lower[start..body_start].contains("ld+json") {
            let body = html[body_start..body_end]
                .trim()
                .trim_start_matches("<!--")
                .trim_end_matches("-->")
                .trim();
            let parsed = serde_json::from_str::<Value>(body).or_else(|_| {
                // Raw line breaks inside strings are common and not valid JSON.
                serde_json::from_str::<Value>(&body.replace(['\n', '\r', '\t'], " "))
            });
            if let Ok(v) = parsed {
                out.push(v);
            }
        }
        pos = body_end;
        if out.len() >= MAX_LD_BLOCKS {
            break;
        }
    }
    out
}

/// Whether `v`'s `@type` is (or includes) one of `names`, with or without the schema.org prefix.
fn is_type(v: &Value, names: &[&str]) -> bool {
    let matches = |t: &Value| {
        t.as_str().is_some_and(|s| {
            let s = s.rsplit('/').next().unwrap_or(s);
            names.contains(&s)
        })
    };
    match &v["@type"] {
        Value::Array(types) => types.iter().any(matches),
        t => matches(t),
    }
}

/// The `Product` objects in a JSON-LD value: at its top, in a `@graph`, or in a list's items.
fn collect_products(v: &Value, out: &mut Vec<Value>, depth: usize) {
    if depth > 6 || out.len() >= MAX_PRODUCTS * 2 {
        return;
    }
    match v {
        Value::Array(items) => {
            for item in items {
                collect_products(item, out, depth + 1);
            }
        }
        Value::Object(o) => {
            if is_type(
                v,
                &[
                    "Product",
                    "ProductGroup",
                    "IndividualProduct",
                    "ProductModel",
                ],
            ) {
                out.push(v.clone());
                return;
            }
            for key in ["@graph", "itemListElement", "item", "mainEntity"] {
                if let Some(inner) = o.get(key) {
                    collect_products(inner, out, depth + 1);
                }
            }
        }
        _ => {}
    }
}

/// A number given as a number or as text like "1,299.00" or "$129".
fn number(s: &str) -> Option<f64> {
    let digits: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .collect();
    // "1.299,00" is a European thousands separator; "1,299.00" an American one.
    let normalized = match (digits.rfind('.'), digits.rfind(',')) {
        (Some(d), Some(c)) if c > d => digits.replace('.', "").replace(',', "."),
        (None, Some(c)) if digits.len() - c == 3 => digits.replace(',', "."),
        _ => digits.replace(',', ""),
    };
    normalized.parse::<f64>().ok().filter(|n| n.is_finite())
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(number))
        .filter(|n| n.is_finite())
}

/// A three-letter currency code.
fn currency(s: &str) -> Option<String> {
    let c = s.trim().to_ascii_uppercase();
    (c.len() == 3 && c.chars().all(|c| c.is_ascii_alphabetic())).then_some(c)
}

/// Whether a schema.org availability (`https://schema.org/InStock`, "in stock", ...) means it
/// can be bought now. `None` when it doesn't say.
fn stock(s: &str) -> Option<bool> {
    let s = s.to_ascii_lowercase().replace([' ', '_', '-'], "");
    if s.contains("outofstock") || s.contains("soldout") || s.contains("discontinued") {
        Some(false)
    } else if s.contains("instock")
        || s.contains("limitedavailability")
        || s.contains("onlineonly")
        || s.contains("instoreonly")
    {
        Some(true)
    } else {
        None
    }
}

fn str_or_name(v: &Value) -> Option<String> {
    let raw = match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => a.iter().find_map(str_or_name),
        Value::Object(_) => v["name"].as_str().map(String::from),
        _ => None,
    };
    raw.map(|s| clip(&s, 80)).filter(|s| !s.is_empty())
}

/// The picture URLs in an `image` value: a URL, an `ImageObject`, or a list of either.
fn images(v: &Value, base: Option<&Url>, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            if let Some(u) = absolute(base, s) {
                if !out.contains(&u) && out.len() < MAX_IMAGES {
                    out.push(u);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|i| images(i, base, out)),
        Value::Object(o) => {
            for key in ["url", "contentUrl"] {
                if let Some(i) = o.get(key) {
                    images(i, base, out);
                    break;
                }
            }
        }
        _ => {}
    }
}

/// The first offer that names a price: `offers` is an `Offer`, an `AggregateOffer`, or a list.
fn offer(v: &Value) -> Option<(f64, Option<String>, Option<bool>)> {
    match v {
        Value::Array(a) => a.iter().find_map(offer),
        Value::Object(_) => {
            if let Some(inner) = v.get("offers").filter(|o| !o.is_null()) {
                if let Some(found) = offer(inner) {
                    return Some(found);
                }
            }
            let spec = &v["priceSpecification"];
            let price = num(&v["price"])
                .or_else(|| num(&v["lowPrice"]))
                .or_else(|| num(&spec["price"]))
                .or_else(|| num(&spec[0]["price"]))
                .filter(|p| *p >= 0.0)?;
            let cur = v["priceCurrency"]
                .as_str()
                .or_else(|| spec["priceCurrency"].as_str())
                .and_then(currency);
            Some((price, cur, v["availability"].as_str().and_then(stock)))
        }
        _ => None,
    }
}

/// A rating out of 5 and how many reviews it rests on, from an `aggregateRating`.
fn rating(v: &Value) -> (Option<f64>, Option<u64>) {
    let value = num(&v["ratingValue"]);
    let best = num(&v["bestRating"]).filter(|b| *b > 0.0).unwrap_or(5.0);
    let count = num(&v["reviewCount"])
        .or_else(|| num(&v["ratingCount"]))
        .filter(|c| *c >= 0.0 && *c < 1e9)
        .map(|c| c as u64)
        .filter(|c| *c > 0);
    let value = value
        .map(|r| r * 5.0 / best)
        .filter(|r| (0.0..=5.0).contains(r))
        .map(|r| (r * 100.0).round() / 100.0);
    match (value, count) {
        (Some(r), Some(c)) => (Some(r), Some(c)),
        _ => (None, None),
    }
}

fn product(p: &Value, base: Option<&Url>, page_url: &str) -> Option<Candidate> {
    let name = p["name"]
        .as_str()
        .map(|n| clip(n, MAX_TEXT_CHARS))
        .filter(|n| !n.is_empty())?;
    let url = p["url"]
        .as_str()
        .and_then(|u| absolute(base, u))
        .unwrap_or_else(|| page_url.to_string());
    let mut pics = Vec::new();
    images(&p["image"], base, &mut pics);
    let (price, cur, in_stock) = match offer(&p["offers"]) {
        Some((price, cur, stock)) => (Some(price), cur, stock),
        None => (None, None, None),
    };
    let (rating, review_count) = rating(&p["aggregateRating"]);
    let reviews = match &p["review"] {
        Value::Array(a) => a.iter().collect(),
        v @ Value::Object(_) => vec![v],
        _ => vec![],
    };
    let review_snippets = reviews
        .into_iter()
        .filter_map(|r| {
            let body = r["reviewBody"].as_str().or(r["description"].as_str())?;
            let body = clip(body, MAX_REVIEW_CHARS);
            if body.is_empty() {
                return None;
            }
            Some(match num(&r["reviewRating"]["ratingValue"]) {
                Some(stars) => format!("{stars}/5: {body}"),
                None => body,
            })
        })
        .take(MAX_REVIEWS)
        .collect();
    let specs = p["additionalProperty"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let name = clip(s["name"].as_str()?, 80);
            let value = match &s["value"] {
                Value::String(v) => clip(v, MAX_TEXT_CHARS),
                Value::Number(n) => n.to_string(),
                _ => return None,
            };
            (!name.is_empty() && !value.is_empty()).then_some((name, value))
        })
        .take(MAX_SPECS)
        .collect();
    let category = match &p["category"] {
        Value::String(s) => Some(s.rsplit(['>', '/']).next().unwrap_or(s).to_string()),
        Value::Array(a) => a
            .iter()
            .filter_map(|c| c.as_str())
            .next_back()
            .map(String::from),
        _ => None,
    }
    .map(|c| clip(&c, 80))
    .filter(|c| !c.is_empty());
    let features = match &p["features"] {
        Value::Array(a) => a
            .iter()
            .filter_map(|f| f.as_str())
            .map(|f| clip(f, MAX_TEXT_CHARS))
            .filter(|f| !f.is_empty())
            .take(MAX_FEATURES)
            .collect(),
        _ => Vec::new(),
    };
    Some(Candidate {
        store: host(&url),
        name,
        url,
        brand: str_or_name(&p["brand"]).or_else(|| str_or_name(&p["manufacturer"])),
        model: p["model"]
            .as_str()
            .or(p["mpn"].as_str())
            .map(|m| clip(m, 80))
            .filter(|m| !m.is_empty()),
        description: p["description"]
            .as_str()
            .map(|d| clip(d, MAX_DESCRIPTION_CHARS))
            .filter(|d| !d.is_empty()),
        images: pics,
        price,
        regular_price: None,
        currency: cur,
        in_stock,
        in_store: None,
        free_shipping: None,
        rating,
        review_count,
        review_snippets,
        features,
        specs,
        category,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOE_PAGE: &str = r#"<!doctype html><html><head>
<title>Trail Runner 5 | Example Shoes</title>
<meta property="og:title" content="Trail Runner 5 &amp; Co">
<meta property='og:image' content="/img/tr5-og.jpg">
<meta name=description content="A grippy trail shoe.">
<script type="application/ld+json">
{"@context":"https://schema.org","@graph":[
  {"@type":"BreadcrumbList","itemListElement":[]},
  {"@type":["Product"],"name":"Trail Runner 5","brand":{"@type":"Brand","name":"Hoka"},
   "image":["https://cdn.example/tr5-1.jpg",{"@type":"ImageObject","url":"/img/tr5-2.jpg"},"data:image/png;base64,xx"],
   "description":"Cushioned <b>trail</b> shoe.",
   "offers":{"@type":"AggregateOffer","lowPrice":"129.95","priceCurrency":"usd","availability":"https://schema.org/InStock"},
   "aggregateRating":{"@type":"AggregateRating","ratingValue":"9.2","bestRating":"10","reviewCount":"1,204"},
   "review":[{"reviewBody":"Great grip
on wet rock.","reviewRating":{"ratingValue":5}},{"reviewBody":""}],
   "additionalProperty":[{"name":"Drop","value":"5 mm"},{"name":"Weight","value":289}]}
]}
</script>
<script>var x = 1;</script>
</head><body><nav>Home</nav><main><h1>Trail Runner 5</h1><p>Our best trail shoe.</p></main></body></html>"#;

    #[test]
    fn a_shop_page_yields_its_product() {
        let facts = extract("https://shop.example/p/tr5", SHOE_PAGE, 500);
        assert_eq!(facts.title.as_deref(), Some("Trail Runner 5 & Co"));
        assert_eq!(
            facts.image.as_deref(),
            Some("https://shop.example/img/tr5-og.jpg")
        );
        assert_eq!(facts.description.as_deref(), Some("A grippy trail shoe."));
        assert_eq!(facts.products.len(), 1);
        let p = &facts.products[0];
        assert_eq!(p.name, "Trail Runner 5");
        assert_eq!(p.brand.as_deref(), Some("Hoka"));
        assert_eq!(p.url, "https://shop.example/p/tr5");
        assert_eq!(p.store, "shop.example");
        assert_eq!(
            p.images,
            [
                "https://cdn.example/tr5-1.jpg",
                "https://shop.example/img/tr5-2.jpg"
            ]
        );
        assert_eq!(p.description.as_deref(), Some("Cushioned trail shoe."));
        assert_eq!(
            (p.price, p.currency.as_deref(), p.in_stock),
            (Some(129.95), Some("USD"), Some(true))
        );
        assert_eq!(
            (p.rating, p.review_count),
            (Some(4.6), Some(1204)),
            "a rating out of 10 is scaled to 5"
        );
        assert_eq!(p.review_snippets, ["5/5: Great grip on wet rock."]);
        assert_eq!(
            p.specs,
            [
                ("Drop".to_string(), "5 mm".to_string()),
                ("Weight".to_string(), "289".to_string())
            ]
        );
        assert!(
            facts.excerpt.contains("Our best trail shoe."),
            "{}",
            facts.excerpt
        );
    }

    #[test]
    fn open_graph_tags_stand_in_for_missing_json_ld() {
        let html = r#"<html><head><meta property="og:type" content="product">
            <meta property="og:title" content="Linen Shirt">
            <meta property="og:image" content="https://img.example/shirt.jpg">
            <meta property="product:price:amount" content="49,90">
            <meta property="product:price:currency" content="EUR">
            <meta property="product:availability" content="out of stock"></head></html>"#;
        let facts = extract("https://www.boutique.example/shirt?ref=1", html, 0);
        assert_eq!(facts.excerpt, "");
        let p = &facts.products[0];
        assert_eq!(p.name, "Linen Shirt");
        assert_eq!(p.store, "boutique.example");
        assert_eq!(p.images, ["https://img.example/shirt.jpg"]);
        assert_eq!(
            (p.price, p.currency.as_deref(), p.in_stock),
            (Some(49.9), Some("EUR"), Some(false))
        );
        assert_eq!((p.rating, p.review_count), (None, None));
    }

    #[test]
    fn an_article_lists_the_products_it_reviews_and_keeps_its_text() {
        let html = r#"<html><head><title>The 3 best rain jackets</title>
            <script type="application/ld+json">{"@type":"ItemList","itemListElement":[
              {"@type":"ListItem","position":1,"item":{"@type":"Product","name":"Storm Shell","url":"https://brand.example/storm"}},
              {"@type":"ListItem","position":2,"item":{"@type":"Product","name":"Drizzle Lite"}}]}</script>
            <script type="application/ld+json">{ not json </script>
            </head><body><p>We tested 20 jackets.</p></body></html>"#;
        let facts = extract("https://reviews.example/rain", html, 100);
        let names: Vec<&str> = facts.products.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Storm Shell", "Drizzle Lite"]);
        assert_eq!(facts.products[0].url, "https://brand.example/storm");
        assert_eq!(facts.products[1].url, "https://reviews.example/rain");
        assert_eq!(facts.products[0].price, None);
        assert!(facts.excerpt.contains("We tested 20 jackets."));
    }

    #[test]
    fn small_parsers() {
        assert_eq!(number("$1,299.00"), Some(1299.0));
        assert_eq!(number("1.299,00 €"), Some(1299.0));
        assert_eq!(number("49,90"), Some(49.9));
        assert_eq!(number("129"), Some(129.0));
        assert_eq!(number("free"), None);
        assert_eq!(stock("http://schema.org/OutOfStock"), Some(false));
        assert_eq!(stock("InStock"), Some(true));
        assert_eq!(stock("PreOrder"), None);
        assert_eq!(
            decode_entities("a &amp; b &#39;c&#x27; &bogus; &"),
            "a & b 'c' &bogus; &"
        );
        assert_eq!(
            attributes(r#" property="og:title" content='x "y"' data-x=1 async"#),
            [
                ("property".to_string(), "og:title".to_string()),
                ("content".to_string(), "x \"y\"".to_string()),
                ("data-x".to_string(), "1".to_string()),
                ("async".to_string(), String::new())
            ]
        );
        assert_eq!(
            absolute(None, "javascript:alert(1)"),
            None,
            "only http(s) links are kept"
        );
        assert_eq!(host("https://www.nike.com/t/x"), "nike.com");
    }
}
