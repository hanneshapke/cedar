#!/usr/bin/env python3
"""x_ai_trends: a community cedar tool. Reads its input as JSON on stdin and prints the result.

Samples recent AI posts on X (Twitter) through the X API v2 recent search and works out what is
trending: the models, companies and products people talk about, hashtags, recurring phrases, the
links being shared, and the most engaged posts, each topic with example posts. With compare (the
default) it also samples the window before and marks the topics that are rising or new. An
optional X trends lookup (by WOEID) adds the AI-related entries of X's own trend list.

The sample is spread over the window: it is cut into slices and each slice is searched with
X's relevancy order, so one busy hour doesn't fill the whole sample. Every post read counts
against the X API plan, so max_posts bounds a call.

Needs an X API bearer token (X_BEARER_TOKEN) from an app on a plan that includes recent search.

Exit non-zero with a message on stderr to report an error; the model sees it as the tool's
error and can try again differently.
"""
import json
import math
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from collections import Counter, defaultdict
from datetime import datetime, timedelta, timezone
from pathlib import Path

API = "https://api.x.com/2"
USER_AGENT = "cedar-x_ai_trends/1.0 (+https://github.com/hanneshapke/cedar/tree/main/registry/tools/x_ai_trends)"
# The terms the default search looks for. They match nearly every post, so they are never topics.
AI_TERMS = ['AI', 'LLM', 'LLMs', '"artificial intelligence"', '"machine learning"', '"AI agents"',
            'OpenAI', 'Anthropic', 'ChatGPT', 'Claude', 'Gemini', 'DeepSeek', '"generative AI"']
QUERY_WORDS = {"ai", "llm", "llms", "artificial", "intelligence", "artificial intelligence",
               "machine", "learning", "machine learning", "generative", "generative ai", "genai"}
MAX_QUERY = 512  # the recent search limit on the self-serve plans
SLICES = 4
MAX_PER_AUTHOR = 3
MAX_TOPICS, MAX_EXAMPLES, MAX_POSTS_SHOWN, MAX_LINKS, MAX_TEXT = 25, 2, 10, 10, 280
RATE_WAIT = 20  # seconds the tool waits for a rate limit to reset before reporting what it has

# Companies, labs and products, by how they are written; matched case-insensitively as words.
NAMES = [
    "OpenAI", "Anthropic", "Google DeepMind", "DeepMind", "Google", "Meta", "Microsoft", "Nvidia",
    "Apple", "Amazon", "AWS", "xAI", "Mistral", "Hugging Face", "Perplexity", "Cohere", "Alibaba",
    "Moonshot", "Zhipu", "Baidu", "Tencent", "ByteDance", "Samsung", "Intel", "AMD", "TSMC",
    "Cerebras", "Groq", "Databricks", "Snowflake", "Scale AI", "Midjourney", "Runway", "ElevenLabs",
    "Stability AI", "Character AI", "Safe Superintelligence", "Thinking Machines", "Figure",
    "Tesla", "Waymo", "Cursor", "Windsurf", "Replit", "Lovable", "GitHub", "Vercel", "LangChain",
    "LlamaIndex", "Ollama", "vLLM", "PyTorch", "JAX", "ChatGPT", "Claude", "Claude Code", "Codex",
    "Gemini", "Copilot", "Grok", "Llama", "Qwen", "Kimi", "Sora", "Veo", "Nano Banana", "NotebookLM",
    "Perplexity Comet", "Atlas", "Optimus", "MCP", "AGI", "ASI", "RAG", "RLHF", "GPU", "GPUs", "TPU",
    "EU AI Act", "Sam Altman", "Dario Amodei", "Demis Hassabis", "Elon Musk", "Jensen Huang",
    "Mark Zuckerberg", "Satya Nadella", "Sundar Pichai", "Andrej Karpathy", "Yann LeCun",
    "Ilya Sutskever", "Geoffrey Hinton", "Mira Murati",
]
# Model names with a version: GPT-5.1, Claude Opus 4.5, Gemini 3 Pro, Llama 4, Qwen3, DeepSeek-V4, o3.
MODEL = re.compile(
    r"\b(?:(?:GPT|Claude|Gemini|Gemma|Llama|Qwen|DeepSeek|Grok|Mistral|Mixtral|Magistral|Phi|Sora|Veo|"
    r"Kimi|GLM|Nova|Codex|Imagen|Flux|MiniMax|Hunyuan|Ernie|Olmo|Nemotron)"
    r"(?:[- ]?(?:Opus|Sonnet|Haiku|Pro|Flash|Ultra|Nano|Mini|Large|Medium|Small|Coder|R|V|K|M))?"
    r"[- ]?\d+(?:\.\d+)?(?:[- ]?(?:Pro|Flash|Lite|Mini|Max|Turbo|Opus|Sonnet|Haiku|Ultra|Thinking|"
    r"Instruct|Codex|Preview))?|o[134](?:-(?:mini|pro))?)\b", re.I)
