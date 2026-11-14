//! The models Cedar downloads for llama.cpp, one per size of GPU memory. Each is pinned to a
//! commit of a Hugging Face repository and to the SHA-256 of every file.
//!
//! All of them read images as well as text: each comes with its vision projector (`mmproj`), which
//! llama-server loads beside the model, and cedar is told that the model takes images.
//!
//! They are the website's own suggestions for a model that sees (`PICKS` in
//! docs/landing/models.html, the `images` lists): Gemma 4 E4B for a 12 or 16 GB graphics card,
//! Gemma 4 26B-A4B from 20 GB, Qwen3.6 35B-A3B from 24 GB, Qwen3.5 122B-A10B where 128 GB allow
//! it. Keep the two in step. Mixture-of-experts models are preferred to dense ones of the same
//! size, as there: they answer several times faster.

use serde::Serialize;

/// A file of a model: its path in the repository, and what it must be once downloaded.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct File {
    pub path: &'static str,
    /// Bytes.
    pub size: u64,
    /// Lowercase hexadecimal.
    pub sha256: &'static str,
}

impl File {
    /// The name it is saved under: the last part of its path.
    pub fn name(&self) -> &'static str {
        self.path.rsplit('/').next().unwrap_or(self.path)
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Model {
    /// The name llama-server serves it under and cedar registers.
    pub id: &'static str,
    pub label: &'static str,
    /// The Hugging Face repository and its commit.
    pub repo: &'static str,
    pub commit: &'static str,
    /// The GGUF file, or the parts of one that is split, in order: llama-server is given the
    /// first and finds the others beside it.
    pub weights: &'static [File],
    /// The vision projector, which lets the model read images.
    pub mmproj: File,
    /// The least GPU memory, in MiB, that holds all of it and a context of [`CONTEXT`] tokens.
    pub min_budget_mb: u64,
    /// The most tokens of one reply. cedar gives up on a reply after 300 seconds, so this stays
    /// below its default of 16384, which only a fast GPU writes in that time.
    pub max_tokens: u32,
    /// The `reasoning_effort` values the model takes; none when its template has no such switch.
    pub reasoning_levels: &'static [&'static str],
    pub license: &'static str,
    /// A small model: it answers, but misses steps in longer tasks, so where it is all that fits
    /// a hosted provider serves Cedar better and stays the recommended way.
    pub small: bool,
}

/// The context every model is run with. Cedar resends the whole conversation each turn, and its
/// documentation asks for at least this much.
pub const CONTEXT: u32 = 32768;

impl Model {
    /// Every file to download: the weights, then the vision projector.
    pub fn files(&self) -> impl Iterator<Item = &File> {
        self.weights.iter().chain(std::iter::once(&self.mmproj))
    }

    /// The size of the whole download, in bytes.
    pub fn size(&self) -> u64 {
        self.files().map(|f| f.size).sum()
    }

    pub fn url(&self, file: &File) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.repo, self.commit, file.path
        )
    }

    /// The page that describes the model and its license.
    pub fn page(&self) -> String {
        format!("https://huggingface.co/{}", self.repo)
    }
}

