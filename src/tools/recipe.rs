use super::{Tool, ToolContext};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use reqwest::StatusCode;
use serde_json::{json, Map, Value};
use std::time::Duration;

const MAX_INGREDIENTS: usize = 60;
const MAX_STEPS: usize = 40;
const MAX_TIPS: usize = 10;
/// Titles, times, quantities, and other short fields.
const MAX_TEXT_CHARS: usize = 200;
/// A step, a tip, or the description.
const MAX_LONG_CHARS: usize = 2000;
/// The photo is shown about this wide; Wikimedia scales its thumbnails to the width asked for.
const IMAGE_WIDTH: u32 = 960;
/// How many search hits to look through for one that has a picture.
const SEARCH_LIMIT: u32 = 5;
/// Wikimedia asks API clients to say who they are.
const USER_AGENT: &str = concat!(
    "cedar/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/hanneshapke/cedar)"
);

/// Shows the user a recipe as a card, drawn by the chat front end from this tool's result: a photo
/// of the dish, the ingredients, and the steps one after another.
///
/// The model writes the recipe; the tool checks it, and always looks up a photo for it, first as
/// the lead image of the best matching Wikipedia article, then among Wikimedia Commons files. It
/// runs in the cedar process, since the sandbox has no network. Its result is a JSON object:
/// `{"title", "description"?, "servings"?, "prep_time"?, "cook_time"?, "total_time"?,
/// "ingredients": [{"quantity"?, "item"}], "steps": [{"title"?, "text", "duration"?}],
/// "tips"?: [text], "image"?: {"url", "page_url", "title", "source"}, "image_note"?}`.
pub struct ShowRecipe {
    http: reqwest::Client,
    wikipedia_url: String,
    commons_url: String,
}

impl ShowRecipe {
    /// Both URLs are MediaWiki `api.php` endpoints (see `DEFAULT_RECIPE_WIKIPEDIA_URL` and
    /// `DEFAULT_RECIPE_COMMONS_URL`).
    pub fn new(wikipedia_url: String, commons_url: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent(USER_AGENT)
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            http,
            wikipedia_url,
            commons_url,
        })
    }

    /// A photo for `query`: a Wikipedia article's lead image, else a Commons file. `None` when
    /// neither search finds a picture.
    async fn find_image(&self, query: &str, timeout: Duration) -> Result<Option<Image>> {
        let params = |namespace: &str, search: String| {
            vec![
                ("action", "query".to_string()),
                ("format", "json".to_string()),
                ("formatversion", "2".to_string()),
                ("generator", "search".to_string()),
                ("gsrsearch", search),
                ("gsrnamespace", namespace.to_string()),
                ("gsrlimit", SEARCH_LIMIT.to_string()),
                ("inprop", "url".to_string()),
            ]
        };
        let mut article = params("0", query.to_string());
        article.extend([
            ("prop", "pageimages|info".to_string()),
            ("piprop", "thumbnail".to_string()),
            ("pithumbsize", IMAGE_WIDTH.to_string()),
        ]);
        let body = self
            .get_json(&self.wikipedia_url, &article, timeout)
            .await?;
        if let Some(image) = article_image(&body) {
            return Ok(Some(image));
        }
        let mut file = params("6", format!("filetype:bitmap {query}"));
        file.extend([
            ("prop", "imageinfo|info".to_string()),
            ("iiprop", "url".to_string()),
            ("iiurlwidth", IMAGE_WIDTH.to_string()),
        ]);
        let body = self.get_json(&self.commons_url, &file, timeout).await?;
        Ok(commons_image(&body))
    }

    async fn get_json(
        &self,
        url: &str,
        params: &[(&str, String)],
        timeout: Duration,
    ) -> Result<Value> {
        let resp = self
            .http
            .get(url)
            .header("Accept", "application/json")
            .query(params)
            .timeout(timeout)
            .send()
            .await
            .context("sending image search request")?;
        let status = resp.status();
        match status {
            s if s.is_success() => {}
            StatusCode::TOO_MANY_REQUESTS => bail!("the image search is rate limited"),
            _ => bail!("image search error {status}"),
        }
        resp.json().await.context("parsing image search response")
    }
}

#[async_trait]
impl Tool for ShowRecipe {
    fn name(&self) -> &str {
        "show_recipe"
    }