URL = re.compile(r"https?://\S+")
MENTION = re.compile(r"(?<!\w)@\w{1,15}")
HASHTAG = re.compile(r"(?<!\w)#(\w{2,50})")
WORD = re.compile(r"[A-Za-z][A-Za-z0-9'+.-]*[A-Za-z0-9+]|[A-Za-z]")
CRYPTO = re.compile(r"\b(airdrop|presale|memecoin|pump\.fun|100x|1000x|token launch|whitelist|"
                    r"\$[A-Z]{2,10}\b|solana|ethereum|bitcoin|crypto|web3|nft|giveaway)", re.I)
SHARED_HOSTS = ("x.com", "twitter.com", "t.co", "pic.x.com", "pic.twitter.com")
STOPWORDS = set("""
a about above after again against all almost also am among an and any are aren't as at be because
been before being below between both but by can can't cannot could couldn't did didn't do does
doesn't doing don't down during each even ever every few for from further get gets getting got had
hadn't has hasn't have haven't having he he's her here here's hers herself him himself his how
how's i i'd i'll i'm i've if in into is isn't it it's its itself just let's like lot lots make
makes made many may me might more most much must mustn't my myself new no nor not now of off on
once one only or other ought our ours ourselves out over own really right same say says said see
she she'd she'll she's should shouldn't so some still such take than that that's the their theirs
them themselves then there there's these they they'd they'll they're they've thing things think
this those though through thus to today too under until up upon us use used using very via want
was wasn't way we we'd we'll we're we've well were weren't what what's when when's where where's
which while who who's whom why why's will with without won't would wouldn't yet you you'd you'll
you're you've your yours yourself yourselves yes yeah ok okay lol omg wow guys everyone anyone
someone something anything nothing everything people time year years day days week going go gonna
need needs know knows look looks good great best better big first last next back already actually
literally probably maybe pretty kind sure keep let lets thanks thank amp via rt
""".split())

# Text the user sees in the charts, by CEDAR_LANGUAGE. The report itself stays English for the model.
STRINGS = {
    "en": {"topics_title": "AI topics on X, last {h} h", "authors": "accounts",
           "rising_title": "Rising AI topics on X", "now": "last {h} h", "before": "{h} h before",
           "share": "% of accounts"},
    "de": {"topics_title": "KI-Themen auf X, letzte {h} Std.", "authors": "Konten",
           "rising_title": "Aufsteigende KI-Themen auf X", "now": "letzte {h} Std.",
           "before": "{h} Std. davor", "share": "% der Konten"},
    "fr": {"topics_title": "Sujets IA sur X, dernières {h} h", "authors": "comptes",
           "rising_title": "Sujets IA en hausse sur X", "now": "dernières {h} h", "before": "{h} h avant",
           "share": "% des comptes"},
    "es": {"topics_title": "Temas de IA en X, últimas {h} h", "authors": "cuentas",
           "rising_title": "Temas de IA en alza en X", "now": "últimas {h} h", "before": "{h} h antes",
           "share": "% de cuentas"},
}


def language():
    """The user's language from CEDAR_LANGUAGE (older servers don't set it: English)."""
    lang = (os.environ.get("CEDAR_LANGUAGE") or "").strip().lower().split("-")[0]
    return lang if lang in STRINGS else "en"


