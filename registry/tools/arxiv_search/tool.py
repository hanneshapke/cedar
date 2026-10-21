#!/usr/bin/env python3
"""arxiv_search: a community cedar tool. Reads its input as JSON on stdin and prints the result.

Searches arXiv's public API (no key) by keywords, title, author, category and submission date,
or fetches papers by their arXiv ids, and returns each paper's title, authors, dates, categories,
links and abstract. With download_pdfs it also saves the papers' PDFs to arxiv/ in the
workspace, where read_document can read them.

arXiv asks API clients to send at most one request every three seconds: a call makes one search
request, and waits that long between PDF downloads.

Exit non-zero with a message on stderr to report an error; the model sees it as the tool's
error and can try again differently.
"""
import json
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET
from datetime import date
from pathlib import Path

API = "https://export.arxiv.org/api/query"
USER_AGENT = "cedar-arxiv_search/1.0 (+https://github.com/hanneshapke/cedar/tree/main/registry/tools/arxiv_search)"
NS = {"atom": "http://www.w3.org/2005/Atom", "arxiv": "http://arxiv.org/schemas/atom",
      "opensearch": "http://a9.com/-/spec/opensearch/1.1/"}
SORTS = {"relevance": ("relevance", "descending"), "newest": ("submittedDate", "descending"),
         "updated": ("lastUpdatedDate", "descending")}
FIELDS = re.compile(r"\b(ti|au|abs|co|jr|cat|rn|id|all|submittedDate):", re.I)
# New ids (2301.01234, with an optional version) and old ones (hep-th/9901001).
ARXIV_ID = re.compile(r"^(\d{4}\.\d{4,5}|[a-z-]+(\.[A-Z]{2})?/\d{7})(v\d+)?$")
MAX_AUTHORS, MAX_ABSTRACT, MAX_PDFS, PDF_PAUSE = 8, 1800, 5, 3
MAX_PDF_BYTES = 50 << 20


def whole(value, default, low, high):
    try:
        return max(low, min(high, int(value)))
    except (TypeError, ValueError):
        return default


def fetch(url, timeout=60):
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return resp.read(MAX_PDF_BYTES + 1)


# --- the query -------------------------------------------------------------------------------

def terms(text, field):
    """Free text as arXiv query terms: quoted phrases stay together, words are ANDed."""
    parts = re.findall(r'"([^"]+)"|(\S+)', text)
    out = []
    for phrase, word in parts:
        if phrase:
            out.append(f'{field}:"{phrase.strip()}"')
        elif word.upper() not in ("AND", "OR", "ANDNOT"):
            word = re.sub(r'[()":]', "", word)
            out.append(f"{field}:{word}")
    return " AND ".join(t for t in out if not t.endswith(":"))


def day(value, name):
    try:
        return date.fromisoformat(str(value)).strftime("%Y%m%d")
    except ValueError:
        sys.exit(f"{name} must be a date like 2025-01-31, not {value!r}")


def build_query(args):
    """The search_query string, or None when the call only names ids."""
    parts = []
    query = str(args.get("query") or "").strip()
    if query:
        # A query that already uses arXiv's field prefixes (ti:, au:, cat:, ...) goes as is.
        parts.append(f"({query})" if FIELDS.search(query) else terms(query, "all"))
    if args.get("title"):
        parts.append(terms(str(args["title"]), "ti"))
    if args.get("author"):
        name = re.sub(r'["()]', "", str(args["author"])).strip()
        parts.append(f'au:"{name}"' if " " in name else f"au:{name}")
    if args.get("category"):
        cats = [c.strip() for c in str(args["category"]).replace(",", " ").split() if c.strip()]
        for c in cats:
            if not re.match(r"^[a-z-]+(\.[A-Za-z-]+)?\*?$", c):
                sys.exit(f"{c!r} is not an arXiv category such as cs.CL, stat.ML or math.PR")
        parts.append("(" + " OR ".join(f"cat:{c}" for c in cats) + ")" if len(cats) > 1 else f"cat:{cats[0]}")
    since, until = args.get("since"), args.get("until")
    if since or until:
        low = day(since, "since") if since else "19910101"
        high = day(until, "until") if until else date.today().strftime("%Y%m%d")
        parts.append(f"submittedDate:[{low}0000 TO {high}2359]")
    parts = [p for p in parts if p]
    return " AND ".join(parts) or None


