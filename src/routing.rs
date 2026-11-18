//! Choosing a model for a run that names none, while the `model_selection` setting is off (the
//! default; Settings → Advanced in the chat turns it on, and the pickers come back).
//!
//! With one model registered, that one is used. With several, they are ranked by size, read off
//! their names (the parameter count: `qwen3:30b`, `gemma-3-270m`, `Llama-3.3-70B-Instruct`), and
//! the run moves between the smallest and the biggest (see [`Tiers`]):
//!
//! - planning (the planning call, and a run that follows a plan), reviewing a conversation
//!   (`POST /v1/compact`), complex tool calls (see [`complex_tool`]) and long tasks (more than
//!   [`SMALL_TURNS`] turns) go to the biggest model;
//! - everything else, the everyday questions and the simple lookups, goes to the smallest.
//!
//! A run starts on the smallest model unless it plans. When that model reaches for a complex
//! tool, its reply is dropped and the turn is asked again of the biggest, which then keeps the
//! run: a run never goes back to the smaller model.

use crate::models::ModelEntry;
use crate::store::Store;
use anyhow::Result;
use serde_json::json;

const SELECTION_SETTING: &str = "model_selection";

/// How many turns a run may take on the smallest model; a run that needs more is a complex task
/// and goes on with the biggest.
pub const SMALL_TURNS: u32 = 4;

/// Whether users choose the model themselves (the saved `model_selection`; off unless an admin
/// turned it on). Off, a run that names no model is given one by its size (see the module).
pub async fn selection_on(store: &Store) -> Result<bool> {
    Ok(store
        .setting(SELECTION_SETTING)
        .await?
        .and_then(|v| v.as_bool())
        .unwrap_or(false))
}

/// Save `model_selection`. Off is the default, so it is saved as no setting at all.
pub async fn set_selection(store: &Store, on: bool) -> Result<()> {
    store
        .set_setting(SELECTION_SETTING, on.then_some(json!(true)).as_ref())
        .await
}

/// The parameter count in `name`, in billions: `7b`, `1.5B`, `270m`, `E4B` (Gemma's effective
/// count) or `8x7b` (a mixture of experts), standing as a word of its own between separators.
/// The largest wins when a name has several (`Qwen3-30B-A3B`). `None` when the name has none;
/// quantization (`Q4_K_M`, `4bit`), context sizes (`4k`) and versions (`v4`, `3.5`) are not
/// counts.
pub fn param_count(name: &str) -> Option<f64> {
    let name = name.to_ascii_lowercase();
    let name = name
        .strip_suffix(".gguf")
        .or_else(|| name.strip_suffix(".bin"))
        .unwrap_or(&name);
    name.split(|c: char| !(c.is_ascii_alphanumeric() || c == '.'))
        .filter_map(count_word)
        .fold(None, |best: Option<f64>, n| Some(best.map_or(n, |b| b.max(n))))
}

/// One word of a name as a parameter count in billions, if it is one.
fn count_word(word: &str) -> Option<f64> {
    let (number, unit) = word.split_at(word.len().checked_sub(1)?);
    let scale = match unit {
        "b" => 1.0,
        "m" => 0.001,
        _ => return None,
    };
    // `8x7b`: eight experts of seven billion each.
    if let Some((experts, each)) = number.split_once('x') {
        let experts = plain_number(experts).filter(|n| n.fract() == 0.0)?;
        return Some(experts * plain_number(each)? * scale);
    }
    // `e4b`: Gemma 3n's effective parameters.
    let number = number.strip_prefix('e').unwrap_or(number);
    Some(plain_number(number)? * scale)
}

/// `7`, `1.5`: digits with at most one point between them, and more than nothing.
fn plain_number(s: &str) -> Option<f64> {
    let digits_and_points = !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '.');
    let shaped = s.starts_with(|c: char| c.is_ascii_digit()) && s.ends_with(|c: char| c.is_ascii_digit());
    (digits_and_points && shaped && s.matches('.').count() <= 1)
        .then(|| s.parse::<f64>().ok())
        .flatten()
        .filter(|n| *n > 0.0)
}

/// Words that mark the small model of a family whose names carry no parameter count.
const SMALL_WORDS: [&str; 10] = [
    "mini", "nano", "micro", "tiny", "small", "lite", "flash", "haiku", "instant", "fast",
];

/// How big a model is, as far as its name tells. Ordered smallest first: a counted model, by its
/// count; then a model without a count whose name says it is the small one of its family
/// (`gpt-5-mini`); then any other model without a count. Names without one are mostly the
/// hosted models, which are bigger than what people run themselves.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub enum Size {
    Params(f64),
    SmallOfFamily,
    Unknown,
}