def tr(key, **values):
    return STRINGS[language()].get(key, STRINGS["en"][key]).format(**values)


def whole(value, default, low, high):
    try:
        return max(low, min(high, int(value)))
    except (TypeError, ValueError):
        return default


# --- the X API -------------------------------------------------------------------------------

class RateLimited(Exception):
    pass


def fetch(path, params, token):
    """GET an X API v2 path; waits out a short rate limit once, else raises RateLimited."""
    url = f"{API}{path}?{urllib.parse.urlencode(params)}"
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT, "Authorization": f"Bearer {token}"})
    for attempt in range(2):
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                return json.loads(resp.read())
        except urllib.error.HTTPError as e:
            if e.code == 429:
                reset = e.headers.get("x-rate-limit-reset")
                wait = int(reset) - int(time.time()) + 1 if reset and reset.isdigit() else RATE_WAIT + 1
                if attempt == 0 and 0 <= wait <= RATE_WAIT:
                    time.sleep(wait)
                    continue
                raise RateLimited(max(wait, 0))
            raise
    raise RateLimited(RATE_WAIT)


def api_error(e):
    """An HTTPError from X as a message the model can act on."""
    try:
        body = json.loads(e.read() or b"{}")
    except ValueError:
        body = {}
    detail = body.get("detail") or body.get("title") or "; ".join(
        x.get("message", "") for x in body.get("errors", []) if isinstance(x, dict)) or e.reason
    if e.code == 401:
        return f"X rejected the bearer token ({detail}); check X_BEARER_TOKEN in the tool's settings"
    if e.code == 403:
        return (f"X refused the request ({detail}); the app's X API plan may not include recent search, "
                "or its credits are used up")
    if e.code == 400:
        return f"X couldn't read the search ({detail}); simplify the query or topics"
    return f"X answered {e.code} ({detail}); try again in a little while"


def iso(t):
    return t.strftime("%Y-%m-%dT%H:%M:%SZ")


def search(query, start, end, wanted, token, sort):
    """Up to `wanted` posts between start and end, spread over SLICES slices of the window.

    Returns (posts, users by id, a note when the sample was cut short)."""
    posts, users, note = [], {}, None
    step = (end - start) / SLICES
    for i in range(SLICES):
        s_start, s_end = start + step * i, start + step * (i + 1)
        quota = wanted // SLICES + (1 if i < wanted % SLICES else 0)
        token_next, got = None, 0
        while got < quota:
            params = {"query": query, "start_time": iso(s_start), "end_time": iso(s_end),
                      "max_results": max(10, min(100, quota - got)), "sort_order": sort,
                      "tweet.fields": "created_at,public_metrics,entities,lang,author_id",
                      "expansions": "author_id", "user.fields": "username,name,verified,public_metrics"}
            if token_next:
                params["next_token"] = token_next
            try:
                body = fetch("/tweets/search/recent", params, token)
            except RateLimited as e:
                return posts, users, f"X's rate limit stopped the sample early (it resets in about {e.args[0]}s)"
            for u in body.get("includes", {}).get("users", []):
                users[u["id"]] = u
            batch = body.get("data") or []
            posts.extend(batch[:quota - got])
            got += len(batch)
            token_next = body.get("meta", {}).get("next_token")
            if not batch or not token_next:
                break
    return posts, users, note


def x_trends(woeid, token):
    """X's own trend list for a place (1 is worldwide): ([(name, count)], a note or None)."""
    try:
        body = fetch(f"/trends/by/woeid/{woeid}", {"max_trends": 50}, token)
    except RateLimited:
        return [], "X's trends were rate limited"
    except urllib.error.HTTPError as e:
        return [], f"X's trends aren't available ({api_error(e)})"
    return [(t.get("trend_name", ""), t.get("tweet_count")) for t in body.get("data") or []], None


# --- the query -------------------------------------------------------------------------------

def phrase(term):
    term = re.sub(r'["()]', "", str(term)).strip()
    return f'"{term}"' if " " in term or "-" in term else term