def parse_ids(value):
    ids = value if isinstance(value, list) else re.split(r"[\s,]+", str(value or ""))
    out = []
    for raw in ids:
        i = str(raw).strip()
        i = re.sub(r"^(https?://)?(www\.)?arxiv\.org/(abs|pdf)/", "", i).removesuffix(".pdf")
        i = re.sub(r"^arxiv:", "", i, flags=re.I)
        if not i:
            continue
        if not ARXIV_ID.match(i):
            sys.exit(f"{raw!r} is not an arXiv id such as 2301.01234 or hep-th/9901001")
        out.append(i)
    return out


# --- the results -----------------------------------------------------------------------------

def text_of(el, path):
    found = el.find(path, NS)
    return re.sub(r"\s+", " ", found.text).strip() if found is not None and found.text else ""


def paper(entry):
    abs_url = text_of(entry, "atom:id")
    arxiv_id = abs_url.rsplit("/abs/", 1)[-1]
    links = {l.get("title") or l.get("rel"): l.get("href") for l in entry.findall("atom:link", NS)}
    authors = [text_of(a, "atom:name") for a in entry.findall("atom:author", NS)]
    primary = entry.find("arxiv:primary_category", NS)
    return {
        "id": arxiv_id,
        "title": text_of(entry, "atom:title"),
        "authors": authors,
        "published": text_of(entry, "atom:published")[:10],
        "updated": text_of(entry, "atom:updated")[:10],
        "primary_category": primary.get("term") if primary is not None else None,
        "categories": [c.get("term") for c in entry.findall("atom:category", NS)],
        "abstract": text_of(entry, "atom:summary"),
        "comment": text_of(entry, "arxiv:comment"),
        "journal_ref": text_of(entry, "arxiv:journal_ref"),
        "doi": text_of(entry, "arxiv:doi"),
        "url": abs_url.replace("http://", "https://"),
        "pdf": (links.get("pdf") or f"https://arxiv.org/pdf/{arxiv_id}").replace("http://", "https://"),
    }


def search(params):
    url = f"{API}?{urllib.parse.urlencode(params)}"
    try:
        body = fetch(url)
    except urllib.error.HTTPError as e:
        sys.exit(f"arXiv answered {e.code} ({e.reason}); check the query and try again")
    except (urllib.error.URLError, TimeoutError) as e:
        sys.exit(f"couldn't reach arXiv ({e}); try again in a little while")
    try:
        root = ET.fromstring(body)
    except ET.ParseError as e:
        sys.exit(f"arXiv sent a response that isn't a feed ({e}); try again in a little while")
    entries = root.findall("atom:entry", NS)
    # A malformed query comes back as one entry whose id points at the API's error page.
    if len(entries) == 1 and "/api/errors" in text_of(entries[0], "atom:id"):
        sys.exit(f"arXiv couldn't read the query: {text_of(entries[0], 'atom:summary')}")
    total = text_of(root, "opensearch:totalResults")
    return [paper(e) for e in entries if text_of(e, "atom:title")], int(total) if total.isdigit() else None


def download(papers):
    out = Path("arxiv")
    out.mkdir(exist_ok=True)
    saved = []
    for i, p in enumerate(papers[:MAX_PDFS]):
        if i:
            time.sleep(PDF_PAUSE)
        path = out / (p["id"].replace("/", "_") + ".pdf")
        try:
            body = fetch(p["pdf"], timeout=120)
        except (urllib.error.URLError, TimeoutError) as e:
            saved.append((p, None, f"couldn't download ({e})"))
            continue
        if not body.startswith(b"%PDF"):
            saved.append((p, None, "arXiv didn't return a PDF (it may still be processing)"))
        elif len(body) > MAX_PDF_BYTES:
            saved.append((p, None, "larger than 50 MB"))
        else:
            path.write_bytes(body)
            saved.append((p, str(path), None))
    return saved