    fn description(&self) -> &str {
        "Show the user a recipe as a card in the chat: a photo of the dish, the ingredients as a \
         checklist, and the cooking steps one after another. Use it whenever your answer is a \
         recipe, instead of writing the recipe out as text. A photo is always looked up on \
         Wikipedia and Wikimedia Commons; give image_query (the dish's common name, e.g. \
         \"Spaghetti carbonara\") when the title is not a good search term. The card shows the \
         whole recipe, so afterwards reply with at most a sentence or two, without repeating it."
    }

    fn schema(&self) -> Value {
        let short = |description: &str| json!({ "type": "string", "description": description });
        json!({ "type": "object",
                "properties": {
                    "title": short("The dish, e.g. \"Spaghetti carbonara\"."),
                    "description": short("One or two sentences about the dish."),
                    "servings": short("How many it serves, e.g. \"4\" or \"12 cookies\"."),
                    "prep_time": short("Preparation time, e.g. \"15 min\"."),
                    "cook_time": short("Cooking time, e.g. \"1 h 10 min\"."),
                    "total_time": short("Total time, if it differs from preparation plus cooking."),
                    "ingredients": {
                        "type": "array", "minItems": 1, "maxItems": MAX_INGREDIENTS,
                        "description": "What goes in, in the order it is used.",
                        "items": { "type": "object",
                                   "properties": {
                                       "quantity": short("The amount, e.g. \"200 g\" or \"2 tbsp\"."),
                                       "item": short("The ingredient, e.g. \"spaghetti\" or \"eggs, at room temperature\".")
                                   },
                                   "required": ["item"] }
                    },
                    "steps": {
                        "type": "array", "minItems": 1, "maxItems": MAX_STEPS,
                        "description": "The method, one step per action, in order.",
                        "items": { "type": "object",
                                   "properties": {
                                       "title": short("A few words naming the step, e.g. \"Cook the pasta\"."),
                                       "text": short("What to do."),
                                       "duration": short("How long the step takes, e.g. \"10 min\".")
                                   },
                                   "required": ["text"] }
                    },
                    "tips": { "type": "array", "maxItems": MAX_TIPS, "items": { "type": "string" },
                              "description": "Optional tips, variations, or storage notes." },
                    "image_query": short("What to search for the photo; defaults to the title.")
                },
                "required": ["title", "ingredients", "steps"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let (mut recipe, query) = parse_input(input)?;
        // A missing photo doesn't stop the recipe: the card is drawn without one.
        match self.find_image(&query, ctx.timeout).await {
            Ok(Some(image)) => {
                recipe.insert("image".into(), image.to_json());
            }
            Ok(None) => {
                recipe.insert(
                    "image_note".into(),
                    json!(format!("no photo found for {query:?}")),
                );
            }
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "recipe image lookup failed");
                recipe.insert(
                    "image_note".into(),
                    json!(format!("the photo could not be looked up: {e:#}")),
                );
            }
        }
        Ok(Value::Object(recipe).to_string())
    }
}

#[derive(Debug, PartialEq)]
struct Image {
    url: String,
    page_url: String,
    title: String,
    source: &'static str,
}

impl Image {
    fn to_json(&self) -> Value {
        json!({ "url": self.url, "page_url": self.page_url, "title": self.title, "source": self.source })
    }
}

/// The search hits in a MediaWiki `generator=search` answer, best match first.
fn ranked_pages(body: &Value) -> Vec<&Value> {
    let mut pages: Vec<&Value> = body["query"]["pages"]
        .as_array()
        .map(|p| p.iter().collect())
        .unwrap_or_default();
    pages.sort_by_key(|p| p["index"].as_u64().unwrap_or(u64::MAX));
    pages
}

fn https(url: Option<&str>) -> Option<String> {
    url.filter(|u| u.starts_with("https://")).map(String::from)
}

/// The lead image of the best matching Wikipedia article that has one.
fn article_image(body: &Value) -> Option<Image> {
    ranked_pages(body).into_iter().find_map(|page| {
        Some(Image {
            url: https(page["thumbnail"]["source"].as_str())?,
            page_url: https(page["fullurl"].as_str())?,
            title: page["title"].as_str()?.to_string(),
            source: "Wikipedia",
        })
    })
}

/// The best matching Commons file, as a thumbnail, with its description page for the credit.
fn commons_image(body: &Value) -> Option<Image> {
    ranked_pages(body).into_iter().find_map(|page| {
        let info = &page["imageinfo"][0];
        let title = page["title"].as_str()?;
        Some(Image {
            url: https(info["thumburl"].as_str().or(info["url"].as_str()))?,
            page_url: https(info["descriptionurl"].as_str().or(page["fullurl"].as_str()))?,
            title: title.strip_prefix("File:").unwrap_or(title).to_string(),
            source: "Wikimedia Commons",
        })
    })
}

/// `value` as trimmed text cut to `max` characters, or `None` if it is missing or blank.
fn text(value: &Value, max: usize) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(max).collect())
}

