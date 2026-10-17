use super::{Tool, ToolContext};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Datelike, FixedOffset, NaiveDate};
use futures_util::future::join_all;
use reqwest::StatusCode;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::time::Duration;

const MAX_SYMBOLS: usize = 6;
const MAX_SYMBOL_CHARS: usize = 15;
const DEFAULT_NEWS: u64 = 4;
const MAX_NEWS: u64 = 8;
/// Articles kept in all, after merging the ones several symbols share.
const MAX_NEWS_TOTAL: usize = 24;
/// Points kept per chart line; a longer history is thinned evenly, keeping the latest close.
const MAX_CHART_POINTS: usize = 300;
const DEFAULT_BENCHMARK: &str = "^GSPC";
const RANGES: &[&str] = &["1mo", "3mo", "6mo", "1y", "2y", "5y"];
const DEFAULT_RANGE: &str = "1y";
/// Trading days in a week, month, quarter, half year and year.
const PERIODS: &[(&str, usize)] = &[("1w", 5), ("1m", 21), ("3m", 63), ("6m", 126), ("1y", 252)];
const TRADING_DAYS: f64 = 252.0;
/// Annualized volatility (percent) above which a symbol counts as very volatile, and above which
/// a rating's confidence is lowered.
const HIGH_VOLATILITY: f64 = 55.0;
const SHAKY_VOLATILITY: f64 = 45.0;
const DISCLAIMER: &str = "A rules-based reading of price history, not financial advice. It does \
                          not know earnings, valuation, or your situation; past performance does \
                          not predict future returns.";
/// Yahoo turns away clients that don't look like a browser, so the name says both.
const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (compatible; cedar/",
    env!("CARGO_PKG_VERSION"),
    "; +https://github.com/hanneshapke/cedar)"
);

/// Tracks and compares stocks and indices: the latest quote, the price history as a chart, how
/// each did against a benchmark index, recent news, and a buy/hold/sell rating with the reasoning
/// behind it. The chat front end draws the result as a card.
///
/// Prices come from Yahoo Finance's chart API and news from its search API (neither needs a key).
/// Both run in the cedar process, since the sandbox has no network. The rating is computed here,
/// from the price history only: each factor (trend, momentum, relative strength, RSI, drawdown,
/// volatility) scores -2 to +2 and says why, and the sum makes the call. The result is JSON:
/// `{"range", "benchmark"?, "as_of", "symbols": [{"symbol", "name", "price", "change_pct",
/// "returns", "indicators", "chart": {"dates", "close"}, "rating": {"call", "score",
/// "confidence", "summary", "factors", "watch"}, ...}], "news": [...], "unavailable"?,
/// "disclaimer"}`.
pub struct CompareStocks {
    http: reqwest::Client,
    chart_url: String,
    news_url: String,
}

impl CompareStocks {
    /// `chart_url` is the chart API's root (the symbol goes under it) and `news_url` the search
    /// API (see `DEFAULT_STOCKS_CHART_URL` and `DEFAULT_STOCKS_NEWS_URL`).
    pub fn new(chart_url: String, news_url: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent(USER_AGENT)
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            http,
            chart_url: chart_url.trim_end_matches('/').to_string(),
            news_url,
        })
    }

    async fn history(&self, symbol: &str, range: &str, timeout: Duration) -> Result<History> {
        // Symbols only hold [A-Z0-9.^=-] (see `symbol`), so these two are all that need escaping.
        let path = symbol.replace('^', "%5E").replace('=', "%3D");
        let resp = self
            .http
            .get(format!("{}/{path}", self.chart_url))
            .header("Accept", "application/json")
            .query(&[
                ("range", range),
                ("interval", "1d"),
                ("includeAdjustedClose", "true"),
            ])
            .timeout(timeout)
            .send()
            .await
            .context("sending the price request")?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if let Some(e) = body["chart"]["error"]["description"].as_str() {
            bail!("{e}");
        }
        match status {
            s if s.is_success() => {}
            StatusCode::NOT_FOUND => bail!("unknown symbol"),
            StatusCode::TOO_MANY_REQUESTS => bail!("the price service is rate limited"),
            _ => bail!("price service error {status}"),
        }
        parse_chart(&body)
    }

    async fn news(&self, symbol: &str, count: u64, timeout: Duration) -> Result<Vec<Article>> {
        let resp = self
            .http
            .get(&self.news_url)
            .header("Accept", "application/json")
            .query(&[
                ("q", symbol.to_string()),
                ("quotesCount", "0".to_string()),
                ("newsCount", count.to_string()),
            ])
            .timeout(timeout)
            .send()
            .await
            .context("sending the news request")?;
        let status = resp.status();
        if !status.is_success() {
            bail!("news service error {status}");
        }
        let body: Value = resp.json().await.context("parsing the news")?;
        Ok(parse_news(&body, symbol, count as usize))
    }
}

#[async_trait]
impl Tool for CompareStocks {
    fn name(&self) -> &str {
        "compare_stocks"
    }

