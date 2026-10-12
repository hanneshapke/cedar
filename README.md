# cedar

A minimal agent harness in Rust. It sends a conversation to Claude, runs any tool calls the model makes, feeds the results back, and stops when the model stops asking for tools. The whole thing is one file, `main.rs`.

## How it works

1. Your message is appended to the conversation history.
2. The history, system prompt, and tool definitions are sent to the Anthropic Messages API.
3. The reply is scanned for content blocks:
   - `text` is collected as the final answer.
   - `tool_use` is executed locally and the result is sent back as a `tool_result`. Errors are returned to the model with `is_error: true` so it can recover.
   - `server_tool_use` (web search) is run by Anthropic. It is only logged here.
4. If the stop reason is `tool_use` (or `pause_turn` for a paused server-tool loop), go back to step 2. Otherwise return the text.

The loop gives up with an error after 25 round trips for a single user message.

## Quick start

You need a recent Rust toolchain and an Anthropic API key.

```sh
export ANTHROPIC_API_KEY=sk-ant-...

# One-shot: pass the task as the first argument
cargo run -- "list the files here and summarize them"

# Interactive REPL: run with no argument, type "exit" or "quit" to leave
cargo run
```

Tool calls are logged to stderr, local tools in yellow and web searches in magenta. The final answer goes to stdout. In the REPL the conversation history is kept until you exit, but nothing is saved between runs.

## Configuration

| Setting | Where | Default |
| --- | --- | --- |
| `ANTHROPIC_API_KEY` | environment | required |
| `ANTHROPIC_MODEL` | environment | `claude-sonnet-5-5` |
| `MAX_TURNS` | constant in `main.rs` | 25 |
| `max_tokens` | `Agent::send` in `main.rs` | 4096 |
| Web search `max_uses` | `with_web_search(5)` in `main()` | 5 per request |
| System prompt | `main()` | "helpful coding agent working in the current directory" |

## Tools

| Tool | What it does |
| --- | --- |
| `bash` | Runs a command with `sh -c` and returns stdout, stderr, and the exit status if it was non-zero. |
| `read_file` | Reads a text file and returns its contents. |
| `write_file` | Writes content to a path, creating or overwriting the file. |
| `web_search` | Anthropic's server-side web search. It is executed by Anthropic and needs no local code. |

## Adding a tool

A tool is a name, a description, a JSON schema for its input, and a function. Implement the `Tool` trait and add it to the list in `main()`.

```rust
struct Now;
impl Tool for Now {
    fn name(&self) -> &str { "now" }
    fn description(&self) -> &str { "Return the current Unix time in seconds." }
    fn schema(&self) -> Value { json!({ "type": "object", "properties": {} }) }
    fn call(&self, _input: &Value) -> Result<String> {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        Ok(secs.to_string())
    }
}

// in main():
vec![Box::new(Bash), Box::new(ReadFile), Box::new(WriteFile), Box::new(Now)]
```

The `definition()` method builds the JSON the API expects, so you do not write it by hand.

## Safety

`bash` and `write_file` run with your full user permissions, and there is no confirmation prompt. The model can delete files, overwrite files, and run any command. Run cedar in a disposable directory, a container, or a VM, especially with tasks you did not write yourself.

## Limitations

- No streaming. You see the answer only after the model finishes.
- Tool output is not truncated, so a large file or noisy command can fill the context window.
- Conversation history grows without bound within a session.
- Requests are not retried on API errors. In the REPL an error is printed and the session continues.

## License

Apache 2.0. See [LICENSE](LICENSE).
