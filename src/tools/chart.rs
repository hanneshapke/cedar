//! `generate_chart`: turns data into a chart image (PNG, SVG, or PDF) in the workspace.
//!
//! Like `generate_pdf`, the rendering happens inside the sandbox: the chart spec is checked here,
//! staged as JSON in the container's `/tmp`, and drawn by the `cedar-render-chart` command that the
//! sandbox image ships (`sandbox/render_chart.py`, built on matplotlib).

use super::{shell_quote, stderr_text, Tool, ToolContext};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use uuid::Uuid;

/// The renderer command in the sandbox image.
const RENDERER: &str = "cedar-render-chart";
const KINDS: [&str; 7] = ["line", "bar", "barh", "area", "scatter", "pie", "histogram"];
const EXTENSIONS: [&str; 3] = [".png", ".svg", ".pdf"];
const MAX_SERIES: usize = 20;
const MAX_POINTS: usize = 100_000;

pub struct GenerateChart;

#[async_trait]
impl Tool for GenerateChart {
    fn name(&self) -> &str {
        "generate_chart"
    }

    fn description(&self) -> &str {
        "Draw a chart from data and save it as a PNG, SVG, or PDF in the workspace. Kinds: line, \
         bar, barh (horizontal bars), area, scatter, pie, histogram. Give the x-axis categories or \
         values as `labels` and one or more `series` of numbers. The image can be embedded in \
         generate_pdf Markdown with ![caption](path.png)."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "path": { "type": "string",
                              "description": "Where to save the chart, relative to /workspace. Must end in .png, .svg, or .pdf." },
                    "kind": { "type": "string", "enum": KINDS },
                    "labels": { "type": "array", "items": { "type": ["string", "number"] },
                                "description": "X-axis categories or values, one per data point (pie: slice names). Numbers for scatter. Not used by histogram." },
                    "series": { "type": "array",
                                "items": { "type": "object",
                                           "properties": {
                                               "name": { "type": "string", "description": "Legend entry." },
                                               "values": { "type": "array", "items": { "type": ["number", "null"] },
                                                           "description": "One value per label (null for a gap). For histogram, the raw samples." }
                                           },
                                           "required": ["values"] },
                                "description": "The data. Pie takes exactly one series." },
                    "title": { "type": "string" },
                    "x_label": { "type": "string" },
                    "y_label": { "type": "string" },
                    "stacked": { "type": "boolean", "description": "Stack the series (bar, barh, area)." },
                    "bins": { "type": "integer", "minimum": 1, "maximum": 500,
                              "description": "Histogram bin count (default automatic)." },
                    "log_scale": { "type": "boolean", "description": "Logarithmic value axis." },
                    "band": { "type": "object",
                              "properties": {
                                  "from": { "type": "number" },
                                  "to": { "type": "number" },
                                  "label": { "type": "string", "description": "e.g. \"Target\"." }
                              },
                              "required": ["from", "to"],
                              "description": "A shaded range on the value axis, such as a target range (not for pie)." }
                },
                "required": ["path", "kind", "series"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let spec = parse_spec(input)?;
        let path = spec["path"].as_str().expect("checked").to_string();
        let staged = format!("/tmp/{RENDERER}-{}.json", Uuid::new_v4());
        ctx.sandbox
            .write_file(&staged, spec.to_string().as_bytes())
            .await
            .context("staging the chart spec in the sandbox")?;
        let staged = shell_quote(&staged);
        let cmd = format!("{RENDERER} -- {staged}; s=$?; rm -f -- {staged}; exit $s");
        let out = ctx.sandbox.exec(&cmd, ctx.timeout, ctx.max_output).await?;
        let stderr = stderr_text(&out, ctx.max_output);
        match out.exit_code {
            Some(0) => {}
            Some(124) => bail!(
                "drawing {path} timed out after {}s; try fewer points",
                ctx.timeout.as_secs()
            ),
            Some(127) => bail!(
                "the sandbox image has no `{RENDERER}` command; rebuild it from sandbox/Dockerfile ({stderr})"
            ),
            Some(code) => bail!("drawing {path} failed (exit status {code}): {stderr}"),
            None => bail!("drawing {path} was terminated by a signal"),
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stats: Option<Value> = stdout
            .lines()
            .last()
            .and_then(|l| serde_json::from_str(l).ok());
        let mut msg = match stats {
            Some(s) if s["bytes"].is_u64() => format!("wrote {path}: {} bytes", s["bytes"]),
            _ => format!("wrote {path}"),
        };
        if !stderr.is_empty() {
            msg.push_str("\n[warnings]\n");
            msg.push_str(&stderr);
        }
        Ok(msg)
    }
}

/// Check the model's input and return the spec handed to the renderer: the known fields only,
/// with strings trimmed and empty optional fields dropped.
fn parse_spec(input: &Value) -> Result<Value> {
    let path = input["path"]
        .as_str()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| anyhow!("missing path"))?;
    if path.contains('\0') {
        bail!("path must not contain NUL bytes");
    }
    let lower = path.to_ascii_lowercase();
    if !EXTENSIONS.iter().any(|e| lower.ends_with(e)) {
        bail!("path must end in .png, .svg, or .pdf");
    }
    let kind = input["kind"]
        .as_str()
        .ok_or_else(|| anyhow!("missing kind"))?;
    if !KINDS.contains(&kind) {
        bail!("invalid kind {kind:?}; use one of {}", KINDS.join(", "));
    }

