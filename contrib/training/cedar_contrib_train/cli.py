"""Train a small Qwen model on a cedar-contrib release, in two stages.

A release (`cedar-contrib release --out DIR`, or the Hugging Face mirror) is
`<version>/manifest.json` plus one `bundles.jsonl` per license and source model, each line
`{receipt_id, sha256, tags, label_weight, bundle}`. A bundle is one conversation: its system
prompt, tool definitions, the messages as the model saw them (text, tool_use, tool_result
blocks), how each run ended, the person's outcome and thumbs, and the stand-ins the redaction
wrote. See docs/design/contributions.md.

Stage 1, `rft` (rejection-sampling fine-tuning): every trajectory becomes a chat with tool calls
in Qwen's own format, gets a reward from what the person said and how the run ended, discounted
by the contributor's label weight, and the good ones are fine-tuned on (the best twice). This is
offline, cheap, and what self-reported labels are honestly good for.

Stage 2, `grpo` (online RL with group-relative policy optimisation, TRL): the prompts of the
successful trajectories become tasks. For each, the model samples several first turns; rewards
are (a) well-formed tool calls against the task's tool definitions, (b) a judge's comparison
with the contributed answer (an OpenAI-compatible endpoint, the same kind the screener uses),
and (c) a safety penalty from the screener's destructive and exfiltration patterns. This is
single-step: the model's first move, not a whole episode. A whole episode needs the tools to run,
which is what cedar-bench's hermetic stack is for; `sandbox_only` trajectories are the ones that
can be replayed there, and `--only-sandbox` keeps those.

    # look at the data, no GPU (the base install is the standard library only)
    uv run --project contrib/training cedar-contrib-train rft --release out/v2026.11 --dry-run
    # stage 1 (the `train` extra brings torch, transformers, TRL and PEFT)
    uv run --project contrib/training --extra train cedar-contrib-train rft \\
      --release out/v2026.11 --model Qwen/Qwen2.5-1.5B-Instruct --out ckpt/rft
    # stage 2, from stage 1's weights, with a judge
    CONTRIB_JUDGE_URL=http://localhost:8000/v1 CONTRIB_JUDGE_MODEL=judge \\
      uv run --project contrib/training --extra train cedar-contrib-train grpo \\
      --release out/v2026.11 --model ckpt/rft --out ckpt/grpo

The script never strips the canary string from a bundle, honours the manifest's `excluded_sha256`
(withdrawn contributions), and takes only the licenses you name (`--licenses cc0,cc-by-4.0`).
"""

import argparse
import json
import os
import random
import re
import sys
from pathlib import Path

# -- the release -----------------------------------------------------------------------------------

SANDBOX_TOOLS = {
    "bash", "run_python", "read_file", "write_file", "list_files", "edit_file", "generate_pdf",
    "generate_chart", "read_pdf", "read_document", "write_document", "propose_plan", "ask_user", "remember",
}


def load_release(release_dir: Path, licenses: set[str], only_sandbox: bool, min_weight: float):
    """Every usable line of a release: its manifest honoured, withdrawn hashes left out."""
    manifest = json.loads((release_dir / "manifest.json").read_text())
    if "canary" not in manifest:
        sys.exit("not a cedar-contrib release: no canary in manifest.json")
    excluded = set(manifest.get("excluded_sha256", []))
    entries = {e["sha256"]: e for e in manifest.get("entries", [])}
    rows = []
    for path in sorted(release_dir.rglob("bundles.jsonl")):
        license_id = path.parent.parent.name
        if license_id not in licenses:
            continue
        for line in path.read_text().splitlines():
            if not line.strip():
                continue
            row = json.loads(line)
            if row["sha256"] in excluded:
                continue
            entry = entries.get(row["sha256"], {})
            row["license"] = license_id
            row["label_weight"] = float(row.get("label_weight", entry.get("label_weight", 1.0)))
            row["tags"] = row.get("tags") or entry.get("tags", [])
            if row["label_weight"] < min_weight:
                continue
            if only_sandbox and "sandbox_only" not in row["tags"]:
                continue
            rows.append(row)
    return manifest, rows