/// The list at `input[key]`, between 1 and `max` long.
fn list<'a>(input: &'a Value, key: &str, max: usize) -> Result<&'a Vec<Value>> {
    let items = input[key]
        .as_array()
        .filter(|l| !l.is_empty())
        .ok_or_else(|| anyhow!("give {key} as a list with at least one entry"))?;
    if items.len() > max {
        bail!("a recipe has at most {max} {key}, got {}", items.len());
    }
    Ok(items)
}

/// The checked recipe as the tool returns it, and what to search for its photo.
fn parse_input(input: &Value) -> Result<(Map<String, Value>, String)> {
    let mut recipe = Map::new();
    let title =
        text(&input["title"], MAX_TEXT_CHARS).ok_or_else(|| anyhow!("give the recipe a title"))?;
    recipe.insert("title".into(), json!(title));
    if let Some(d) = text(&input["description"], MAX_LONG_CHARS) {
        recipe.insert("description".into(), json!(d));
    }
    for key in ["servings", "prep_time", "cook_time", "total_time"] {
        // Models often give servings as a number.
        let value = match &input[key] {
            Value::Number(n) => Some(n.to_string()),
            v => text(v, MAX_TEXT_CHARS),
        };
        if let Some(v) = value {
            recipe.insert(key.into(), json!(v));
        }
    }

    let ingredients = list(input, "ingredients", MAX_INGREDIENTS)?
        .iter()
        .enumerate()
        .map(|(i, v)| ingredient(v).with_context(|| format!("ingredient {}", i + 1)))
        .collect::<Result<Vec<_>>>()?;
    recipe.insert("ingredients".into(), Value::Array(ingredients));

    let steps = list(input, "steps", MAX_STEPS)?
        .iter()
        .enumerate()
        .map(|(i, v)| step(v).with_context(|| format!("step {}", i + 1)))
        .collect::<Result<Vec<_>>>()?;
    recipe.insert("steps".into(), Value::Array(steps));

    let tips: Vec<String> = input["tips"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| text(t, MAX_LONG_CHARS))
        .take(MAX_TIPS)
        .collect();
    if !tips.is_empty() {
        recipe.insert("tips".into(), json!(tips));
    }

    let query = text(&input["image_query"], MAX_TEXT_CHARS).unwrap_or(title);
    Ok((recipe, query))
}

/// `{"quantity"?, "item"}`; a plain string is taken as the whole line.
fn ingredient(value: &Value) -> Result<Value> {
    if value.is_string() {
        let item = text(value, MAX_TEXT_CHARS).ok_or_else(|| anyhow!("is blank"))?;
        return Ok(json!({ "item": item }));
    }
    let item = text(&value["item"], MAX_TEXT_CHARS).ok_or_else(|| anyhow!("give the item"))?;
    let mut out = Map::new();
    if let Some(q) = match &value["quantity"] {
        Value::Number(n) => Some(n.to_string()),
        v => text(v, MAX_TEXT_CHARS),
    } {
        out.insert("quantity".into(), json!(q));
    }
    out.insert("item".into(), json!(item));
    Ok(Value::Object(out))
}