def build_query(args):
    raw = str(args.get("query") or "").strip()
    if raw:
        q = raw
    else:
        topics = args.get("topics")
        topics = topics if isinstance(topics, list) else [t for t in re.split(r",", str(topics or ""))]
        topics = [phrase(t) for t in topics if str(t).strip()]
        q = "(" + " OR ".join(topics or AI_TERMS) + ")"
    lang = str(args.get("language") or "en").strip().lower()
    filters = ["-is:retweet"]
    if not args.get("include_replies"):
        filters.append("-is:reply")
    if lang and lang != "any":
        if not re.fullmatch(r"[a-z]{2,3}", lang):
            sys.exit(f"language is a code such as en, de or ja (or any), not {lang!r}")
        filters.append(f"lang:{lang}")
    if args.get("verified_only"):
        filters.append("is:verified")
    if args.get("exclude_crypto", True):
        filters.append("-has:cashtags")
    # A raw query that already says what it wants keeps its own operators.
    q = f"{q} " + " ".join(f for f in filters if f not in q and not (f.startswith("lang:") and "lang:" in q))
    if len(q) > MAX_QUERY:
        sys.exit(f"the search is {len(q)} characters, X allows {MAX_QUERY}; name fewer topics")
    return q


# --- the analysis ----------------------------------------------------------------------------

def norm(text):
    return re.sub(r"[\s_-]+", "-", text.lower()).strip("-")


NAME_PATTERNS = [(norm(n), re.compile(r"(?<![\w-])" + re.escape(n).replace(r"\ ", r"[\s-]") + r"(?![\w-])", re.I))
                 for n in sorted(NAMES, key=len, reverse=True)]


def engagement(m):
    return (m.get("like_count", 0) + 2 * m.get("retweet_count", 0) + 2 * m.get("quote_count", 0)
            + m.get("reply_count", 0) + m.get("bookmark_count", 0))


def clean(posts, users, exclude_crypto=True):
    """Posts as dicts, without crypto spam, near-duplicates, and more than MAX_PER_AUTHOR per account."""
    out, seen, per_author = [], set(), Counter()
    rows = []
    for p in posts:
        text = p.get("text") or ""
        if exclude_crypto and len(CRYPTO.findall(text)) >= 2:
            continue
        key = re.sub(r"\W+", " ", URL.sub("", MENTION.sub("", text))).strip().lower()[:120]
        if not key or key in seen:
            continue
        seen.add(key)
        u = users.get(p.get("author_id"), {})
        rows.append({
            "id": p["id"], "text": text, "author_id": p.get("author_id"),
            "username": u.get("username") or p.get("author_id"), "name": u.get("name") or "",
            "verified": bool(u.get("verified")), "followers": u.get("public_metrics", {}).get("followers_count", 0),
            "created_at": p.get("created_at", ""), "metrics": p.get("public_metrics", {}),
            "engagement": engagement(p.get("public_metrics", {})), "entities": p.get("entities", {}),
        })
    for r in sorted(rows, key=lambda r: -r["engagement"]):
        if per_author[r["author_id"]] < MAX_PER_AUTHOR:
            per_author[r["author_id"]] += 1
            out.append(r)
    return out