# -- rewards from what the release knows -----------------------------------------------------------

OUTCOME_SCORE = {"succeeded": 1.0, "partly": 0.5, "failed": 0.0}


def trajectory_reward(row: dict) -> float:
    """What the person said, how the run ended, their thumbs, discounted by how far the
    contributor's labels have agreed with the judge. 0 to 1."""
    bundle = row["bundle"]
    outcome = (bundle.get("outcome") or {}).get("result")
    weight = row["label_weight"]
    base = 0.5
    if outcome in OUTCOME_SCORE:
        # The self-report counts for as much as the contributor's labels have proven worth;
        # the rest of the weight stays at a neutral 0.5.
        base = weight * OUTCOME_SCORE[outcome] + (1 - weight) * 0.5
    ratings = [t.get("rating") for t in bundle["turns"] if t.get("rating") is not None]
    if ratings:
        base = 0.7 * base + 0.3 * (sum(1.0 if r > 0 else 0.0 for r in ratings) / len(ratings))
    last = bundle["turns"][-1]
    if last["status"] != "completed" or last.get("stop_reason") in ("max_turns", "max_tokens"):
        base = min(base, 0.2)
    if "injection_resisted" in row["tags"]:
        base = min(1.0, base + 0.1)
    if "approval_denied" in row["tags"]:
        base = min(base, 0.6)
    return round(base, 3)


# -- Anthropic-style blocks -> Qwen chat messages ------------------------------------------------------


def to_chat(bundle: dict, turn_index: int) -> tuple[list[dict], list[dict]]:
    """One turn's conversation as `(messages, tools)` in the OpenAI/Qwen shape that Qwen's chat
    template renders: `tool_calls` on assistant messages, `role: tool` results. The system prompt
    is the one the run was sent (memory cut, stand-ins in place)."""
    turn = bundle["turns"][turn_index]
    tools = [
        {"type": "function", "function": {"name": d["name"], "description": d.get("description", ""), "parameters": d.get("input_schema", {"type": "object"})}}
        for d in turn.get("tool_definitions", [])
    ]
    messages: list[dict] = [{"role": "system", "content": turn["system"]}] if turn.get("system") else []
    for m in turn["messages"]:
        blocks = m["content"] if isinstance(m["content"], list) else [{"type": "text", "text": str(m["content"])}]
        if m["role"] == "assistant":
            text = "\n".join(b.get("text", "") for b in blocks if b.get("type") == "text").strip()
            calls = [
                {"id": b.get("id", f"call_{i}"), "type": "function", "function": {"name": b["name"], "arguments": json.dumps(b.get("input", {}), ensure_ascii=False)}}
                for i, b in enumerate(blocks)
                if b.get("type") == "tool_use"
            ]
            msg = {"role": "assistant", "content": text}
            if calls:
                msg["tool_calls"] = calls
            messages.append(msg)
        else:
            results = [b for b in blocks if b.get("type") == "tool_result"]
            texts = [b.get("text", "") for b in blocks if b.get("type") == "text"]
            files = [b for b in blocks if b.get("type") in ("image", "document")]
            for r in results:
                content = r["content"] if isinstance(r["content"], str) else "\n".join(x.get("text", "") for x in r["content"] if isinstance(x, dict))
                messages.append({"role": "tool", "tool_call_id": r.get("tool_use_id", ""), "content": content})
            user = "\n".join(texts).strip()
            if files:
                user += "\n" + "\n".join(f"[attached {f['type']}, {f.get('source', {}).get('media_type', '')}, left out of this sample]" for f in files)
            if user.strip():
                messages.append({"role": "user", "content": user.strip()})
    return messages, tools


def first_prompt(bundle: dict) -> tuple[list[dict], list[dict], str]:
    """The task a trajectory poses: system, tools and the first user message of its first turn,
    with the contributed final answer as the reference."""
    messages, tools = to_chat(bundle, 0)
    prompt = [m for m in messages if m["role"] in ("system", "user")][:2]
    answer = next((m["content"] for m in reversed(messages) if m["role"] == "assistant" and m["content"]), "")
    return prompt, tools, answer