impl Size {
    fn rank(self) -> (u8, f64) {
        match self {
            Size::Params(n) => (0, n),
            Size::SmallOfFamily => (1, 0.0),
            Size::Unknown => (2, 0.0),
        }
    }
}

/// The size of a registered model: from its name on the server, else its label, else its id.
pub fn size_of(entry: &ModelEntry) -> Size {
    let names = [&entry.model, &entry.label, &entry.id];
    if let Some(n) = names.iter().find_map(|n| param_count(n)) {
        return Size::Params(n);
    }
    let small = entry
        .model
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| SMALL_WORDS.contains(&w));
    if small {
        Size::SmallOfFamily
    } else {
        Size::Unknown
    }
}

/// The two models a run moves between: the smallest and the biggest of the registry. They are
/// the same entry when one model is registered, or when the names tell no difference in size.
#[derive(Debug, Clone, PartialEq)]
pub struct Tiers {
    pub small: ModelEntry,
    pub big: ModelEntry,
}

impl Tiers {
    /// The tiers among `models` (in picker order); `None` when none is registered. With
    /// `images`, only the models that take images count, as long as there is one. Models of the
    /// same size are told apart by the default flag, then by their order: the default one wins
    /// either tier, so models that all look the same size leave the default one for everything.
    pub fn pick(models: &[ModelEntry], images: bool) -> Option<Tiers> {
        let seeing: Vec<&ModelEntry> = models.iter().filter(|m| m.multimodal).collect();
        let pool: Vec<&ModelEntry> = if images && !seeing.is_empty() {
            seeing
        } else {
            models.iter().collect()
        };
        // Lower is preferred among equals: the default entry, then the earlier one.
        let order = |i: usize, m: &ModelEntry| (!m.is_default, i);
        let ranked = || pool.iter().enumerate().map(|(i, m)| (size_of(m).rank(), order(i, m), *m));
        let small = ranked().min_by(|a, b| {
            a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1))
        })?;
        let big = ranked().min_by(|a, b| {
            b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1))
        })?;
        Some(Tiers {
            small: small.2.clone(),
            big: big.2.clone(),
        })
    }

    /// Whether there is a choice to make: two different models.
    pub fn differ(&self) -> bool {
        self.small.id != self.big.id
    }
}