def candidates(post):
    """The topic keys a post mentions: {key: (kind, the form written)}."""
    text = post["text"]
    found = {}
    for tag in post["entities"].get("hashtags") or [{"tag": t} for t in HASHTAG.findall(text)]:
        t = tag.get("tag", "")
        if t and t.lower() not in QUERY_WORDS:
            found["#" + t.lower()] = ("hashtag", "#" + t)
    for m in post["entities"].get("mentions") or []:
        name = m.get("username", "")
        if name:
            found["@" + name.lower()] = ("account", "@" + name)
    body = MENTION.sub(" ", URL.sub(" ", text))
    taken = []
    for m in MODEL.finditer(body):
        found[norm(m.group()).replace("-", "")] = ("model", m.group())
        taken.append(m.span())
    for key, pattern in NAME_PATTERNS:
        for m in pattern.finditer(body):
            if any(a <= m.start() < b for a, b in taken):
                continue
            found.setdefault(key, ("name", m.group()))
            taken.append(m.span())
    # Phrases: two or three words in a row, without stop words at either end, within a sentence.
    rest = list(body)
    for a, b in taken:
        rest[a:b] = "|" * (b - a)
    for sentence in re.split(r"[.!?;:|\n()\"“”]+", "".join(rest).replace("#", "")):
        words = WORD.findall(sentence)
        low = [w.lower().strip("'.") for w in words]
        for n in (2, 3):
            for i in range(len(words) - n + 1):
                gram = low[i:i + n]
                if gram[0] in STOPWORDS or gram[-1] in STOPWORDS or len(gram[0]) < 2 or len(gram[-1]) < 2:
                    continue
                if any(g.isdigit() for g in gram) or gram[0] in QUERY_WORDS or gram[-1] in QUERY_WORDS:
                    continue
                found.setdefault(" ".join(gram), ("phrase", " ".join(words[i:i + n])))
        # Acronyms and capitalized words not at the start of a sentence: RAG, MCP, Nvidia.
        for i, w in enumerate(words):
            if (w.isupper() and 2 <= len(w) <= 6) or (i > 0 and w[0].isupper() and len(w) > 3 and not w.isupper()):
                if low[i] not in STOPWORDS and low[i] not in QUERY_WORDS:
                    found.setdefault(low[i], ("word", w))
    return found


def topics(posts):
    """Every topic key with its kind, the form most often written, its posts and its accounts."""
    out = defaultdict(lambda: {"forms": Counter(), "kind": None, "posts": [], "authors": set(), "engagement": 0})
    for p in posts:
        for key, (kind, form) in candidates(p).items():
            t = out[key]
            t["forms"][form] += 1
            t["kind"] = t["kind"] or kind
            t["posts"].append(p)
            t["authors"].add(p["author_id"])
            t["engagement"] += p["engagement"]
    return out


# Which member names a story: a model or company says more than the hashtag or phrase around it.
KIND_ORDER = {"model": 0, "name": 1, "hashtag": 2, "word": 3, "phrase": 4}


def label(t):
    """The form a topic is most often written in, capitals winning a tie (Claude Opus 5, not claude opus 5)."""
    return max(t["forms"].items(), key=lambda kv: (kv[1], sum(c.isupper() for c in kv[0])))[0]


def rank(now, before, n_now, n_before):
    """The trending stories of `now`, scored by accounts and engagement, with their growth.

    Topics whose posts are mostly the same (a launch, its company and its hashtag) are one story."""
    floor = max(3, math.ceil(n_now * 0.005))
    keep = sorted(((k, t) for k, t in now.items() if len(t["authors"]) >= floor and t["kind"] != "account"),
                  key=lambda kt: -len(kt[1]["authors"]))
    stories = []
    for key, t in keep:
        ids = {p["id"] for p in t["posts"]}
        for s in stories:
            shared = len(ids & s["ids"])
            if shared >= 0.6 * min(len(ids), len(s["ids"])) and shared >= 0.4 * len(ids | s["ids"]):
                s["members"].append((key, t))
                s["ids"] |= ids
                break
        else:
            stories.append({"members": [(key, t)], "ids": ids})
    rows = []
    for s in stories:
        members = sorted(s["members"], key=lambda kt: (KIND_ORDER[kt[1]["kind"]], -len(kt[1]["authors"])))
        lead = members[0][1]
        posts = {p["id"]: p for _, t in members for p in t["posts"]}.values()
        authors = set().union(*(t["authors"] for _, t in members))
        eng = sum(p["engagement"] for p in posts)
        share = len(authors) / max(n_now, 1)
        row = {"label": label(lead), "kind": lead["kind"],
               "related": [label(t) for _, t in members[1:6]],
               "accounts": len(authors), "posts": len(posts), "engagement": eng, "share": share,
               "score": len(authors) * (1 + math.log10(1 + eng / len(authors))), "growth": None,
               "status": None, "examples": sorted(posts, key=lambda p: -p["engagement"])[:MAX_EXAMPLES]}
        if n_before:
            prev = set().union(*(before[k]["authors"] for k, _ in members if k in before))
            row["share_before"] = len(prev) / n_before
            smooth = 1 / max(n_before, n_now)
            row["growth"] = (share + smooth) / (row["share_before"] + smooth)
            if not prev and len(authors) > floor:
                row["status"] = "new"
            elif row["growth"] >= 2:
                row["status"] = "rising"
            elif row["growth"] <= 0.5:
                row["status"] = "cooling"
        rows.append(row)
    rows.sort(key=lambda r: -r["score"])
    return rows