# -- the screener's safety patterns, for the GRPO penalty ----------------------------------------------

DESTRUCTIVE = [
    re.compile(r"\brm\s+(?:-[a-zA-Z]*r[a-zA-Z]*f|-[a-zA-Z]*f[a-zA-Z]*r)\s+(?:/\*|/etc|/usr|/var|/home|/|~|\$HOME|\.\.)(?:[\s;&|/]|$)"),
    re.compile(r":\(\)\s*\{\s*:\s*\|\s*:\s*&\s*\}\s*;\s*:"),
    re.compile(r"\bmkfs(?:\.\w+)?\s+/dev/"),
    re.compile(r"\bdd\s+if=/dev/(?:zero|random|urandom)\s+of=/dev/"),
    re.compile(r"(?i)\b(?:curl|wget)\b[^\n]*\|\s*(?:sh|bash|zsh|python[23]?)\b"),
    re.compile(r"(?i)\b(?:cat|base64|xxd|tar|zip)\b[^\n]*(?:\.ssh/|\.aws/|\.env\b|id_rsa|/etc/shadow|\.netrc|\.gnupg)[^\n]*\|\s*(?:curl|wget|nc|ncat)"),
]


def safety_reward(completion_text: str) -> float:
    return -1.0 if any(p.search(completion_text) for p in DESTRUCTIVE) else 0.0


# -- stage 1: rejection-sampling fine-tuning -----------------------------------------------------------


def build_rft_examples(rows: list[dict], keep_at: float, boost_at: float) -> list[dict]:
    examples = []
    for row in rows:
        reward = trajectory_reward(row)
        if reward < keep_at:
            continue
        bundle = row["bundle"]
        for i in range(len(bundle["turns"])):
            messages, tools = to_chat(bundle, i)
            if not any(m["role"] == "assistant" for m in messages):
                continue
            ex = {"messages": messages, "tools": tools, "reward": reward, "receipt_id": row["receipt_id"]}
            examples.append(ex)
            if reward >= boost_at:
                examples.append(ex)  # the best count twice
    random.shuffle(examples)
    return examples


def run_rft(args):
    manifest, rows = load_release(Path(args.release), set(args.licenses.split(",")), args.only_sandbox, args.min_weight)
    examples = build_rft_examples(rows, args.keep_at, args.boost_at)
    print(f"release {manifest['version']}: {len(rows)} bundles usable, {len(examples)} training chats (reward >= {args.keep_at})")
    if args.dry_run:
        rewards = sorted(trajectory_reward(r) for r in rows)
        print("reward quantiles:", {q: rewards[round(q * (len(rewards) - 1))] for q in (0.1, 0.5, 0.9)} if rewards else {})
        if examples:
            print(json.dumps(examples[0]["messages"][:4], indent=2, ensure_ascii=False)[:2000])
        return
    import torch  # noqa: F401
    from datasets import Dataset
    from peft import LoraConfig
    from transformers import AutoTokenizer
    from trl import SFTConfig, SFTTrainer

    tokenizer = AutoTokenizer.from_pretrained(args.model)
    # Render with the model's own template so tool calls and results take the shape it was
    # pretrained on; SFTTrainer then trains on the text.
    texts = [tokenizer.apply_chat_template(e["messages"], tools=e["tools"] or None, tokenize=False) for e in examples]
    dataset = Dataset.from_dict({"text": texts})
    config = SFTConfig(
        output_dir=args.out,
        num_train_epochs=args.epochs,
        per_device_train_batch_size=args.batch,
        gradient_accumulation_steps=args.accumulate,
        learning_rate=args.lr,
        max_length=args.max_len,
        logging_steps=10,
        save_strategy="epoch",
        bf16=True,
        report_to="none",
    )
    trainer = SFTTrainer(
        model=args.model,
        args=config,
        train_dataset=dataset,
        processing_class=tokenizer,
        peft_config=LoraConfig(r=args.lora_r, lora_alpha=2 * args.lora_r, target_modules="all-linear", task_type="CAUSAL_LM"),
    )
    trainer.train()
    trainer.save_model(args.out)
    print(f"saved {args.out}")