    fn description(&self) -> &str {
        "Track and compare stocks, ETFs, and indices, shown to the user as a card in the chat: the \
         latest price and day's move, a chart of the price history against a benchmark index, \
         returns over 1 week to 1 year, recent news headlines, and a buy/hold/sell rating per \
         symbol with every factor behind it (trend, momentum, performance against the benchmark, \
         RSI, drawdown, volatility). Use Yahoo Finance symbols: AAPL, MSFT, BRK-B, SAP.DE, \
         indices like ^GSPC (S&P 500), ^IXIC (Nasdaq), ^DJI (Dow), ^GDAXI (DAX), or BTC-USD. The \
         rating reads price history only; afterwards give your own buy/hold/sell call per symbol \
         in a few sentences, starting from the rating's reasoning and weighing the headlines the \
         tool returned (earnings, guidance, lawsuits, deals), say where you differ from the \
         rating and why, and note that it is not financial advice. Don't repeat the numbers the \
         card already shows."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "symbols": { "type": "array", "minItems": 1, "maxItems": MAX_SYMBOLS,
                                 "items": { "type": "string" },
                                 "description": "The tickers to show and compare, e.g. [\"AAPL\", \"MSFT\"] or [\"^GSPC\", \"^IXIC\"]." },
                    "benchmark": { "type": "string",
                                   "description": "The index to compare against, default ^GSPC (S&P 500). Use one that fits the market, e.g. ^GDAXI for German stocks; \"none\" for no benchmark." },
                    "range": { "type": "string", "enum": RANGES,
                               "description": "The period the chart opens on, default 1y. At least a year is always fetched for the rating." },
                    "news_count": { "type": "integer", "minimum": 0, "maximum": MAX_NEWS,
                                    "description": "Headlines per symbol, default 4; 0 for none." }
                },
                "required": ["symbols"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let req = parse_input(input)?;
        let fetch_range = match req.range.as_str() {
            "2y" | "5y" => req.range.as_str(),
            _ => "1y",
        };
        let mut wanted = req.symbols.clone();
        if let Some(b) = &req.benchmark {
            if !wanted.contains(b) {
                wanted.push(b.clone());
            }
        }
        let histories = join_all(
            wanted
                .iter()
                .map(|s| self.history(s, fetch_range, ctx.timeout)),
        );
        let news = join_all(
            req.symbols
                .iter()
                .filter(|_| req.news_count > 0)
                .map(|s| self.news(s, req.news_count, ctx.timeout)),
        );
        let (histories, news) = tokio::join!(histories, news);

        let mut unavailable = Vec::new();
        let mut by_symbol = HashMap::new();
        for (symbol, history) in wanted.iter().zip(histories) {
            match history {
                Ok(h) => {
                    by_symbol.insert(symbol.clone(), h);
                }
                Err(e) => unavailable.push(json!({ "symbol": symbol, "reason": format!("{e:#}") })),
            }
        }
        if req.symbols.iter().all(|s| !by_symbol.contains_key(s)) {
            let reasons: Vec<String> = unavailable
                .iter()
                .map(|u| {
                    format!(
                        "{}: {}",
                        u["symbol"].as_str().unwrap_or(""),
                        u["reason"].as_str().unwrap_or("")
                    )
                })
                .collect();
            bail!("no prices found ({})", reasons.join("; "));
        }
        let bench = req
            .benchmark
            .as_ref()
            .and_then(|b| by_symbol.get(b).map(|h| (b.as_str(), h)));

        let symbols: Vec<Value> = req
            .symbols
            .iter()
            .filter_map(|s| by_symbol.get(s))
            .map(|h| report(h, bench.filter(|(b, _)| *b != h.symbol)))
            .collect();

        let mut result = Map::new();
        result.insert("range".into(), json!(req.range));
        if let Some((b, h)) = bench {
            result.insert(
                "benchmark".into(),
                json!({ "symbol": b, "name": h.name, "in_symbols": req.symbols.iter().any(|s| s == b) }),
            );
            if !req.symbols.iter().any(|s| s == b) {
                result.insert("benchmark_series".into(), series_json(h));
            }
        }
        let as_of = by_symbol.values().map(|h| (h.time, h.offset)).max();
        if let Some((t, off)) = as_of {
            result.insert("as_of".into(), json!(timestamp(t, off)));
        }
        result.insert("symbols".into(), Value::Array(symbols));

        let mut articles: Vec<Article> = Vec::new();
        let mut news_errors = Vec::new();
        for (symbol, found) in req.symbols.iter().zip(news) {
            match found {
                Ok(list) => merge_news(&mut articles, list),
                Err(e) => news_errors.push(format!("{symbol}: {e:#}")),
            }
        }
        articles.sort_by_key(|a| std::cmp::Reverse(a.published));
        articles.truncate(MAX_NEWS_TOTAL);
        result.insert(
            "news".into(),
            Value::Array(articles.iter().map(Article::to_json).collect()),
        );
        if !news_errors.is_empty() {
            result.insert(
                "news_note".into(),
                json!(format!(
                    "headlines could not be loaded for {}",
                    news_errors.join("; ")
                )),
            );
        }
        if !unavailable.is_empty() {
            result.insert("unavailable".into(), Value::Array(unavailable));
        }
        result.insert("disclaimer".into(), json!(DISCLAIMER));
        Ok(Value::Object(result).to_string())
    }
}

// -- input ------------------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
struct Request {
    symbols: Vec<String>,
    benchmark: Option<String>,
    range: String,
    news_count: u64,
}

/// A ticker as Yahoo spells it: upper case, letters, digits, and `.^=-`.
fn symbol(raw: &str) -> Result<String> {
    let s = raw.trim().to_ascii_uppercase();
    if s.is_empty() {
        bail!("a symbol is blank");
    }
    if s.chars().count() > MAX_SYMBOL_CHARS
        || !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".^=-".contains(c))
    {
        bail!("{raw:?} is not a ticker symbol (use e.g. AAPL, BRK-B, SAP.DE, ^GSPC)");
    }
    Ok(s)
}

fn parse_input(input: &Value) -> Result<Request> {
    // Models sometimes give one string, or a comma-separated list.
    let raw: Vec<String> = match &input["symbols"] {
        Value::Array(items) => items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(String::from)
                    .ok_or_else(|| anyhow!("symbols are strings"))
            })
            .collect::<Result<_>>()?,
        Value::String(s) => s.split(',').map(String::from).collect(),
        _ => bail!("give symbols as a list of tickers, e.g. [\"AAPL\", \"MSFT\"]"),
    };
    let mut symbols = Vec::new();
    for r in raw.iter().filter(|r| !r.trim().is_empty()) {
        let s = symbol(r)?;
        if !symbols.contains(&s) {
            symbols.push(s);
        }
    }
    if symbols.is_empty() {
        bail!("give at least one symbol");
    }
    if symbols.len() > MAX_SYMBOLS {
        bail!(
            "compare at most {MAX_SYMBOLS} symbols at once, got {}",
            symbols.len()
        );
    }
    let benchmark = match input["benchmark"].as_str().map(str::trim) {
        None | Some("") => Some(DEFAULT_BENCHMARK.to_string()),
        Some(b) if b.eq_ignore_ascii_case("none") => None,
        Some(b) => Some(symbol(b).context("benchmark")?),
    };
    let range = match input["range"]
        .as_str()
        .map(|r| r.trim().to_ascii_lowercase())
    {
        None => DEFAULT_RANGE.to_string(),
        Some(r) if r.is_empty() => DEFAULT_RANGE.to_string(),
        Some(r) if RANGES.contains(&r.as_str()) => r,
        Some(r) => bail!("range {r:?} is not one of {}", RANGES.join(", ")),
    };
    let news_count = match &input["news_count"] {
        Value::Null => DEFAULT_NEWS,
        v => v
            .as_u64()
            .ok_or_else(|| anyhow!("news_count is a whole number from 0 to {MAX_NEWS}"))?
            .min(MAX_NEWS),
    };
    Ok(Request {
        symbols,
        benchmark,
        range,
        news_count,
    })
}