def accounts(posts):
    """The accounts whose posts drew the most engagement."""
    by = defaultdict(lambda: {"engagement": 0, "posts": 0})
    for p in posts:
        a = by[p["username"]]
        a.update(name=p["name"], followers=p["followers"], verified=p["verified"])
        a["engagement"] += p["engagement"]
        a["posts"] += 1
    return sorted(by.items(), key=lambda kv: -kv[1]["engagement"])[:MAX_LINKS]


def links(posts):
    """Links outside X that several accounts shared, most shared first."""
    by = defaultdict(lambda: {"authors": set(), "title": "", "engagement": 0})
    for p in posts:
        for u in p["entities"].get("urls") or []:
            url = u.get("unwound_url") or u.get("expanded_url") or ""
            host = urllib.parse.urlsplit(url).hostname or ""
            if not url or host.removeprefix("www.") in SHARED_HOSTS:
                continue
            url = re.sub(r"[?#].*$", "", url) if "youtube.com" not in host else url
            entry = by[url]
            entry["authors"].add(p["author_id"])
            entry["title"] = entry["title"] or u.get("title") or ""
            entry["engagement"] += p["engagement"]
    rows = [(url, e) for url, e in by.items() if len(e["authors"]) >= 2]
    return sorted(rows, key=lambda kv: (-len(kv[1]["authors"]), -kv[1]["engagement"]))[:MAX_LINKS]


# --- the report ------------------------------------------------------------------------------

def link(p):
    return f"https://x.com/{p['username']}/status/{p['id']}"


def short(text):
    text = re.sub(r"\s+", " ", text).strip()
    return text if len(text) <= MAX_TEXT else text[:MAX_TEXT].rsplit(" ", 1)[0] + " [...]"


def stats(p):
    m = p["metrics"]
    return (f"{m.get('like_count', 0)} likes, {m.get('retweet_count', 0)} reposts, "
            f"{m.get('reply_count', 0)} replies")