def report(papers, total, query, ids, sort, start):
    out = []
    if ids and not query:
        out.append(f"{len(papers)} of {len(ids)} requested arXiv paper{'s' * (len(ids) != 1)}:")
        missing = {re.sub(r"v\d+$", "", i) for i in ids} - {re.sub(r"v\d+$", "", p["id"]) for p in papers}
        if missing:
            out.append("Not found on arXiv: " + ", ".join(sorted(missing)) + ".")
    else:
        head = f"arXiv search for {query} (sorted by {sort})"
        if papers:
            of = f" of {total}" if total is not None else ""
            out.append(f"{head}: showing {start + 1}-{start + len(papers)}{of} matches.")
        else:
            out.append(f"{head}: no matches" + (f" from result {start + 1} on." if start else "."))
    for n, p in enumerate(papers, start + 1):
        authors = ", ".join(p["authors"][:MAX_AUTHORS])
        if len(p["authors"]) > MAX_AUTHORS:
            authors += f", and {len(p['authors']) - MAX_AUTHORS} more"
        dates = f"submitted {p['published']}" + (
            f", updated {p['updated']}" if p["updated"] and p["updated"] != p["published"] else "")
        cats = ", ".join(dict.fromkeys(c for c in [p["primary_category"], *p["categories"]] if c))
        abstract = p["abstract"]
        if len(abstract) > MAX_ABSTRACT:
            abstract = abstract[:MAX_ABSTRACT].rsplit(" ", 1)[0] + " [...]"
        out.append("")
        out.append(f"[{n}] {p['title']}")
        out.append(f"    {authors}")
        out.append(f"    arXiv:{p['id']} | {dates} | {cats}")
        extra = [x for x in (p["journal_ref"] and f"Published in {p['journal_ref']}",
                             p["doi"] and f"DOI {p['doi']}", p["comment"]) if x]
        if extra:
            out.append("    " + " | ".join(extra))
        out.append(f"    {p['url']} (PDF: {p['pdf']})")
        out.append(f"    Abstract: {abstract}")
    if not papers:
        out.append("")
        out.append("Nothing matched. Try fewer or broader words, drop the category or dates, "
                   "or search the title or author instead.")
    elif total and start + len(papers) < total and not ids:
        out.append("")
        out.append(f"More results: call again with start={start + len(papers)}.")
    return "\n".join(out)


def main():
    args = json.load(sys.stdin) or {}
    ids = parse_ids(args.get("ids"))
    query = build_query(args)
    if not query and not ids:
        sys.exit("give a query (or a title, author or category) to search for, or arXiv ids to look up")
    count = whole(args.get("max_results"), 8, 1, 25)
    start = whole(args.get("start"), 0, 0, 1000)
    sort = args.get("sort") or "relevance"
    if sort not in SORTS:
        sys.exit(f"sort is one of {', '.join(SORTS)}, not {sort!r}")

    params = {}
    if query:
        params.update(search_query=query, start=start, max_results=count,
                      sortBy=SORTS[sort][0], sortOrder=SORTS[sort][1])
    if ids:
        params["id_list"] = ",".join(ids)
        params.setdefault("max_results", len(ids))
    papers, total = search(params)
    print(report(papers, total, query, ids, sort, start if query else 0))

    if args.get("download_pdfs") and papers:
        print()
        print(f"PDFs (the first {min(len(papers), MAX_PDFS)}, saved for read_document):")
        for p, path, problem in download(papers):
            print(f"- arXiv:{p['id']}: {path}" if path else f"- arXiv:{p['id']}: {problem}")


if __name__ == "__main__":
    main()