/// From the smallest to the largest. The thresholds sit a little under round sizes because cards
/// report slightly less than their nominal memory (a 24 GB card says 24564 MiB).
pub static MODELS: [Model; 4] = [
    Model {
        id: "gemma-4-e4b",
        label: "Gemma 4 E4B",
        repo: "ggml-org/gemma-4-E4B-it-GGUF",
        commit: "8903cb408c7df39cde2ed15fac397e6d7be92951",
        // 8-bit: on a 12 GB card it leaves over 3 GB for the context, and loses less than 4-bit.
        weights: &[File {
            path: "gemma-4-E4B-it-Q8_0.gguf",
            size: 8_031_237_344,
            sha256: "49008e7cc7edadf154c8748904a89ee86eb6905d220aac568f6b7b92ca6aa0c6",
        }],
        mmproj: File {
            path: "mmproj-gemma-4-E4B-it-Q8_0.gguf",
            size: 559_874_848,
            sha256: "9257f21a0f5358204b4a612cff83fad2e56412f4f607f0049331ce60c0cf9d34",
        },
        min_budget_mb: 11_500,
        max_tokens: 8192,
        reasoning_levels: &[],
        license: "Apache-2.0",
        small: true,
    },
    Model {
        id: "gemma-4-26b-a4b",
        label: "Gemma 4 26B-A4B",
        repo: "ggml-org/gemma-4-26B-A4B-it-GGUF",
        commit: "1ca99aae748b4b01f609fa51eb23682ee5f00b85",
        weights: &[File {
            path: "gemma-4-26B-A4B-it-Q4_0.gguf",
            size: 14_618_145_824,
            sha256: "d208665ab1cd3a69f7a9a4bc59430e8448c8093d9b06334f566ac59d6d504a03",
        }],
        mmproj: File {
            path: "mmproj-gemma-4-26B-A4B-it-Q8_0.gguf",
            size: 806_408_320,
            sha256: "cc4e855736da450bf1e162d8cccfe0ad685727d0c9e04ef7dd8d884f3121039b",
        },
        min_budget_mb: 19_500,
        max_tokens: 8192,
        reasoning_levels: &[],
        license: "Apache-2.0",
        small: false,
    },
    Model {
        id: "qwen3.6-35b-a3b",
        label: "Qwen3.6 35B-A3B",
        repo: "ggml-org/Qwen3.6-35B-A3B-GGUF",
        commit: "52f9f9b8052fb9c8b69bba4a95eda0f319ac3d2a",
        weights: &[File {
            path: "Qwen3.6-35B-A3B-Q4_K_M.gguf",
            size: 20_542_732_576,
            sha256: "f4ffe03c9139e6a905a5d346f77d9fabdd179367c3280dfd005d1e75ca5ed8c8",
        }],
        mmproj: File {
            path: "mmproj-Qwen3.6-35B-A3B-Q8_0.gguf",
            size: 614_194_304,
            sha256: "904cbf8c8e876220066ab3bf676c7efa40f3da372276fdaf8b01d2fb2a37a51d",
        },
        min_budget_mb: 23_500,
        max_tokens: 8192,
        reasoning_levels: &[],
        license: "Apache-2.0",
        small: false,
    },
    Model {
        id: "qwen3.5-122b-a10b",
        label: "Qwen3.5 122B-A10B",
        // llama.cpp's own organization has no GGUF of this one; the website's guide points here too.
        repo: "unsloth/Qwen3.5-122B-A10B-GGUF",
        commit: "51eab4d59d53f573fb9206cb3ce613f1d0aa392b",
        weights: &[
            File {
                path: "Q4_K_M/Qwen3.5-122B-A10B-Q4_K_M-00001-of-00003.gguf",
                size: 10_943_552,
                sha256: "467c9bd92ea518539cf75bf5a5fbfbd35e9a0b40d766ccaa67bf120e12041df3",
            },
            File {
                path: "Q4_K_M/Qwen3.5-122B-A10B-Q4_K_M-00002-of-00003.gguf",
                size: 49_968_146_912,
                sha256: "90db14846413aebdac365b57206441437cac5f7e5037d94b325f0167f902e6e7",
            },
            File {
                path: "Q4_K_M/Qwen3.5-122B-A10B-Q4_K_M-00003-of-00003.gguf",
                size: 26_557_874_144,
                sha256: "e3c24b8ebec070bb4f69ea0aca25a16531da7440cd515529953e046882901f97",
            },
        ],
        mmproj: File {
            path: "mmproj-F16.gguf",
            size: 908_724_960,
            sha256: "691af71bd41d437ce4a1d989f589c4b492237d702b33819ed8f897f9fae6e725",
        },
        min_budget_mb: 81_000,
        max_tokens: 8192,
        reasoning_levels: &[],
        license: "Apache-2.0",
        small: false,
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
        assert_eq!(id(11_264), None); // an 11 GB card
        assert_eq!(id(12_288), Some("gemma-4-e4b")); // a 12 GB card
        assert_eq!(id(12_282), Some("gemma-4-e4b")); // what a 3080 Ti reports
        assert_eq!(id(16_376), Some("gemma-4-e4b")); // a 16 GB card
        assert_eq!(id(20_470), Some("gemma-4-26b-a4b")); // a 20 GB card
        assert_eq!(id(21_845), Some("gemma-4-26b-a4b")); // a 32 GB Mac
        assert_eq!(id(24_564), Some("qwen3.6-35b-a3b")); // a 24 GB card
        assert_eq!(id(24_576), Some("qwen3.6-35b-a3b")); // a 36 GB Mac
        assert_eq!(id(32_607), Some("qwen3.6-35b-a3b")); // a 32 GB card
        assert_eq!(id(49_140), Some("qwen3.6-35b-a3b"));
        assert_eq!(id(65_536), Some("qwen3.6-35b-a3b")); // a 96 GB Mac
        assert_eq!(id(87_381), Some("qwen3.5-122b-a10b")); // a 128 GB Mac
    }

    #[test]
    fn the_catalog_is_well_formed() {
        let hex64 =
            |s: &str| s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        for (i, m) in MODELS.iter().enumerate() {
            assert_eq!(by_id(m.id), Some(m));
            assert!(m
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b)));
            assert!(m.commit.len() == 40 && m.commit.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(!m.weights.is_empty());
            let mut names = Vec::new();
            for file in m.files() {
                assert!(
                    hex64(file.sha256) && file.size > 0,
                    "{} {}",
                    m.id,
                    file.path
                );
                assert!(file.path.ends_with(".gguf") && !file.path.contains(".."));
                assert!(
                    m.url(file).starts_with("https://huggingface.co/")
                        && m.url(file).ends_with(file.path)
                );
                names.push(file.name());
            }
            // Every one reads images: a projector of its own, saved beside the weights.
            assert!(m.mmproj.name().starts_with("mmproj"), "{}", m.id);
            let count = names.len();
            names.sort();
            names.dedup();
            assert_eq!(
                names.len(),
                count,
                "{}: two files would be saved under one name",
                m.id
            );
            // All of it and some context fit in the memory the model asks for.
            assert!(m.size() + (2 << 30) <= m.min_budget_mb << 20, "{}", m.id);
            assert!(m.max_tokens < CONTEXT);
            if i > 0 {
                assert!(
                    MODELS[i - 1].min_budget_mb < m.min_budget_mb,
                    "sorted by memory"
                );
            }
            // Only the smallest is a small model.
            assert_eq!(m.small, i == 0, "{}", m.id);
        }
        // A split model's parts are in order, the first being the one llama-server is given.
        let split = by_id("qwen3.5-122b-a10b").unwrap();
        assert_eq!(split.weights.len(), 3);
        assert!(split.weights[0].name().ends_with("-00001-of-00003.gguf"));
        assert_eq!(split.size(), 77_445_689_568);
    }
}