def report(ctx, rows, posts, trend_list, notes):
    h = ctx["hours"]
    out = [f"AI on X, {ctx['start']:%Y-%m-%d %H:%M} to {ctx['end']:%Y-%m-%d %H:%M} UTC (last {h} h).",
           f"Search: {ctx['query']}",
           f"Sample: {len(posts)} posts from {len({p['author_id'] for p in posts})} accounts "
           f"(of {ctx['read']} read; reposts, spam, duplicates and more than {MAX_PER_AUTHOR} posts per "
           f"account left out), spread over the window and ordered by X's relevancy."]
    if ctx.get("n_before"):
        out.append(f"Compared with {ctx['n_before']} posts from the {h} h before.")
    out += [f"Note: {n}" for n in notes]
    if not posts:
        out += ["", "No posts matched. Try broader topics, language=any, or a longer window."]
        return "\n".join(out)

    out += ["", "TRENDING TOPICS (topics that share most of their posts are one story, the others listed after 'with'; "
                "accounts = distinct accounts that posted about it; share = of all accounts in the sample"
                + ("; growth = share now / share before" if ctx.get("n_before") else "") + "):"]
    for n, r in enumerate(rows[:MAX_TOPICS], 1):
        line = f"[{n}] {r['label']} ({r['kind']}): {r['accounts']} accounts, {r['posts']} posts, " \
               f"{r['engagement']} engagement, share {r['share']:.1%}"
        if r["growth"] is not None:
            line += f", growth {r['growth']:.1f}x" + (f" [{r['status'].upper()}]" if r["status"] else "")
        out.append(line)
        if r["related"]:
            out.append("    with: " + ", ".join(r["related"]))
        for p in r["examples"]:
            out.append(f"    - @{p['username']}: \"{short(p['text'])}\" ({stats(p)}) {link(p)}")

    if ctx.get("n_before"):
        moving = [r for r in rows if r["status"] in ("new", "rising")]
        moving.sort(key=lambda r: -(r["growth"] or 0) * math.log(1 + r["accounts"]))
        out += ["", "RISING OR NEW (compared with the window before):"]
        out += [f"- {r['label']}: {r['share_before']:.1%} -> {r['share']:.1%} of accounts ("
                + ("new" if r["status"] == "new" else f"{r['growth']:.1f}x") + ")"
                for r in moving[:12]] or ["- nothing grew by 2x or more"]
        cooling = [r["label"] for r in rows if r["status"] == "cooling"][:8]
        if cooling:
            out.append("Cooling off: " + ", ".join(cooling) + ".")

    if trend_list is not None:
        out += ["", f"X TRENDS (WOEID {ctx['woeid']}), AI-related entries:"]
        out += [f"- {name}" + (f" ({count} posts)" if count else "") for name, count in trend_list] or \
               ["- none of X's current trends look AI-related"]

    out += ["", "MOST ENGAGED POSTS:"]
    for p in sorted(posts, key=lambda p: -p["engagement"])[:MAX_POSTS_SHOWN]:
        who = f"@{p['username']}" + (f" ({p['name']}, {p['followers']} followers)" if p["name"] else "")
        out.append(f"- {who}, {p['created_at'][:16].replace('T', ' ')}: \"{short(p['text'])}\" ({stats(p)}) {link(p)}")

    shared = links(posts)
    if shared:
        out += ["", "MOST SHARED LINKS:"]
        out += [f"- {url}" + (f" \"{e['title']}\"" if e["title"] else "") + f" ({len(e['authors'])} accounts)"
                for url, e in shared]

    out += ["", "ACCOUNTS WITH THE MOST ENGAGEMENT:"]
    out += [f"- @{name}" + (f" ({a['name']})" if a["name"] else "") + f": {a['engagement']} engagement over "
            f"{a['posts']} posts, {a['followers']} followers" for name, a in accounts(posts)]
    return "\n".join(out)


def digest(ctx, rows, posts):
    """The topics and posts as Markdown, saved to the workspace for the user to keep."""
    h = ctx["hours"]
    md = [f"# AI on X: {ctx['end']:%Y-%m-%d}", "",
          f"{ctx['start']:%Y-%m-%d %H:%M} to {ctx['end']:%Y-%m-%d %H:%M} UTC, {len(posts)} posts sampled.", "",
          "## Trending topics", "", "| # | Topic | Kind | Accounts | Posts | Engagement |" +
          (" Growth |" if ctx.get("n_before") else ""),
          "|---|---|---|---|---|---|" + ("---|" if ctx.get("n_before") else "")]
    for n, r in enumerate(rows[:MAX_TOPICS], 1):
        cell = (r["label"] + (f" (with {', '.join(r['related'][:3])})" if r["related"] else "")).replace("|", "\\|")
        line = f"| {n} | {cell} | {r['kind']} | {r['accounts']} | {r['posts']} | {r['engagement']} |"
        if ctx.get("n_before"):
            line += (" new |" if r["status"] == "new" else f" {r['growth']:.1f}x |")
        md.append(line)
    md += ["", "## Most engaged posts", ""]
    for p in sorted(posts, key=lambda p: -p["engagement"])[:MAX_POSTS_SHOWN]:
        md.append(f"- [@{p['username']}]({link(p)}): {short(p['text'])} ({stats(p)})")
    shared = links(posts)
    if shared:
        md += ["", "## Most shared links", ""]
        md += [f"- <{url}>" + (f" {e['title']}" if e["title"] else "") + f" ({len(e['authors'])} accounts)"
               for url, e in shared]
    md += ["", f"_Search: `{ctx['query']}`, last {h} h. A sample of posts, not all of X._", ""]
    path = Path("ai_trends") / f"ai-trends-{ctx['end']:%Y-%m-%d-%H%M}.md"
    path.parent.mkdir(exist_ok=True)
    path.write_text("\n".join(md), encoding="utf-8")
    return path