// -- prices -----------------------------------------------------------------------------------

/// One symbol's quote and daily closes, oldest first.
#[derive(Debug, Clone)]
struct History {
    symbol: String,
    name: String,
    currency: String,
    exchange: String,
    /// Yahoo's instrument type: EQUITY, ETF, INDEX, CRYPTOCURRENCY, MUTUALFUND, ...
    kind: String,
    price: f64,
    previous_close: Option<f64>,
    day_high: Option<f64>,
    day_low: Option<f64>,
    high_52w: Option<f64>,
    low_52w: Option<f64>,
    /// When `price` was quoted (Unix seconds), and the exchange's UTC offset in seconds.
    time: i64,
    offset: i32,
    dates: Vec<NaiveDate>,
    closes: Vec<f64>,
}

fn parse_chart(body: &Value) -> Result<History> {
    let result = &body["chart"]["result"][0];
    let meta = &result["meta"];
    let symbol = meta["symbol"]
        .as_str()
        .ok_or_else(|| anyhow!("the price service sent no data"))?
        .to_string();
    let offset = meta["gmtoffset"].as_i64().unwrap_or(0) as i32;
    let stamps = result["timestamp"].as_array().cloned().unwrap_or_default();
    let quote = &result["indicators"]["quote"][0];
    // Adjusted closes account for splits and dividends, so returns over time are right.
    let adjusted = &result["indicators"]["adjclose"][0]["adjclose"];
    let closes_in = if adjusted.is_array() {
        adjusted
    } else {
        &quote["close"]
    };
    let mut dates = Vec::new();
    let mut closes = Vec::new();
    for (i, t) in stamps.iter().enumerate() {
        let (Some(t), Some(c)) = (t.as_i64(), closes_in[i].as_f64()) else {
            continue;
        };
        if !c.is_finite() || c <= 0.0 {
            continue;
        }
        let Some(date) = DateTime::from_timestamp(t + offset as i64, 0).map(|d| d.date_naive())
        else {
            continue;
        };
        // Yahoo sometimes repeats today's bar; the later one is fresher.
        if dates.last() == Some(&date) {
            closes.pop();
            dates.pop();
        }
        dates.push(date);
        closes.push(c);
    }
    let price = meta["regularMarketPrice"]
        .as_f64()
        .or(closes.last().copied())
        .ok_or_else(|| anyhow!("no prices for {symbol}"))?;
    if closes.len() < 2 {
        bail!("not enough price history for {symbol}");
    }
    let num = |k: &str| meta[k].as_f64().filter(|v| v.is_finite() && *v > 0.0);
    let text = |k: &str| meta[k].as_str().map(str::trim).filter(|s| !s.is_empty());
    let year = &closes[closes.len().saturating_sub(252)..];
    Ok(History {
        name: text("longName")
            .or(text("shortName"))
            .unwrap_or(&symbol)
            .to_string(),
        currency: text("currency").unwrap_or("").to_string(),
        exchange: text("fullExchangeName")
            .or(text("exchangeName"))
            .unwrap_or("")
            .to_string(),
        kind: text("instrumentType").unwrap_or("EQUITY").to_string(),
        price,
        previous_close: num("previousClose")
            .or(num("regularMarketPreviousClose"))
            .or_else(|| {
                // The second-to-last close, unless the last bar is already today's price.
                closes.len().checked_sub(2).map(|i| closes[i])
            }),
        day_high: num("regularMarketDayHigh"),
        day_low: num("regularMarketDayLow"),
        high_52w: num("fiftyTwoWeekHigh").or_else(|| year.iter().copied().reduce(f64::max)),
        low_52w: num("fiftyTwoWeekLow").or_else(|| year.iter().copied().reduce(f64::min)),
        time: meta["regularMarketTime"].as_i64().unwrap_or(0),
        offset,
        symbol,
        dates,
        closes,
    })
}

fn timestamp(t: i64, offset: i32) -> String {
    let tz = FixedOffset::east_opt(offset).unwrap_or(FixedOffset::east_opt(0).unwrap());
    DateTime::from_timestamp(t, 0)
        .map(|d| d.with_timezone(&tz).to_rfc3339())
        .unwrap_or_default()
}

// -- indicators -------------------------------------------------------------------------------

fn pct(now: f64, then: f64) -> f64 {
    (now / then - 1.0) * 100.0
}

/// The return over the last `days` trading days, in percent.
fn period_return(closes: &[f64], days: usize) -> Option<f64> {
    let n = closes.len();
    (n > days).then(|| pct(closes[n - 1], closes[n - 1 - days]))
}

/// The return since the last close of the previous calendar year.
fn ytd_return(dates: &[NaiveDate], closes: &[f64]) -> Option<f64> {
    let year = dates.last()?.year();
    let i = dates.iter().rposition(|d| d.year() < year)?;
    Some(pct(*closes.last()?, closes[i]))
}

fn sma(closes: &[f64], n: usize) -> Option<f64> {
    (closes.len() >= n).then(|| closes[closes.len() - n..].iter().sum::<f64>() / n as f64)
}

/// Wilder's 14-day relative strength index.
fn rsi(closes: &[f64], n: usize) -> Option<f64> {
    if closes.len() <= n {
        return None;
    }
    let changes: Vec<f64> = closes.windows(2).map(|w| w[1] - w[0]).collect();
    let mut gain = changes[..n].iter().filter(|c| **c > 0.0).sum::<f64>() / n as f64;
    let mut loss = -changes[..n].iter().filter(|c| **c < 0.0).sum::<f64>() / n as f64;
    for c in &changes[n..] {
        gain = (gain * (n - 1) as f64 + c.max(0.0)) / n as f64;
        loss = (loss * (n - 1) as f64 + (-c).max(0.0)) / n as f64;
    }
    Some(if loss == 0.0 {
        100.0
    } else {
        100.0 - 100.0 / (1.0 + gain / loss)
    })
}