# -- stage 2: GRPO ---------------------------------------------------------------------------------------


def parse_tool_calls(text: str) -> list[dict]:
    """Qwen writes a tool call as `<tool_call>{"name": ..., "arguments": {...}}</tool_call>`."""
    calls = []
    for m in re.finditer(r"<tool_call>\s*(\{.*?\})\s*</tool_call>", text, re.S):
        try:
            calls.append(json.loads(m.group(1)))
        except json.JSONDecodeError:
            calls.append({"name": None})
    return calls


def format_reward_fn(completions, tools, **kwargs):
    """Well-formed tool calls naming a defined tool with a JSON object as arguments, or a plain
    answer; a malformed call or an unknown tool is penalised."""
    out = []
    for text, tool_list in zip(completions, tools):
        text = text if isinstance(text, str) else text[0]["content"]
        names = {t["function"]["name"] for t in tool_list}
        calls = parse_tool_calls(text)
        if "<tool_call>" in text and not calls:
            out.append(-1.0)
            continue
        score = 0.0
        for c in calls:
            if c.get("name") in names and isinstance(c.get("arguments"), dict):
                score += 0.5
            else:
                score -= 1.0
        out.append(max(-1.0, min(1.0, score)))
    return out


def safety_reward_fn(completions, **kwargs):
    return [safety_reward(c if isinstance(c, str) else c[0]["content"]) for c in completions]


class Judge:
    """An OpenAI-compatible endpoint asked to compare a first move with the contributed outcome."""

    RUBRIC = (
        "You compare an agent's first move on a task with how a successful run of the same task "
        "ended. The move may be a tool call or an answer. Score 0 to 1 how likely this move leads "
        "to that outcome: a sensible first tool call toward it is 0.6-0.9, a complete correct answer "
        "1.0, an irrelevant or harmful move 0. Reply with one JSON object: {\"score\": number}."
    )

    def __init__(self, url: str, model: str, key: str | None):
        import urllib.request

        self.url = url.rstrip("/") + "/chat/completions"
        self.model = model
        self.key = key
        self.request = urllib.request

    def score(self, task: str, reference: str, move: str) -> float:
        body = json.dumps({
            "model": self.model,
            "temperature": 0,
            "max_tokens": 50,
            "messages": [
                {"role": "system", "content": self.RUBRIC},
                {"role": "user", "content": f"TASK:\n{task[:3000]}\n\nSUCCESSFUL OUTCOME:\n{reference[:2000]}\n\nFIRST MOVE:\n{move[:2000]}"},
            ],
        }).encode()
        headers = {"content-type": "application/json"}
        if self.key:
            headers["authorization"] = f"Bearer {self.key}"
        try:
            with self.request.urlopen(self.request.Request(self.url, data=body, headers=headers), timeout=60) as resp:
                text = json.loads(resp.read())["choices"][0]["message"]["content"]
            m = re.search(r"\{.*\}", text, re.S)
            return max(0.0, min(1.0, float(json.loads(m.group(0))["score"]))) if m else 0.0
        except Exception as e:  # a judge that is down scores nothing, and says so once
            print(f"judge failed: {e}", file=sys.stderr)
            return 0.0


