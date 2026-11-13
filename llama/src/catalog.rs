//! The models Cedar downloads for llama.cpp, one per size of GPU memory. Each is a single GGUF
//! file from llama.cpp's own organization on Hugging Face, pinned to a commit and a SHA-256, and
//! each calls tools well (see the website's list of models).

use serde::Serialize;

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Model {
    /// The name llama-server serves it under and cedar registers.
    pub id: &'static str,
    pub label: &'static str,
    /// The Hugging Face repository, its commit and the file in it.
    pub repo: &'static str,
    pub commit: &'static str,
    pub file: &'static str,
    /// Bytes.
    pub size: u64,
    /// Lowercase hexadecimal.
    pub sha256: &'static str,
    /// The least GPU memory, in MiB, that holds the file and a context of [`CONTEXT`] tokens.
    pub min_budget_mb: u64,
    /// The most tokens of one reply. cedar gives up on a reply after 300 seconds, so this stays
    /// below its default of 16384, which only a fast GPU writes in that time.
    pub max_tokens: u32,
    /// The `reasoning_effort` values the model takes; none when its template has no such switch.
    pub reasoning_levels: &'static [&'static str],
    pub license: &'static str,
}

/// The context every model is run with. Cedar resends the whole conversation each turn, and its
/// documentation asks for at least this much.
pub const CONTEXT: u32 = 32768;

impl Model {
    pub fn url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.repo, self.commit, self.file
        )
    }

    /// The page that describes the model and its license.
    pub fn page(&self) -> String {
        format!("https://huggingface.co/{}", self.repo)
    }
}

/// From the smallest to the largest. The thresholds sit a little under round sizes because cards
/// report slightly less than their nominal memory (a 16 GB card says 16376 MiB).
pub static MODELS: [Model; 4] = [
    Model {
        id: "gpt-oss-20b",
        label: "gpt-oss 20B",
        repo: "ggml-org/gpt-oss-20b-GGUF",
        commit: "41e8a94d15d62070b36cf4553a0250717ecf46b8",
        file: "gpt-oss-20b-MXFP4.gguf",
        size: 12_109_566_624,
        sha256: "27cd6c432c7672cb812a92f611cf3ba7bbc35928262bb1e1253ff4ee6ae35901",
        min_budget_mb: 15_500,
        max_tokens: 8192,
        reasoning_levels: &["low", "medium", "high"],
        license: "Apache-2.0",
    },
    Model {
        id: "qwen3.6-27b",
        label: "Qwen3.6 27B",
        repo: "ggml-org/Qwen3.6-27B-GGUF",
        commit: "fec6ff7c0c86cf07f6e5ae0a1391c3a042f2017f",
        file: "Qwen3.6-27B-Q4_K_M.gguf",
        size: 19_403_683_392,
        sha256: "f3ab00cc923590947877bad882e09ada67789f747f5d3cb3c178fdf91b9aaab9",
        min_budget_mb: 23_500,
        max_tokens: 8192,
        reasoning_levels: &[],
        license: "Apache-2.0",
    },
    Model {
        id: "qwen3.6-35b-a3b",
        label: "Qwen3.6 35B-A3B",
        repo: "ggml-org/Qwen3.6-35B-A3B-GGUF",
        commit: "52f9f9b8052fb9c8b69bba4a95eda0f319ac3d2a",
        file: "Qwen3.6-35B-A3B-Q4_K_M.gguf",
        size: 20_542_732_576,
        sha256: "f4ffe03c9139e6a905a5d346f77d9fabdd179367c3280dfd005d1e75ca5ed8c8",
        min_budget_mb: 31_500,
        max_tokens: 8192,
        reasoning_levels: &[],
        license: "Apache-2.0",
    },
    Model {
        id: "gpt-oss-120b",
        label: "gpt-oss 120B",
        repo: "ggml-org/gpt-oss-120b-GGUF",
        commit: "f59990363fbbb67f01bce03e5e7bf0929e3e320b",
        file: "gpt-oss-120b-MXFP4.gguf",
        size: 63_387_346_208,
        sha256: "582bd40f6886200101f4c4ed9f25f3fe80cc14c86e9e2b37746cd8904a0c622d",
        min_budget_mb: 71_000,
        max_tokens: 8192,
        reasoning_levels: &["low", "medium", "high"],
        license: "Apache-2.0",
    },
];

/// The largest model that fits in `budget_mb` MiB of GPU memory, if any does.
pub fn choose_model(budget_mb: u64) -> Option<&'static Model> {
    MODELS.iter().rev().find(|m| m.min_budget_mb <= budget_mb)
}

pub fn by_id(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_by_memory() {
        let id = |mb| choose_model(mb).map(|m| m.id);
        assert_eq!(id(8_192), None);
        assert_eq!(id(12_288), None);
        assert_eq!(id(16_376), Some("gpt-oss-20b")); // a 16 GB card
        assert_eq!(id(24_564), Some("qwen3.6-27b")); // a 24 GB card
        assert_eq!(id(32_607), Some("qwen3.6-35b-a3b")); // a 32 GB card
        assert_eq!(id(49_140), Some("qwen3.6-35b-a3b"));
        assert_eq!(id(87_381), Some("gpt-oss-120b")); // a 128 GB Mac
    }

    #[test]
    fn the_catalog_is_well_formed() {
        for (i, m) in MODELS.iter().enumerate() {
            assert_eq!(by_id(m.id), Some(m));
            assert!(m
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b)));
            assert!(
                m.sha256.len() == 64
                    && m.sha256
                        .bytes()
                        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            );
            assert!(m.commit.len() == 40 && m.commit.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(m.file.ends_with(".gguf") && m.url().starts_with("https://huggingface.co/"));
            // The file and some context fit in the memory the model asks for.
            assert!(m.size + (2 << 30) <= m.min_budget_mb << 20, "{}", m.id);
            assert!(m.max_tokens < CONTEXT);
            if i > 0 {
                assert!(
                    MODELS[i - 1].min_budget_mb < m.min_budget_mb,
                    "sorted by memory"
                );
            }
        }
    }
}