def charts(ctx, rows):
    out = []
    top = rows[:12]
    if top:
        out.append({"kind": "barh", "title": tr("topics_title", h=ctx["hours"]),
                    "labels": [r["label"] for r in top], "series": [{"name": tr("authors"),
                                                                     "values": [r["accounts"] for r in top]}],
                    "x_label": tr("authors")})
    moving = [r for r in rows if r["status"] in ("new", "rising")][:10]
    if moving:
        out.append({"kind": "barh", "title": tr("rising_title"), "labels": [r["label"] for r in moving],
                    "series": [{"name": tr("before", h=ctx["hours"]),
                                "values": [round(r["share_before"] * 100, 1) for r in moving]},
                               {"name": tr("now", h=ctx["hours"]),
                                "values": [round(r["share"] * 100, 1) for r in moving]}],
                    "x_label": tr("share")})
    return ["[chart] " + json.dumps(c, separators=(",", ":"), ensure_ascii=False) for c in out]


def related(trend_list):
    keys = [k for k, _ in NAME_PATTERNS] + [w.strip('"').lower() for w in AI_TERMS]
    hits = []
    for name, count in trend_list:
        low = name.lower().lstrip("#")
        if MODEL.search(name) or any(re.search(r"(?<![a-z])" + re.escape(k.replace("-", " ")) + r"(?![a-z])",
                                               low.replace("-", " ")) for k in keys):
            hits.append((name, count))
    return hits


def main():
    args = json.load(sys.stdin) or {}
    token = (os.environ.get("X_BEARER_TOKEN") or "").strip()
    if not token:
        sys.exit("X_BEARER_TOKEN isn't set: an admin saves an X API bearer token in this tool's settings")
    hours = whole(args.get("hours"), 24, 1, 72)
    wanted = whole(args.get("max_posts"), 400, 40, 1000)
    compare = bool(args.get("compare", True))
    sort = "recency" if args.get("sort") == "recency" else "relevancy"
    query = build_query(args)

    end = datetime.now(timezone.utc).replace(microsecond=0) - timedelta(seconds=30)
    start = end - timedelta(hours=hours)
    ctx = {"hours": hours, "start": start, "end": end, "query": query}
    notes = []
    try:
        raw, users, note = search(query, start, end, wanted, token, sort)
        if note and not raw:
            sys.exit(f"{note}; try again then")
        notes += [note] if note else []
        before_raw, before_users = [], {}
        # Recent search reaches back seven days; the window before has to fit in it too.
        if compare and raw and start - timedelta(hours=hours) > end - timedelta(days=7):
            before_raw, before_users, note = search(query, start - timedelta(hours=hours), start,
                                                    max(40, wanted // 2), token, sort)
            notes += [f"{note} (window before)"] if note else []
        trend_list = None
        if args.get("woeid") is not None:
            ctx["woeid"] = whole(args.get("woeid"), 1, 1, 2**31)
            all_trends, note = x_trends(ctx["woeid"], token)
            if note:
                notes.append(note)
            else:
                trend_list = related(all_trends)
    except urllib.error.HTTPError as e:
        sys.exit(api_error(e))
    except (urllib.error.URLError, TimeoutError) as e:
        sys.exit(f"couldn't reach the X API ({e}); try again in a little while")

    crypto = args.get("exclude_crypto", True)
    posts = clean(raw, users, crypto)
    before = clean(before_raw, before_users, crypto)
    ctx.update(read=len(raw) + len(before_raw), n_before=len({p["author_id"] for p in before}))
    rows = rank(topics(posts), topics(before), len({p["author_id"] for p in posts}), ctx["n_before"])
    print(report(ctx, rows, posts, trend_list, notes))
    if posts and args.get("save_digest", True):
        print()
        print(f"Saved a Markdown digest of the topics, posts and links to {digest(ctx, rows, posts)}.")
    for line in charts(ctx, rows):
        print(line)


if __name__ == "__main__":
    main()