def run_grpo(args):
    manifest, rows = load_release(Path(args.release), set(args.licenses.split(",")), args.only_sandbox, args.min_weight)
    tasks = []
    for row in rows:
        if trajectory_reward(row) < args.keep_at:
            continue
        prompt, tools, answer = first_prompt(row["bundle"])
        if len(prompt) < 2 or not answer:
            continue
        tasks.append({"prompt": prompt, "tools": tools, "reference": answer, "task_text": prompt[-1]["content"]})
    print(f"release {manifest['version']}: {len(tasks)} tasks from successful trajectories")
    if args.dry_run:
        if tasks:
            print(json.dumps({k: tasks[0][k] for k in ("prompt", "reference")}, indent=2, ensure_ascii=False)[:1500])
            print("format reward of a good call:", format_reward_fn(['<tool_call>{"name": "bash", "arguments": {"command": "ls"}}</tool_call>'], [tasks[0]["tools"]]))
            print("safety reward of a bad call:", safety_reward_fn(['<tool_call>{"name": "bash", "arguments": {"command": "rm -rf / --no-preserve-root"}}</tool_call>']))
        return
    from datasets import Dataset
    from peft import LoraConfig
    from trl import GRPOConfig, GRPOTrainer

    judge = None
    if os.environ.get("CONTRIB_JUDGE_URL"):
        judge = Judge(os.environ["CONTRIB_JUDGE_URL"], os.environ.get("CONTRIB_JUDGE_MODEL", "judge"), os.environ.get("CONTRIB_JUDGE_API_KEY"))

    def judge_reward_fn(completions, task_text, reference, **kwargs):
        if judge is None:
            return [0.0] * len(completions)
        return [judge.score(t, r, c if isinstance(c, str) else c[0]["content"]) for c, t, r in zip(completions, task_text, reference)]

    dataset = Dataset.from_list(tasks)
    config = GRPOConfig(
        output_dir=args.out,
        num_train_epochs=args.epochs,
        per_device_train_batch_size=args.batch,
        gradient_accumulation_steps=args.accumulate,
        learning_rate=args.lr,
        num_generations=args.generations,
        max_completion_length=args.max_completion,
        max_prompt_length=args.max_len,
        logging_steps=5,
        save_strategy="epoch",
        bf16=True,
        report_to="none",
        reward_weights=[1.0, 2.0, 2.0],
    )
    trainer = GRPOTrainer(
        model=args.model,
        reward_funcs=[format_reward_fn, judge_reward_fn, safety_reward_fn],
        args=config,
        train_dataset=dataset,
        peft_config=LoraConfig(r=args.lora_r, lora_alpha=2 * args.lora_r, target_modules="all-linear", task_type="CAUSAL_LM"),
    )
    trainer.train()
    trainer.save_model(args.out)
    print(f"saved {args.out}")


# -- main --------------------------------------------------------------------------------------------------


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="stage", required=True)
    for name in ("rft", "grpo"):
        s = sub.add_parser(name)
        s.add_argument("--release", required=True, help="a release directory holding manifest.json")
        s.add_argument("--model", default="Qwen/Qwen2.5-1.5B-Instruct")
        s.add_argument("--out", default=f"ckpt/{name}")
        s.add_argument("--licenses", default="cc0,cc-by-4.0")
        s.add_argument("--only-sandbox", action="store_true", help="only sandbox_only trajectories (replayable in cedar-bench)")
        s.add_argument("--min-weight", type=float, default=0.3, help="skip bundles whose contributor's labels are trusted less than this")
        s.add_argument("--keep-at", type=float, default=0.6, help="trajectory reward needed to train on it")
        s.add_argument("--boost-at", type=float, default=0.9, help="trajectory reward from which a sample counts twice (rft)")
        s.add_argument("--epochs", type=float, default=1.0)
        s.add_argument("--batch", type=int, default=2)
        s.add_argument("--accumulate", type=int, default=8)
        s.add_argument("--lr", type=float, default=1e-5)
        s.add_argument("--max-len", type=int, default=4096)
        s.add_argument("--lora-r", type=int, default=16)
        s.add_argument("--dry-run", action="store_true", help="load and convert, train nothing, need no GPU")
        if name == "grpo":
            s.add_argument("--generations", type=int, default=8)
            s.add_argument("--max-completion", type=int, default=512)
    args = p.parse_args()
    random.seed(0)
    (run_rft if args.stage == "rft" else run_grpo)(args)


if __name__ == "__main__":
    main()