fn log_returns(closes: &[f64]) -> Vec<f64> {
    closes.windows(2).map(|w| (w[1] / w[0]).ln()).collect()
}

/// Annualized volatility of daily returns over the last year, in percent.
fn volatility(closes: &[f64]) -> Option<f64> {
    let r = log_returns(&closes[closes.len().saturating_sub(253)..]);
    if r.len() < 20 {
        return None;
    }
    let mean = r.iter().sum::<f64>() / r.len() as f64;
    let var = r.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (r.len() - 1) as f64;
    Some(var.sqrt() * TRADING_DAYS.sqrt() * 100.0)
}

/// The deepest fall from a high to a later low over the last year, in percent (negative).
fn max_drawdown(closes: &[f64]) -> f64 {
    let mut peak = f64::MIN;
    let mut worst: f64 = 0.0;
    for &c in &closes[closes.len().saturating_sub(252)..] {
        peak = peak.max(c);
        worst = worst.min(pct(c, peak));
    }
    worst
}

/// How strongly the symbol moves with the benchmark (1 = in step) over their shared last year.
fn beta(a: &History, b: &History) -> Option<f64> {
    let b_close: HashMap<NaiveDate, f64> = b
        .dates
        .iter()
        .copied()
        .zip(b.closes.iter().copied())
        .collect();
    let pairs: Vec<(f64, f64)> = a
        .dates
        .iter()
        .zip(&a.closes)
        .filter_map(|(d, c)| b_close.get(d).map(|bc| (*c, *bc)))
        .collect();
    let pairs = &pairs[pairs.len().saturating_sub(253)..];
    if pairs.len() < 60 {
        return None;
    }
    let ra: Vec<f64> = pairs.windows(2).map(|w| (w[1].0 / w[0].0).ln()).collect();
    let rb: Vec<f64> = pairs.windows(2).map(|w| (w[1].1 / w[0].1).ln()).collect();
    let n = ra.len() as f64;
    let (ma, mb) = (ra.iter().sum::<f64>() / n, rb.iter().sum::<f64>() / n);
    let cov: f64 = ra.iter().zip(&rb).map(|(x, y)| (x - ma) * (y - mb)).sum();
    let var: f64 = rb.iter().map(|y| (y - mb).powi(2)).sum();
    (var > 0.0).then(|| cov / var)
}