    let series = input["series"]
        .as_array()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("series must be a non-empty array"))?;
    if series.len() > MAX_SERIES {
        bail!("at most {MAX_SERIES} series");
    }
    let mut clean_series = Vec::with_capacity(series.len());
    let mut points = 0;
    for (i, s) in series.iter().enumerate() {
        let values = s["values"]
            .as_array()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow!("series {} needs a non-empty values array", i + 1))?;
        if values.iter().any(|v| !(v.is_number() || v.is_null())) {
            bail!("series {} values must be numbers or null", i + 1);
        }
        points += values.len();
        let mut entry = Map::new();
        if let Some(name) = s["name"].as_str().map(str::trim).filter(|n| !n.is_empty()) {
            entry.insert("name".into(), name.into());
        }
        entry.insert("values".into(), Value::Array(values.clone()));
        clean_series.push(Value::Object(entry));
    }
    if points > MAX_POINTS {
        bail!("{points} data points; the limit is {MAX_POINTS}");
    }

    let labels = match &input["labels"] {
        Value::Null => None,
        Value::Array(l) if l.iter().all(|v| v.is_string() || v.is_number()) => Some(l),
        _ => bail!("labels must be an array of strings or numbers"),
    };
    match (kind, labels) {
        ("histogram", _) => {}
        (_, None) => bail!("{kind} charts need labels"),
        (_, Some(labels)) => {
            for (i, s) in clean_series.iter().enumerate() {
                let n = s["values"].as_array().expect("set above").len();
                if n != labels.len() {
                    bail!(
                        "series {} has {n} values but there are {} labels",
                        i + 1,
                        labels.len()
                    );
                }
            }
            if kind == "scatter" && !labels.iter().all(Value::is_number) {
                bail!("scatter labels are the x values and must be numbers");
            }
        }
    }
    if kind == "pie" {
        if clean_series.len() != 1 {
            bail!("pie charts take exactly one series");
        }
        let values = clean_series[0]["values"].as_array().expect("set above");
        if values.iter().any(|v| v.as_f64().is_none_or(|x| x < 0.0)) {
            bail!("pie values must be non-negative numbers");
        }
    }

    let mut spec = Map::new();
    spec.insert("path".into(), path.into());
    spec.insert("kind".into(), kind.into());
    if let Some(labels) = labels.filter(|_| kind != "histogram") {
        spec.insert("labels".into(), Value::Array(labels.clone()));
    }
    spec.insert("series".into(), Value::Array(clean_series));
    for key in ["title", "x_label", "y_label"] {
        if let Some(v) = input[key].as_str().map(str::trim).filter(|v| !v.is_empty()) {
            spec.insert(key.into(), v.into());
        }
    }
    for key in ["stacked", "log_scale"] {
        match &input[key] {
            Value::Null => {}
            Value::Bool(b) => {
                spec.insert(key.into(), (*b).into());
            }
            _ => bail!("{key} must be true or false"),
        }
    }
    match &input["band"] {
        Value::Null => {}
        Value::Object(b) if kind != "pie" => {
            let (Some(from), Some(to)) = (
                b.get("from").and_then(Value::as_f64),
                b.get("to").and_then(Value::as_f64),
            ) else {
                bail!("band needs numbers from and to");
            };
            if from == to {
                bail!("band from and to must differ");
            }
            let mut band = Map::new();
            band.insert("from".into(), from.min(to).into());
            band.insert("to".into(), from.max(to).into());
            if let Some(l) = b
                .get("label")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|l| !l.is_empty())
            {
                band.insert(
                    "label".into(),
                    l.chars().take(40).collect::<String>().into(),
                );
            }
            spec.insert("band".into(), Value::Object(band));
        }
        Value::Object(_) => bail!("pie charts take no band"),
        _ => bail!("band must be an object with from and to"),
    }
    match &input["bins"] {
        Value::Null => {}
        v => match v.as_u64().filter(|b| (1..=500).contains(b)) {
            Some(b) => {
                spec.insert("bins".into(), b.into());
            }
            None => bail!("bins must be an integer from 1 to 500"),
        },
    }
    Ok(Value::Object(spec))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spec_keeps_known_fields_trimmed() {
        let input = json!({
            "path": " out/sales.png ", "kind": "bar", "labels": ["Q1", "Q2"],
            "series": [{ "name": " 2025 ", "values": [1, 2.5], "color": "red" }, { "values": [3, null] }],
            "title": " Sales ", "x_label": "", "stacked": true, "extra": 1
        });
        assert_eq!(
            parse_spec(&input).unwrap(),
            json!({
                "path": "out/sales.png", "kind": "bar", "labels": ["Q1", "Q2"],
                "series": [{ "name": "2025", "values": [1, 2.5] }, { "values": [3, null] }],
                "title": "Sales", "stacked": true
            })
        );
    }

    #[test]
    fn histograms_need_no_labels_and_drop_them() {
        let input = json!({ "path": "h.SVG", "kind": "histogram", "labels": ["a"],
                            "series": [{ "values": [1, 2, 2, 3] }], "bins": 3 });
        let spec = parse_spec(&input).unwrap();
        assert!(spec.get("labels").is_none());
        assert_eq!(spec["bins"], 3);
    }

    #[test]
    fn bad_specs_are_rejected_with_a_reason() {
        let err = |v: Value| parse_spec(&v).unwrap_err().to_string();
        let ok = json!({ "path": "c.png", "kind": "line", "labels": [1, 2], "series": [{ "values": [1, 2] }] });
        assert!(parse_spec(&ok).is_ok());
        let with = |k: &str, v: Value| {
            let mut x = ok.clone();
            x[k] = v;
            x
        };
        assert_eq!(
            err(with("path", json!("c.jpg"))),
            "path must end in .png, .svg, or .pdf"
        );
        assert!(err(with("kind", json!("donut"))).starts_with("invalid kind \"donut\""));
        assert_eq!(
            err(with("series", json!([]))),
            "series must be a non-empty array"
        );
        assert_eq!(
            err(with("series", json!([{ "values": ["1", 2] }]))),
            "series 1 values must be numbers or null"
        );
        assert_eq!(
            err(with("series", json!([{ "values": [1] }]))),
            "series 1 has 1 values but there are 2 labels"
        );
        assert_eq!(err(with("labels", Value::Null)), "line charts need labels");
        assert_eq!(
            err(with("labels", json!([{}, 1]))),
            "labels must be an array of strings or numbers"
        );
        assert_eq!(
            err(with("stacked", json!("yes"))),
            "stacked must be true or false"
        );
        assert_eq!(
            err(with("bins", json!(0))),
            "bins must be an integer from 1 to 500"
        );

        assert_eq!(
            parse_spec(&with(
                "band",
                json!({ "from": 180, "to": 70, "label": " Target " })
            ))
            .unwrap()["band"],
            json!({ "from": 70.0, "to": 180.0, "label": "Target" })
        );
        assert_eq!(
            err(with("band", json!({ "from": 1 }))),
            "band needs numbers from and to"
        );
        assert_eq!(
            err(with("band", json!({ "from": 1, "to": 1 }))),
            "band from and to must differ"
        );

        let scatter = with("kind", json!("scatter"));
        assert!(parse_spec(&scatter).is_ok());
        let mut s = scatter.clone();
        s["labels"] = json!(["a", "b"]);
        assert!(err(s).contains("must be numbers"));

        let mut pie = with("kind", json!("pie"));
        pie["series"] = json!([{ "values": [1, 2] }, { "values": [1, 2] }]);
        assert_eq!(err(pie.clone()), "pie charts take exactly one series");
        pie["series"] = json!([{ "values": [1, -2] }]);
        assert_eq!(err(pie), "pie values must be non-negative numbers");

        let many: Vec<Value> = (0..=MAX_SERIES)
            .map(|_| json!({ "values": [1, 2] }))
            .collect();
        assert!(err(with("series", Value::Array(many))).contains("at most"));
    }
}
