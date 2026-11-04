# cedar-contrib-train

A worked example of reinforcement learning on a [cedar-contrib](../README.md) release with a
small Qwen model, as a [uv](https://docs.astral.sh/uv/) project. The base install needs only the
standard library, enough to read a release and show what the training data would be; the
`train` extra adds torch, transformers, TRL and PEFT.

```sh
# From the repo root. uv makes contrib/training/.venv on first use and keeps it in sync.
uv run --project contrib/training cedar-contrib-train rft --release out/v2026.11 --dry-run

# Stage 1: rejection-sampling fine-tuning on the trajectories
uv run --project contrib/training --extra train cedar-contrib-train rft \
  --release out/v2026.11 --model Qwen/Qwen2.5-1.5B-Instruct --out ckpt/rft

# Stage 2: GRPO on the successful trajectories' prompts, from stage 1's weights, with a judge
CONTRIB_JUDGE_URL=http://localhost:8000/v1 CONTRIB_JUDGE_MODEL=judge \
  uv run --project contrib/training --extra train cedar-contrib-train grpo \
  --release out/v2026.11 --model ckpt/rft --out ckpt/grpo
```

`pip install -e "contrib/training[train]"` works too, in an environment you manage; the command
is then `cedar-contrib-train`.

## What it does

A release is `<version>/manifest.json` and one `bundles.jsonl` per license and source model,
each line `{receipt_id, sha256, tags, label_weight, bundle}`. The script honours the manifest's
`excluded_sha256` (withdrawn contributions), reads only the licenses named (`--licenses`), skips
contributors whose labels were trusted less than `--min-weight`, and keeps the canary string in
every sample.

- **`rft`**: every trajectory becomes a chat in Qwen's own tool-call format (the system prompt
  with its stand-ins, `tool_calls` on assistant messages, `role: tool` results), gets a reward
  from the person's outcome and thumbs and how the run ended, discounted by the contributor's
  label weight, and the good ones (`--keep-at`) are fine-tuned on with LoRA, the best twice.
- **`grpo`**: the successful trajectories' prompts become tasks; the model samples several first
  moves per task and is rewarded for well-formed tool calls against the task's tool definitions,
  a judge's comparison with the contributed answer (`CONTRIB_JUDGE_URL`, `CONTRIB_JUDGE_MODEL`,
  `CONTRIB_JUDGE_API_KEY`; without a judge that reward is zero), and a safety penalty from the
  screener's destructive and exfiltration patterns.

Stage 2 is single-step: the model's first move, not a whole episode, since the tools do not run
during training. A whole episode needs an environment, which is what cedar-bench's hermetic
stack is for; the `sandbox_only` tag marks the trajectories that can be replayed there
(`--only-sandbox`).

To change a dependency, edit `pyproject.toml` and run `uv lock` in `contrib/training/` to update
`uv.lock`.