#[derive(Debug, Default, Clone)]
struct Indicators {
    returns: Vec<(&'static str, Option<f64>)>,
    sma50: Option<f64>,
    sma200: Option<f64>,
    rsi14: Option<f64>,
    volatility: Option<f64>,
    max_drawdown: f64,
    from_high: Option<f64>,
    beta: Option<f64>,
    /// This symbol's 6-month return minus the benchmark's, in percentage points.
    relative_6m: Option<f64>,
    days: usize,
}

fn indicators(h: &History, bench: Option<&History>) -> Indicators {
    let c = &h.closes;
    let mut returns: Vec<(&str, Option<f64>)> = PERIODS
        .iter()
        .map(|(k, d)| (*k, period_return(c, *d)))
        .collect();
    returns.insert(4, ("ytd", ytd_return(&h.dates, c)));
    let relative_6m =
        bench.and_then(|b| Some(period_return(c, 126)? - period_return(&b.closes, 126)?));
    Indicators {
        returns,
        sma50: sma(c, 50),
        sma200: sma(c, 200),
        rsi14: rsi(c, 14),
        volatility: volatility(c),
        max_drawdown: max_drawdown(c),
        from_high: h.high_52w.map(|hi| pct(h.price, hi)),
        beta: bench.and_then(|b| beta(h, b)),
        relative_6m,
        days: c.len(),
    }
}

// -- the rating -------------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
struct Factor {
    name: String,
    /// -2 (strongly against) to +2 (strongly for).
    score: i32,
    /// A few words, used in the summary.
    headline: String,
    detail: String,
}

impl Factor {
    fn new(name: &str, score: i32, headline: &str, detail: String) -> Self {
        Self {
            name: name.into(),
            score,
            headline: headline.into(),
            detail,
        }
    }
}

fn money(x: f64, currency: &str) -> String {
    let digits = if x.abs() >= 1.0 { 2 } else { 4 };
    match currency {
        "USD" => format!("${x:.digits$}"),
        "EUR" => format!("€{x:.digits$}"),
        "GBP" => format!("£{x:.digits$}"),
        "JPY" => format!("¥{x:.0}"),
        "" => format!("{x:.digits$}"),
        c => format!("{x:.digits$} {c}"),
    }
}

fn signed(x: f64) -> String {
    format!("{x:+.1}%")
}

fn factors(h: &History, ind: &Indicators, bench_name: Option<&str>) -> Vec<Factor> {
    let mut out = Vec::new();
    let price = h.price;
    let m = |x: f64| money(x, &h.currency);

    // Trend: where the price sits against its long average, and the averages against each other.
    if let (Some(s50), Some(s200)) = (ind.sma50, ind.sma200) {
        let gap = pct(price, s200);
        let mut score = 0;
        if gap > 2.0 {
            score += 1;
        } else if gap < -2.0 {
            score -= 1;
        }
        score += if s50 > s200 { 1 } else { -1 };
        let place = if gap >= 0.0 { "above" } else { "below" };
        let cross = if s50 > s200 {
            "the 50-day average is above the 200-day (a \"golden cross\" setup)"
        } else {
            "the 50-day average is below the 200-day (a \"death cross\" setup)"
        };
        let (headline, verdict) = match score {
            2 => ("uptrend intact", "the long-term uptrend is intact"),
            -2 => (
                "downtrend",
                "the long-term trend points down, and buyers have not taken over yet",
            ),
            1 => (
                "trend turning up",
                "the trend is improving but not confirmed",
            ),
            -1 => ("trend weakening", "the trend is losing strength"),
            _ => ("no clear trend", "the trend is undecided"),
        };
        out.push(Factor::new(
            "Long-term trend",
            score,
            headline,
            format!(
                "The price ({}) is {:.1}% {place} its 200-day average ({}), and {cross}: {verdict}.",
                m(price),
                gap.abs(),
                m(s200)
            ),
        ));
    } else if let Some(s50) = ind.sma50 {
        let gap = pct(price, s50);
        let score = if gap > 2.0 {
            1
        } else if gap < -2.0 {
            -1
        } else {
            0
        };
        out.push(Factor::new(
            "Trend",
            score,
            if score > 0 { "above its 50-day average" } else if score < 0 { "below its 50-day average" } else { "flat" },
            format!(
                "Only {} days of history, too few for a 200-day average. The price is {:.1}% {} its 50-day average ({}).",
                ind.days,
                gap.abs(),
                if gap >= 0.0 { "above" } else { "below" },
                m(s50)
            ),
        ));
    }

    // Momentum: what the last three and six months did.
    let ret = |k: &str| {
        ind.returns
            .iter()
            .find(|(p, _)| *p == k)
            .and_then(|(_, v)| *v)
    };
    if let (Some(r3), Some(r6)) = (ret("3m"), ret("6m")) {
        let mut score = 0;
        if r6 > 10.0 {
            score += 1;
        } else if r6 < -10.0 {
            score -= 1;
        }
        if r3 > 5.0 {
            score += 1;
        } else if r3 < -5.0 {
            score -= 1;
        }
        let (headline, verdict) = match score {
            2 => (
                "strong momentum",
                "buyers have been in control and the move is still running",
            ),
            1 => ("positive momentum", "the recent direction is up"),
            -1 => ("fading momentum", "the recent direction is down"),
            -2 => (
                "negative momentum",
                "sellers have been in control for months",
            ),
            _ => (
                "mixed momentum",
                "neither buyers nor sellers have the upper hand",
            ),
        };
        out.push(Factor::new(
            "Momentum",
            score,
            headline,
            format!(
                "{} over 6 months and {} over 3 months: {verdict}.",
                signed(r6),
                signed(r3)
            ),
        ));
    }

    // Relative strength: did it beat the market it trades in?
    if let (Some(rel), Some(bench)) = (ind.relative_6m, bench_name) {
        let score = match rel {
            r if r >= 15.0 => 2,
            r if r >= 5.0 => 1,
            r if r <= -15.0 => -2,
            r if r <= -5.0 => -1,
            _ => 0,
        };
        let beta = ind
            .beta
            .map(|b| format!(" It moves about {b:.1}× as much as the index day to day."))
            .unwrap_or_default();
        let verdict = match score {
            s if s > 0 => "money has been flowing into it rather than the market as a whole",
            s if s < 0 => "holding the index would have done better; ask why before buying",
            _ => "it has moved roughly with the market",
        };
        out.push(Factor::new(
            &format!("vs. {bench}"),
            score,
            if score > 0 {
                "beating the market"
            } else if score < 0 {
                "lagging the market"
            } else {
                "in line with the market"
            },
            format!(
                "{:+.1} percentage points against the {bench} over 6 months: {verdict}.{beta}",
                rel
            ),
        ));
    }

    // RSI: how stretched the last few weeks are.
    if let Some(r) = ind.rsi14 {
        let (score, headline, verdict) = if r >= 70.0 {
            (-1, "overbought", "after a run this steep a pause or pullback is more likely, so a better entry may come")
        } else if r <= 30.0 {
            (1, "oversold", "selling looks stretched and rebounds from here are common, though a falling trend can stay oversold")
        } else {
            (0, "RSI neutral", "neither stretched up nor down")
        };
        out.push(Factor::new(
            "RSI (14-day)",
            score,
            headline,
            format!("The relative strength index is {r:.0} (above 70 is overbought, below 30 oversold): {verdict}."),
        ));
    }

    // Drawdown: how far it is from its high.
    if let Some(off) = ind.from_high {
        let (score, headline, verdict) = if off <= -25.0 {
            (-1, "deep below its high", "a fall this large usually reflects real problems; wait for the trend to turn before catching it")
        } else if off >= -5.0 {
            (
                0,
                "near its 52-week high",
                "strength, though new buyers pay up for it",
            )
        } else {
            (
                0,
                "moderate pullback",
                "a normal pullback, not a broken chart",
            )
        };
        let high = m(h.high_52w.unwrap_or(price));
        let place = if off > -0.5 {
            format!("At its 52-week high of {high}")
        } else {
            format!("{:.1}% below its 52-week high of {high}", off.abs())
        };
        out.push(Factor::new(
            "Distance from high",
            score,
            headline,
            format!(
                "{place} (worst fall in the past year: {:.1}%): {verdict}.",
                ind.max_drawdown.abs()
            ),
        ));
    }

    // Volatility: how hard it is to hold.
    if let Some(v) = ind.volatility {
        let (score, headline, verdict) = if v > HIGH_VOLATILITY {
            (
                -1,
                "very volatile",
                "big swings either way are normal, so keep any position small",
            )
        } else if v < 18.0 {
            (0, "steady", "calm by stock standards")
        } else {
            (0, "typical volatility", "ordinary swings for a stock")
        };
        out.push(Factor::new(
            "Volatility",
            score,
            headline,
            format!("Its annualized volatility is {v:.0}%: {verdict}."),
        ));
    }
    out
}

/// What would turn the view: levels and readings to watch.
fn watch(h: &History, ind: &Indicators) -> Vec<String> {
    let m = |x: f64| money(x, &h.currency);
    let mut out = Vec::new();
    if let Some(s200) = ind.sma200 {
        let gap = pct(s200, h.price);
        if h.price >= s200 {
            out.push(format!(
                "A close below the 200-day average (about {}, {:.1}% lower) would turn the trend negative.",
                m(s200),
                gap.abs()
            ));
        } else {
            out.push(format!(
                "A close back above the 200-day average (about {}, {:.1}% higher) would turn the trend positive.",
                m(s200),
                gap
            ));
        }
    }
    if let (Some(off), Some(hi)) = (ind.from_high, h.high_52w) {
        if off > -5.0 {
            out.push(format!(
                "A move above the 52-week high ({}) would confirm the uptrend.",
                m(hi)
            ));
        }
    }
    match ind.rsi14 {
        Some(r) if r >= 70.0 => {
            out.push("RSI cooling below 70 would lift the overbought caution.".into())
        }
        Some(r) if r <= 30.0 => {
            out.push("RSI recovering above 30 would show the selling is easing.".into())
        }
        _ => {}
    }
    if let Some(rel) = ind.relative_6m {
        if rel <= -5.0 {
            out.push("Starting to outperform the benchmark would show buyers coming back.".into());
        }
    }
    out
}

#[derive(Debug, PartialEq)]
struct Rating {
    call: &'static str,
    lean: Option<&'static str>,
    score: i32,
    max_score: i32,
    confidence: &'static str,
    summary: String,
}

fn rate(h: &History, ind: &Indicators, factors: &[Factor]) -> Rating {
    let score: i32 = factors.iter().map(|f| f.score).sum();
    // The most each factor could add, so the score reads against its scale.
    let max_score: i32 = factors
        .iter()
        .map(|f| match f.name.as_str() {
            "Long-term trend" | "Momentum" => 2,
            n if n.starts_with("vs. ") => 2,
            _ => 1,
        })
        .sum();
    let call = match score {
        s if s >= 3 => "buy",
        s if s <= -3 => "sell",
        _ => "hold",
    };
    let lean = match (call, score) {
        ("hold", 2) => Some("leaning buy"),
        ("hold", -2) => Some("leaning sell"),
        _ => None,
    };
    let pos: Vec<&Factor> = factors.iter().filter(|f| f.score > 0).collect();
    let neg: Vec<&Factor> = factors.iter().filter(|f| f.score < 0).collect();
    let opposing = match call {
        "buy" => neg.len(),
        "sell" => pos.len(),
        _ => 0,
    };
    let mut level = match call {
        "hold" if score.abs() <= 1 => 1,
        "hold" => 0,
        _ if score.abs() >= 5 && opposing == 0 => 2,
        _ if opposing <= 1 => 1,
        _ => 0,
    };
    if ind.volatility.is_some_and(|v| v > SHAKY_VOLATILITY) || ind.days < 200 {
        level = (level - 1).max(0);
    }
    let confidence = ["low", "medium", "high"][level as usize];

    let heads = |fs: &[&Factor]| -> String {
        let mut fs = fs.to_vec();
        fs.sort_by_key(|f| -f.score.abs());
        fs.iter()
            .map(|f| f.headline.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let n = factors.len();
    let mut summary = match call {
        "buy" => format!(
            "Buy: {} of {n} signals are positive ({})",
            pos.len(),
            heads(&pos)
        ),
        "sell" => format!(
            "Sell: {} of {n} signals are negative ({})",
            neg.len(),
            heads(&neg)
        ),
        _ if pos.is_empty() && neg.is_empty() => {
            "Hold: no signal stands out either way".to_string()
        }
        _ => format!(
            "Hold: the signals cancel out — for it: {}; against it: {}",
            if pos.is_empty() {
                "nothing".into()
            } else {
                heads(&pos)
            },
            if neg.is_empty() {
                "nothing".into()
            } else {
                heads(&neg)
            }
        ),
    };
    match call {
        "buy" if !neg.is_empty() => summary.push_str(&format!(". The caution: {}", heads(&neg))),
        "sell" if !pos.is_empty() => summary.push_str(&format!(". In its favor: {}", heads(&pos))),
        _ => {}
    }
    summary.push('.');
    if h.kind == "INDEX" {
        summary.push_str(
            " An index can't be bought directly; the call applies to a fund that tracks it.",
        );
    }
    Rating {
        call,
        lean,
        score,
        max_score,
        confidence,
        summary,
    }
}

// -- the result -------------------------------------------------------------------------------

/// Rounded for the result: cents above 1, four decimals below.
fn round_price(x: f64) -> f64 {
    let f = if x.abs() >= 1.0 { 100.0 } else { 10_000.0 };
    (x * f).round() / f
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

fn series_json(h: &History) -> Value {
    let n = h.closes.len();
    let step = n.div_ceil(MAX_CHART_POINTS).max(1);
    // Thin from the end, so the latest close is always kept.
    let mut idx: Vec<usize> = (0..n).rev().step_by(step).collect();
    idx.reverse();
    json!({
        "symbol": h.symbol,
        "name": h.name,
        "dates": idx.iter().map(|&i| h.dates[i].to_string()).collect::<Vec<_>>(),
        "close": idx.iter().map(|&i| round_price(h.closes[i])).collect::<Vec<_>>(),
    })
}

fn report(h: &History, bench: Option<(&str, &History)>) -> Value {
    let ind = indicators(h, bench.map(|(_, b)| b));
    let bench_name = bench.map(|(s, b)| {
        if b.name.is_empty() {
            s
        } else {
            b.name.as_str()
        }
    });
    let factors = factors(h, &ind, bench_name);
    let rating = rate(h, &ind, &factors);
    let opt = |v: Option<f64>| v.map(round_price);
    let mut out = json!({
        "symbol": h.symbol,
        "name": h.name,
        "type": h.kind,
        "currency": h.currency,
        "exchange": h.exchange,
        "price": round_price(h.price),
        "as_of": timestamp(h.time, h.offset),
        "day_high": opt(h.day_high),
        "day_low": opt(h.day_low),
        "fifty_two_week_high": opt(h.high_52w),
        "fifty_two_week_low": opt(h.low_52w),
        "returns": ind.returns.iter().map(|(k, v)| (k.to_string(), json!(v.map(round1)))).collect::<Map<_, _>>(),
        "indicators": {
            "sma50": opt(ind.sma50),
            "sma200": opt(ind.sma200),
            "rsi14": ind.rsi14.map(round1),
            "volatility_pct": ind.volatility.map(round1),
            "max_drawdown_pct": round1(ind.max_drawdown),
            "from_52w_high_pct": ind.from_high.map(round1),
            "beta": ind.beta.map(|b| (b * 100.0).round() / 100.0),
            "vs_benchmark_6m_pts": ind.relative_6m.map(round1),
        },
        "rating": {
            "call": rating.call,
            "lean": rating.lean,
            "score": rating.score,
            "max_score": rating.max_score,
            "confidence": rating.confidence,
            "summary": rating.summary,
            "factors": factors.iter().map(|f| json!({
                "name": f.name, "score": f.score, "headline": f.headline, "detail": f.detail,
            })).collect::<Vec<_>>(),
            "watch": watch(h, &ind),
        },
        "chart": series_json(h),
    });
    if let Some(prev) = h.previous_close {
        out["previous_close"] = json!(round_price(prev));
        out["change"] = json!(round_price(h.price - prev));
        out["change_pct"] = json!((pct(h.price, prev) * 100.0).round() / 100.0);
    }
    out
}

// -- news -------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Clone)]
struct Article {
    title: String,
    publisher: String,
    url: String,
    published: i64,
    thumbnail: Option<String>,
    symbols: Vec<String>,
}

impl Article {
    fn to_json(&self) -> Value {
        let mut v = json!({
            "title": self.title, "publisher": self.publisher, "url": self.url,
            "published": timestamp(self.published, 0), "symbols": self.symbols,
        });
        if let Some(t) = &self.thumbnail {
            v["thumbnail"] = json!(t);
        }
        v
    }
}

fn https(url: Option<&str>) -> Option<String> {
    url.filter(|u| u.starts_with("https://") && !u.contains(char::is_whitespace))
        .map(String::from)
}

fn parse_news(body: &Value, symbol: &str, max: usize) -> Vec<Article> {
    body["news"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let title: String = n["title"].as_str()?.trim().chars().take(300).collect();
            if title.is_empty() {
                return None;
            }
            // The smallest thumbnail that is still sharp at the card's size.
            let sizes = n["thumbnail"]["resolutions"].as_array();
            let thumbnail = sizes.and_then(|r| {
                let mut r: Vec<&Value> = r.iter().collect();
                r.sort_by_key(|t| t["width"].as_u64().unwrap_or(0));
                r.iter()
                    .find(|t| t["width"].as_u64().unwrap_or(0) >= 140)
                    .or(r.last())
                    .and_then(|t| https(t["url"].as_str()))
            });
            Some(Article {
                title,
                publisher: n["publisher"]
                    .as_str()
                    .unwrap_or("")
                    .trim()
                    .chars()
                    .take(80)
                    .collect(),
                url: https(n["link"].as_str())?,
                published: n["providerPublishTime"].as_i64().unwrap_or(0),
                thumbnail,
                symbols: vec![symbol.to_string()],
            })
        })
        .take(max)
        .collect()
}

/// Adds `found` to `all`; an article another symbol already brought in gains this symbol instead.
fn merge_news(all: &mut Vec<Article>, found: Vec<Article>) {
    for a in found {
        match all
            .iter_mut()
            .find(|b| b.url == a.url || b.title == a.title)
        {
            Some(b) => {
                for s in a.symbols {
                    if !b.symbols.contains(&s) {
                        b.symbols.push(s);
                    }
                }
            }
            None => all.push(a),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A history of `closes` on consecutive days ending 2026-10-16.
    fn history(closes: Vec<f64>) -> History {
        let end = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
        let n = closes.len();
        History {
            symbol: "TEST".into(),
            name: "Test Inc.".into(),
            currency: "USD".into(),
            exchange: "NasdaqGS".into(),
            kind: "EQUITY".into(),
            price: *closes.last().unwrap(),
            previous_close: Some(closes[n - 2]),
            day_high: None,
            day_low: None,
            high_52w: closes[n.saturating_sub(252)..]
                .iter()
                .copied()
                .reduce(f64::max),
            low_52w: closes[n.saturating_sub(252)..]
                .iter()
                .copied()
                .reduce(f64::min),
            time: 1_792_180_800,
            offset: -14_400,
            dates: (0..n)
                .map(|i| end - chrono::Days::new((n - 1 - i) as u64))
                .collect(),
            closes,
        }
    }

    /// Steady growth of `daily` percent with a small zig-zag, so RSI and volatility are defined.
    fn trending(n: usize, daily: f64) -> Vec<f64> {
        (0..n)
            .map(|i| {
                100.0 * (1.0 + daily / 100.0).powi(i as i32) * if i % 2 == 0 { 1.0 } else { 1.004 }
            })
            .collect()
    }

    #[test]
    fn input_is_normalized() {
        let req = parse_input(&json!({ "symbols": [" aapl ", "MSFT", "AAPL", "^gspc"] })).unwrap();
        assert_eq!(
            req,
            Request {
                symbols: vec!["AAPL".into(), "MSFT".into(), "^GSPC".into()],
                benchmark: Some("^GSPC".into()),
                range: "1y".into(),
                news_count: 4,
            }
        );
        let req = parse_input(&json!({ "symbols": "sap.de, brk-b", "benchmark": "^gdaxi",
                                       "range": "6MO", "news_count": 20 }))
        .unwrap();
        assert_eq!(req.symbols, ["SAP.DE", "BRK-B"]);
        assert_eq!(req.benchmark.as_deref(), Some("^GDAXI"));
        assert_eq!(req.range, "6mo");
        assert_eq!(req.news_count, MAX_NEWS);
        let req = parse_input(&json!({ "symbols": ["BTC-USD"], "benchmark": "none" })).unwrap();
        assert_eq!(req.benchmark, None);
    }

    #[test]
    fn bad_input_is_rejected() {
        let err = |v: Value| format!("{:#}", parse_input(&v).unwrap_err());
        assert!(err(json!({})).contains("give symbols"));
        assert_eq!(err(json!({ "symbols": [] })), "give at least one symbol");
        assert!(err(json!({ "symbols": ["AAPL; rm -rf"] })).contains("is not a ticker symbol"));
        assert!(
            err(json!({ "symbols": ["A", "B", "C", "D", "E", "F", "G"] }))
                .contains("at most 6 symbols")
        );
        assert!(err(json!({ "symbols": ["A"], "range": "10y" })).contains("not one of"));
        assert!(err(json!({ "symbols": ["A"], "benchmark": "S&P" })).starts_with("benchmark:"));
    }

    #[test]
    fn indicators_match_hand_computed_values() {
        let closes = [10.0, 11.0, 10.5, 11.5, 12.0];
        assert_eq!(sma(&closes, 2), Some(11.75));
        assert_eq!(sma(&closes, 6), None);
        assert!((period_return(&closes, 4).unwrap() - 20.0).abs() < 1e-9);
        assert_eq!(period_return(&closes, 5), None);
        assert!((max_drawdown(&[10.0, 12.0, 9.0, 11.0, 13.0]) + 25.0).abs() < 1e-9);
        assert_eq!(rsi(&[1.0, 2.0, 3.0, 4.0], 3), Some(100.0));
        // Equal gains and losses balance at 50.
        let flat: Vec<f64> = (0..30)
            .map(|i| if i % 2 == 0 { 10.0 } else { 11.0 })
            .collect();
        assert!((rsi(&flat, 14).unwrap() - 50.0).abs() < 5.0);
    }

    #[test]
    fn ytd_counts_from_the_last_close_of_last_year() {
        let dates = ["2025-12-30", "2025-12-31", "2026-01-02", "2026-06-01"]
            .map(|d| d.parse::<NaiveDate>().unwrap());
        assert!((ytd_return(&dates, &[90.0, 100.0, 101.0, 110.0]).unwrap() - 10.0).abs() < 1e-9);
        assert_eq!(ytd_return(&dates[2..], &[101.0, 110.0]), None);
    }

    #[test]
    fn a_steady_climb_beating_the_market_is_a_buy() {
        let h = history(trending(300, 0.15));
        let mut bench = history(trending(300, 0.02));
        bench.symbol = "^GSPC".into();
        let ind = indicators(&h, Some(&bench));
        let fs = factors(&h, &ind, Some("S&P 500"));
        let names: Vec<&str> = fs.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Long-term trend",
                "Momentum",
                "vs. S&P 500",
                "RSI (14-day)",
                "Distance from high",
                "Volatility"
            ]
        );
        assert_eq!(fs[0].score, 2, "{}", fs[0].detail);
        assert!(fs[0].detail.contains("golden cross"), "{}", fs[0].detail);
        assert_eq!(fs[1].score, 2);
        assert_eq!(fs[2].score, 2, "{}", fs[2].detail);
        let rating = rate(&h, &ind, &fs);
        assert_eq!(rating.call, "buy");
        assert!(rating.score >= 5, "{rating:?}");
        assert_eq!(rating.max_score, 9);
        assert!(rating.summary.starts_with("Buy: "), "{}", rating.summary);
        assert!(
            rating.summary.contains("uptrend intact"),
            "{}",
            rating.summary
        );
        let w = watch(&h, &ind);
        assert!(
            w[0].starts_with("A close below the 200-day average"),
            "{w:?}"
        );
    }

    #[test]
    fn a_long_slide_is_a_sell_and_an_index_says_how_to_hold_it() {
        let mut h = history(trending(300, -0.2));
        h.kind = "INDEX".into();
        let ind = indicators(&h, None);
        let fs = factors(&h, &ind, None);
        assert!(
            fs.iter().all(|f| !f.name.starts_with("vs. ")),
            "no benchmark, no comparison"
        );
        assert_eq!(fs[0].score, -2);
        assert!(fs[0].detail.contains("death cross"));
        let rating = rate(&h, &ind, &fs);
        assert_eq!(rating.call, "sell");
        assert!(
            rating.summary.contains("a fund that tracks it"),
            "{}",
            rating.summary
        );
        assert!(watch(&h, &ind)[0].starts_with("A close back above"));
    }

    #[test]
    fn mixed_signals_are_a_hold_with_low_confidence_on_short_history() {
        let h = history(trending(80, 0.0));
        let ind = indicators(&h, None);
        let fs = factors(&h, &ind, None);
        assert_eq!(fs[0].name, "Trend", "too short for a 200-day average");
        let rating = rate(&h, &ind, &fs);
        assert_eq!(rating.call, "hold");
        assert_eq!(rating.confidence, "low");
    }

    #[test]
    fn a_chart_answer_is_read_with_adjusted_closes() {
        let body = json!({ "chart": { "error": null, "result": [{
            "meta": { "symbol": "AAPL", "longName": "Apple Inc.", "currency": "USD",
                      "fullExchangeName": "NasdaqGS", "instrumentType": "EQUITY",
                      "regularMarketPrice": 231.5, "previousClose": 229.0,
                      "fiftyTwoWeekHigh": 260.1, "fiftyTwoWeekLow": 169.2,
                      "regularMarketTime": 1792180800, "gmtoffset": -14400 },
            "timestamp": [1792008000, 1792094400, 1792180800, 1792180900],
            "indicators": {
                "quote": [{ "close": [229.9, null, 231.0, 231.5] }],
                "adjclose": [{ "adjclose": [229.0, null, 230.5, 231.5] }]
            }
        }] } });
        let h = parse_chart(&body).unwrap();
        assert_eq!(h.name, "Apple Inc.");
        assert_eq!(
            h.closes,
            [229.0, 231.5],
            "nulls dropped, a repeated day keeps the later bar"
        );
        assert_eq!(h.dates[0].to_string(), "2026-10-14");
        assert_eq!(h.previous_close, Some(229.0));
        assert_eq!(h.high_52w, Some(260.1));
        assert_eq!(timestamp(h.time, h.offset), "2026-10-16T16:00:00-04:00");
        assert!(parse_chart(&json!({ "chart": { "result": null } })).is_err());
    }

    #[test]
    fn long_charts_are_thinned_keeping_the_last_close() {
        let h = history(trending(1000, 0.01));
        let s = series_json(&h);
        let close = s["close"].as_array().unwrap();
        assert!(close.len() <= MAX_CHART_POINTS, "{}", close.len());
        assert_eq!(
            close.last().unwrap().as_f64().unwrap(),
            round_price(*h.closes.last().unwrap())
        );
        assert_eq!(s["dates"].as_array().unwrap().last().unwrap(), "2026-10-16");
    }

    #[test]
    fn news_is_parsed_and_shared_articles_are_merged() {
        let body = json!({ "news": [
            { "title": "Apple unveils", "publisher": "Reuters", "link": "https://r.com/a",
              "providerPublishTime": 1792180000,
              "thumbnail": { "resolutions": [
                  { "url": "https://img/orig.jpg", "width": 1200 },
                  { "url": "https://img/140.jpg", "width": 140 } ] } },
            { "title": "Insecure", "link": "http://x.com/b" },
            { "title": "  ", "link": "https://x.com/c" },
            { "title": "Chips rally", "publisher": "AP", "link": "https://ap.org/d" },
        ] });
        let apple = parse_news(&body, "AAPL", 5);
        assert_eq!(apple.len(), 2, "http links and blank titles are dropped");
        assert_eq!(apple[0].thumbnail.as_deref(), Some("https://img/140.jpg"));
        assert_eq!(
            timestamp(apple[0].published, 0),
            "2026-10-16T19:46:40+00:00"
        );
        let mut all = apple;
        merge_news(&mut all, parse_news(&body, "MSFT", 1));
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].symbols, ["AAPL", "MSFT"]);
    }
}