/// The tools whose calls are for the biggest model: the ones that run the model's own code or
/// write files in the sandbox, drive the browser, or ask a model themselves (`search_products`,
/// `plan_trip`), and proposing a plan. So is every call that changes something outside the
/// sandbox (see [`crate::tools::writes`]). Reading, searching and the cards (weather, maps,
/// stocks) stay with the smallest.
pub fn complex_tool(name: &str) -> bool {
    matches!(
        name,
        "bash"
            | "run_python"
            | "write_file"
            | "generate_pdf"
            | "generate_chart"
            | "browser"
            | "search_products"
            | "plan_trip"
    ) || name == crate::plan::TOOL
        || crate::tools::writes(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn entry(id: &str, model: &str) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            label: id.into(),
            model: model.into(),
            base_url: "http://localhost:8000/v1".into(),
            reasoning_levels: vec![],
            default_reasoning: None,
            multimodal: false,
            max_tokens: 1024,
            context_window: None,
            is_default: false,
            price: None,
            position: 0,
            listed_models: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn parameter_counts_are_read_off_names() {
        let cases = [
            ("qwen2.5-coder:32b", Some(32.0)),
            ("llama3.1:8b-instruct-q4_K_M", Some(8.0)),
            ("gemma-3-27b-it-Q4_K_M.gguf", Some(27.0)),
            ("meta-llama/Llama-3.3-70B-Instruct", Some(70.0)),
            ("DeepSeek-R1-Distill-Qwen-1.5B", Some(1.5)),
            ("Phi-3.5-mini-3.8B.gguf", Some(3.8)),
            ("gemma-3-270m-it", Some(0.27)),
            ("smollm2:360m", Some(0.36)),
            ("gemma-3n-E4B-it", Some(4.0)),
            ("mixtral-8x7b-instruct", Some(56.0)),
            // The total, not the active parameters.
            ("Qwen3-30B-A3B", Some(30.0)),
            ("gpt-oss-120b", Some(120.0)),
            // Not counts: quantization, context sizes, versions, embeddings' names.
            ("mlx-community/model-4bit", None),
            ("phi-3-mini-4k-instruct", None),
            ("deepseek-ai/DeepSeek-V4-Flash", None),
            ("gpt-4o", None),
            ("claude-3.5-sonnet", None),
            ("bge-m3", None),
            ("model-q8_0", None),
            ("b", None),
            ("", None),
        ];
        for (name, want) in cases {
            let got = param_count(name);
            match (got, want) {
                (Some(g), Some(w)) => assert!((g - w).abs() < 1e-9, "{name}: {g} != {w}"),
                (None, None) => {}
                _ => panic!("{name}: got {got:?}, want {want:?}"),
            }
        }
    }

    #[test]
    fn the_label_and_id_are_read_when_the_name_has_no_count() {
        let mut e = entry("local", "my-model");
        assert_eq!(size_of(&e), Size::Unknown);
        e.label = "My model (12B)".into();
        assert_eq!(size_of(&e), Size::Params(12.0));
        assert_eq!(size_of(&entry("qwen-7b", "served")), Size::Params(7.0));
        assert_eq!(size_of(&entry("mini", "gpt-5-mini")), Size::SmallOfFamily);
    }

    #[test]
    fn one_model_is_both_tiers() {
        assert_eq!(Tiers::pick(&[], false), None);
        let tiers = Tiers::pick(&[entry("only", "llama3.1:8b")], false).unwrap();
        assert_eq!((tiers.small.id.as_str(), tiers.big.id.as_str()), ("only", "only"));
        assert!(!tiers.differ());
    }

    #[test]
    fn the_smallest_and_the_biggest_are_picked_by_count() {
        let models = [
            entry("mid", "gemma-3-12b-it"),
            entry("big", "Llama-3.3-70B-Instruct"),
            entry("small", "qwen3:4b"),
        ];
        let tiers = Tiers::pick(&models, false).unwrap();
        assert_eq!((tiers.small.id.as_str(), tiers.big.id.as_str()), ("small", "big"));
        assert!(tiers.differ());
    }

    #[test]
    fn names_without_a_count_rank_above_counted_ones() {
        let models = [
            entry("hosted", "gpt-5"),
            entry("hosted-mini", "gpt-5-mini"),
            entry("local", "Llama-3.3-70B-Instruct"),
        ];
        let tiers = Tiers::pick(&models, false).unwrap();
        assert_eq!((tiers.small.id.as_str(), tiers.big.id.as_str()), ("local", "hosted"));
        let tiers = Tiers::pick(&models[..2], false).unwrap();
        assert_eq!((tiers.small.id.as_str(), tiers.big.id.as_str()), ("hosted-mini", "hosted"));
    }

    #[test]
    fn models_of_one_size_leave_the_default_for_everything() {
        let mut models = [entry("a", "model-a"), entry("b", "model-b"), entry("c", "model-c")];
        models[1].is_default = true;
        let tiers = Tiers::pick(&models, false).unwrap();
        assert_eq!((tiers.small.id.as_str(), tiers.big.id.as_str()), ("b", "b"));
        // Without a default (which the registry never has), the first.
        models[1].is_default = false;
        let tiers = Tiers::pick(&models, false).unwrap();
        assert_eq!((tiers.small.id.as_str(), tiers.big.id.as_str()), ("a", "a"));
    }

    #[test]
    fn images_narrow_the_choice_to_models_that_take_them() {
        let mut models = [
            entry("small", "qwen3:4b"),
            entry("seeing", "gemma-3-12b-it"),
            entry("big", "Llama-3.3-70B-Instruct"),
        ];
        models[1].multimodal = true;
        let tiers = Tiers::pick(&models, true).unwrap();
        assert_eq!((tiers.small.id.as_str(), tiers.big.id.as_str()), ("seeing", "seeing"));
        // No model takes images: all of them count, and the caller refuses or goes without.
        models[1].multimodal = false;
        let tiers = Tiers::pick(&models, true).unwrap();
        assert_eq!((tiers.small.id.as_str(), tiers.big.id.as_str()), ("small", "big"));
    }

    #[test]
    fn code_browsing_and_writes_are_complex() {
        for name in ["bash", "run_python", "write_file", "browser", "search_products", "propose_plan", "gmail_send_message"] {
            assert!(complex_tool(name), "{name}");
        }
        for name in ["web_search", "fetch_url", "weather_forecast", "read_file", "show_map", "remember", "gmail_search_threads", "ask_user"] {
            assert!(!complex_tool(name), "{name}");
        }
    }
}
