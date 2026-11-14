//! How Cedar starts `llama-server` and where it then answers.

use crate::catalog::{Model, CONTEXT};
use crate::install::ModelFiles;

/// The environment variable llama-server reads its API key from. The key goes there rather than
/// on the command line, where other programs of the machine could read it.
pub const API_KEY_ENV: &str = "LLAMA_API_KEY";

/// Answers 200 once the model is loaded, 503 while it loads.
pub const HEALTH_PATH: &str = "/health";

/// The arguments for serving `model` from its files on this computer only. All layers go to the
/// GPU, the vision projector is loaded so the model reads images, the chat template runs through
/// Jinja (which tool calling needs), and there is one slot so that it has the whole context.
pub fn args(model: &Model, files: &ModelFiles, port: u16) -> Vec<String> {
    [
        "--model",
        &files.weights.to_string_lossy(),
        "--mmproj",
        &files.mmproj.to_string_lossy(),
        "--alias",
        model.id,
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--ctx-size",
        &CONTEXT.to_string(),
        "--gpu-layers",
        "all",
        "--parallel",
        "1",
        "--jinja",
        "--no-webui",
    ]
    .map(str::to_string)
    .to_vec()
}

/// The address cedar registers as the model server.
pub fn base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line() {
        let model = crate::catalog::by_id("qwen3.6-35b-a3b").unwrap();
        let files = ModelFiles {
            weights: "/data/models/qwen3.6-35b-a3b/Qwen3.6-35B-A3B-Q4_K_M.gguf".into(),
            mmproj: "/data/models/qwen3.6-35b-a3b/mmproj-Qwen3.6-35B-A3B-Q8_0.gguf".into(),
        };
        let line = args(model, &files, 18081).join(" ");
        assert_eq!(
            line,
            "--model /data/models/qwen3.6-35b-a3b/Qwen3.6-35B-A3B-Q4_K_M.gguf \
             --mmproj /data/models/qwen3.6-35b-a3b/mmproj-Qwen3.6-35B-A3B-Q8_0.gguf \
             --alias qwen3.6-35b-a3b --host 127.0.0.1 --port 18081 \
             --ctx-size 32768 --gpu-layers all --parallel 1 --jinja --no-webui"
        );
        // The key is never an argument.
        assert!(!line.contains("api-key"));
        assert_eq!(base_url(18081), "http://127.0.0.1:18081/v1");
    }
}