/// `{"title"?, "text", "duration"?}`; a plain string is taken as the text.
fn step(value: &Value) -> Result<Value> {
    if value.is_string() {
        let text = text(value, MAX_LONG_CHARS).ok_or_else(|| anyhow!("is blank"))?;
        return Ok(json!({ "text": text }));
    }
    let body =
        text(&value["text"], MAX_LONG_CHARS).ok_or_else(|| anyhow!("give the step's text"))?;
    let mut out = Map::new();
    if let Some(t) = text(&value["title"], MAX_TEXT_CHARS) {
        out.insert("title".into(), json!(t));
    }
    out.insert("text".into(), json!(body));
    if let Some(d) = text(&value["duration"], MAX_TEXT_CHARS) {
        out.insert("duration".into(), json!(d));
    }
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recipe_is_trimmed_and_normalized() {
        let input = json!({
            "title": "  Carbonara ",
            "description": " Roman pasta. ",
            "servings": 4,
            "prep_time": "10 min",
            "cook_time": "  ",
            "ingredients": [
                { "quantity": "200 g", "item": " spaghetti " },
                { "quantity": 3, "item": "egg yolks" },
                "black pepper",
            ],
            "steps": [
                { "title": "Boil", "text": "Cook the pasta.", "duration": "9 min" },
                "Toss with the eggs off the heat.",
            ],
            "tips": [" Use guanciale. ", ""],
        });
        let (recipe, query) = parse_input(&input).unwrap();
        assert_eq!(query, "Carbonara");
        assert_eq!(
            Value::Object(recipe),
            json!({
                "title": "Carbonara",
                "description": "Roman pasta.",
                "servings": "4",
                "prep_time": "10 min",
                "ingredients": [
                    { "quantity": "200 g", "item": "spaghetti" },
                    { "quantity": "3", "item": "egg yolks" },
                    { "item": "black pepper" },
                ],
                "steps": [
                    { "title": "Boil", "text": "Cook the pasta.", "duration": "9 min" },
                    { "text": "Toss with the eggs off the heat." },
                ],
                "tips": ["Use guanciale."],
            })
        );
    }

    #[test]
    fn the_image_query_overrides_the_title() {
        let input = json!({
            "title": "Nonna's Sunday pasta",
            "image_query": "Spaghetti carbonara",
            "ingredients": ["pasta"], "steps": ["cook"],
        });
        assert_eq!(parse_input(&input).unwrap().1, "Spaghetti carbonara");
    }

    #[test]
    fn bad_input_is_rejected_with_the_entry_number() {
        let err = |input: Value| format!("{:#}", parse_input(&input).unwrap_err());
        assert_eq!(err(json!({})), "give the recipe a title");
        assert!(err(json!({ "title": "x", "steps": ["a"] })).contains("ingredients as a list"));
        assert!(
            err(json!({ "title": "x", "ingredients": ["a"], "steps": [] }))
                .contains("steps as a list")
        );
        assert_eq!(
            err(json!({ "title": "x", "ingredients": ["a", { "quantity": "1" }], "steps": ["a"] })),
            "ingredient 2: give the item"
        );
        assert_eq!(
            err(json!({ "title": "x", "ingredients": ["a"], "steps": [{ "title": "t" }] })),
            "step 1: give the step's text"
        );
        let many: Vec<Value> = (0..41).map(|_| json!("stir")).collect();
        assert!(
            err(json!({ "title": "x", "ingredients": ["a"], "steps": many }))
                .contains("at most 40 steps, got 41")
        );
    }

    #[test]
    fn the_best_ranked_article_with_a_picture_wins() {
        let body = json!({ "query": { "pages": [
            { "title": "Carbonara (film)", "index": 2, "fullurl": "https://en.wikipedia.org/wiki/Carbonara_(film)",
              "thumbnail": { "source": "https://upload.wikimedia.org/film.jpg" } },
            { "title": "Pasta", "index": 1, "fullurl": "https://en.wikipedia.org/wiki/Pasta" },
            { "title": "Carbonara", "index": 3, "fullurl": "https://en.wikipedia.org/wiki/Carbonara",
              "thumbnail": { "source": "https://upload.wikimedia.org/dish.jpg" } },
        ] } });
        assert_eq!(
            article_image(&body),
            Some(Image {
                url: "https://upload.wikimedia.org/film.jpg".into(),
                page_url: "https://en.wikipedia.org/wiki/Carbonara_(film)".into(),
                title: "Carbonara (film)".into(),
                source: "Wikipedia",
            })
        );
        assert_eq!(article_image(&json!({ "batchcomplete": true })), None);
        let plain = json!({ "query": { "pages": [{ "title": "x", "index": 1, "fullurl": "https://w/x",
                                                   "thumbnail": { "source": "http://insecure/x.jpg" } }] } });
        assert_eq!(article_image(&plain), None, "only https images are shown");
    }

    #[test]
    fn commons_files_use_the_thumbnail_and_description_page() {
        let body = json!({ "query": { "pages": [
            { "title": "File:Shakshuka.jpg", "index": 1, "fullurl": "https://commons.wikimedia.org/wiki/File:Shakshuka.jpg",
              "imageinfo": [{ "url": "https://upload.wikimedia.org/full.jpg",
                              "thumburl": "https://upload.wikimedia.org/960px.jpg",
                              "descriptionurl": "https://commons.wikimedia.org/wiki/File:Shakshuka.jpg" }] },
        ] } });
        assert_eq!(
            commons_image(&body),
            Some(Image {
                url: "https://upload.wikimedia.org/960px.jpg".into(),
                page_url: "https://commons.wikimedia.org/wiki/File:Shakshuka.jpg".into(),
                title: "Shakshuka.jpg".into(),
                source: "Wikimedia Commons",
            })
        );
    }
}
