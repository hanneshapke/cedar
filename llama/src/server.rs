//! How Cedar starts `llama-server` and where it then answers.

use crate::catalog::{Model, CONTEXT};

/// The environment variable llama-server reads its API key from. The key goes there rather than
/// on the command line, where other programs of the machine could read it.
pub const API_KEY_ENV: &str = "LLAMA_API_KEY";

/// Answers 200 once the model is loaded, 503 while it loads.
pub const HEALTH_PATH: &str = "/health";

/// The arguments for serving `model` from the file at `model_path` on this computer only. All
/// layers go to the GPU, the chat template runs through Jinja (which tool calling needs), and
/// there is one slot so that it has the whole context.
pub fn args(model: &Model, model_path: &str, port: u16) -> Vec<String> {
    [
        "--model",
        model_path,
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
        let args = args(
            crate::catalog::by_id("gpt-oss-20b").unwrap(),
            "/data/models/gpt-oss-20b-MXFP4.gguf",
            18081,
        );
        let line = args.join(" ");
        assert_eq!(
            line,
            "--model /data/models/gpt-oss-20b-MXFP4.gguf --alias gpt-oss-20b --host 127.0.0.1 --port 18081 \
             --ctx-size 32768 --gpu-layers all --parallel 1 --jinja --no-webui"
        );
        // The key is never an argument.
        assert!(!line.contains("api-key"));
        assert_eq!(base_url(18081), "http://127.0.0.1:18081/v1");
    }
}
