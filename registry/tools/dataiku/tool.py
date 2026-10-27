#!/usr/bin/env python3
"""dataiku: a community cedar tool. Reads its input as JSON on stdin and prints the result.

Works in a Dataiku DSS instance the way Dataiku Headless (github.com/dataiku/dataiku-headless)
does, with Dataiku's own Python API client: it inspects projects, datasets, recipes, scenarios and
jobs, runs the three deterministic executions Headless offers (building a dataset, running a
scenario, and following the job), and talks to Cobuild, Dataiku's agent, which builds and changes
the assets of a project. A Cobuild conversation lives in DSS, so every call stands on its own: the
conversation id, and the id of a pending confirmation or question, come back in the result and the
model passes them to the next call.

Exit non-zero with a message on stderr to report an error; the model sees it as the tool's
error and can try again differently.
"""
import csv
import json
import os
import re
import sys
import time
from pathlib import Path

try:
    import dataikuapi
    from dataikuapi.dss.cobuild import DSSCobuildConversation
    from dataikuapi.utils import DataikuException
except ImportError:
    sys.exit("dataiku-api-client is not installed. An admin installs it with "
             "POST /v1/admin/tools/dataiku/setup (or installs the tool from the marketplace).")

USER_AGENT = "cedar-dataiku"
ACTIONS = ("instance", "list_projects", "project", "dataset", "build", "job", "run_scenario",
           "scenario", "cobuild", "cobuild_confirm", "cobuild_answer")
WRITES = ("build", "run_scenario")
BUILD_MODES = {"recursive": "RECURSIVE_BUILD", "forced": "NON_RECURSIVE_FORCED_BUILD",
               "recursive_forced": "RECURSIVE_FORCED_BUILD"}
KEY = re.compile(r"^[A-Za-z0-9_.-]{1,200}$")
MAX_LIST, MAX_SAMPLE, MAX_EXPORT, MAX_LOG = 200, 200, 1_000_000, 6000
# How long a call waits for a job or scenario before handing back its id to check later. The
# tool's own timeout (tool.json) is longer, so a call always answers.
WAIT_SECS = 240
HERE = Path(__file__).resolve().parent


def fail(message):
    sys.exit(message)


def whole(value, default, low, high):
    try:
        return max(low, min(high, int(value)))
    except (TypeError, ValueError):
        return default


def required(args, name):
    value = str(args.get(name) or "").strip()
    if not value:
        fail(f"{name} is required for action {args.get('action')!r}")
    if not KEY.match(value):
        fail(f"{name} {value!r} is not a valid Dataiku identifier")
    return value


def load_env(path):
    """Read KEY=VALUE lines into os.environ, without overriding what is already set (the secrets
    cedar passes in win). A missing file is fine when those secrets are set."""
    if not path.is_file():
        return
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key, value = key.strip().removeprefix("export ").strip(), value.strip()
        if value[:1] in "\"'" and value[-1:] == value[:1] and len(value) > 1:
            value = value[1:-1]
        elif " #" in value:
            value = value.split(" #", 1)[0].rstrip()
        os.environ.setdefault(key, value)


def truthy(name):
    return os.environ.get(name, "").strip().lower() in ("1", "true", "yes", "on")


def client():
    url = os.environ.get("DKU_DSS_URL", "").strip().rstrip("/")
    key = os.environ.get("DKU_API_KEY", "").strip()
    if not url or not key:
        fail("no Dataiku credentials: save DKU_DSS_URL and DKU_API_KEY in this tool's settings in "
             f"cedar, or put them in {HERE.name}/.env (see .env.example)")
    if not url.startswith("https://") and not url.startswith("http://"):
        fail(f"DKU_DSS_URL must start with https://, not {url!r}")
    return url, dataikuapi.DSSClient(url, api_key=key, extra_headers={"X-DKU-Client-Application": USER_AGENT})


def project_url(base, project_key):
    return f"{base}/projects/{project_key}/flow/"


def short(text, limit):
    text = str(text or "")
    return text if len(text) <= limit else "…" + text[-limit:]


# --- inspection ------------------------------------------------------------------------------

def instance(base, dss, args):
    auth = dss.get_auth_info()
    try:
        version = dss.get_instance_info().raw.get("dssVersion")
    except Exception:  # older instances, or keys that may not read it
        version = None
    return {"url": base, "dss_version": version, "user": auth.get("authIdentifier"),
            "groups": auth.get("groups"), "read_only": truthy("DKU_READ_ONLY")}


def list_projects(base, dss, args):
    query = str(args.get("query") or "").strip().lower()
    limit = whole(args.get("limit"), 50, 1, MAX_LIST)
    projects = []
    for p in dss.list_projects():
        if query and query not in (p.get("projectKey", "") + " " + p.get("name", "")).lower():
            continue
        projects.append({"project_key": p.get("projectKey"), "name": p.get("name"),
                         "owner": p.get("ownerLogin"), "description": short(p.get("shortDesc"), 200),
                         "tags": p.get("tags") or None})
    return {"count": len(projects), "projects": projects[:limit], "truncated": len(projects) > limit}


def project(base, dss, args):
    key = required(args, "project_key")
    p = dss.get_project(key)
    summary = p.get_summary()
    datasets = [{"name": d.name, "type": d.type, "columns": len((d.schema or {}).get("columns") or [])}
                for d in p.list_datasets()]
    recipes = [{"name": r.name, "type": r.type} for r in p.list_recipes()]
    # The SDK's list item answers its name for .id, so the raw fields are read instead.
    scenarios = [{"id": s._data.get("id"), "name": s._data.get("name"), "active": s._data.get("active")}
                 for s in p.list_scenarios()]
    return {"project_key": key, "name": summary.get("name"), "owner": summary.get("ownerLogin"),
            "description": short(summary.get("shortDesc"), 500),
            "datasets": datasets[:MAX_LIST], "recipes": recipes[:MAX_LIST],
            "scenarios": scenarios[:MAX_LIST], "variables": p.get_variables().get("standard") or None,
            "url": project_url(base, key)}


def dataset(base, dss, args):
    key, name = required(args, "project_key"), required(args, "dataset")
    ds = dss.get_project(key).get_dataset(name)
    columns = ds.get_schema().get("columns") or []
    out = {"project_key": key, "dataset": name,
           "columns": [{"name": c.get("name"), "type": c.get("type"),
                        "meaning": c.get("meaning") or None, "comment": c.get("comment") or None}
                       for c in columns]}
    try:
        info = ds.get_info().get_raw()
        out["type"] = info.get("dataset", {}).get("type")
        status = info.get("status") or {}
        out["records"] = (status.get("records") or {}).get("totalValue")
        out["size_bytes"] = (status.get("size") or {}).get("totalValue")
    except Exception:
        pass
    sample = whole(args.get("limit"), 20, 0, MAX_SAMPLE)
    export = bool(args.get("export"))
    if not sample and not export:
        return out
    names = [c.get("name") for c in columns]
    rows, writer, path, written = [], None, None, 0
    cap = whole(args.get("export_limit"), 100_000, 1, MAX_EXPORT) if export else sample
    partitions = args.get("partitions") or None
    handle = None
    try:
        if export:
            path = Path("dataiku") / key / f"{name}.csv"
            path.parent.mkdir(parents=True, exist_ok=True)
            handle = open(path, "w", newline="", encoding="utf-8")
            writer = csv.writer(handle)
            writer.writerow(names)
        for row in ds.iter_rows(partitions=partitions):
            if len(rows) < sample:
                rows.append(dict(zip(names, row)))
            if writer:
                writer.writerow(row)
            written += 1
            if written >= max(cap, sample):
                break
    finally:
        if handle:
            handle.close()
    out["sample"] = rows
    if export:
        out["exported"] = {"path": str(path), "rows": written, "may_be_truncated": written >= cap}
    return out


# --- jobs and scenarios ----------------------------------------------------------------------

def job_state(job):
    status = job.get_status()
    base = status.get("baseStatus", {})
    return base.get("state"), status


def job_result(base, key, job, wait):
    deadline = time.monotonic() + (WAIT_SECS if wait else 0)
    state, status = job_state(job)
    while state not in ("DONE", "FAILED", "ABORTED") and time.monotonic() < deadline:
        time.sleep(3)
        state, status = job_state(job)
    out = {"project_key": key, "job_id": job.id, "state": state,
           "url": f"{base}/projects/{key}/jobs/{job.id}/"}
    if state not in ("DONE", "FAILED", "ABORTED"):
        out["next"] = "Still running: check it later with action job and this job_id."
    if state in ("FAILED", "ABORTED"):
        out["log_tail"] = short(job.get_log(), MAX_LOG)
    activities = status.get("baseStatus", {}).get("activities") or {}
    failed = [{"activity": a, "error": short((v.get("firstFailure") or {}).get("message"), 500)}
              for a, v in activities.items() if v.get("state") == "FAILED"]
    if failed:
        out["failed_activities"] = failed[:10]
    return out


def build(base, dss, args):
    key, name = required(args, "project_key"), required(args, "dataset")
    mode = str(args.get("mode") or "forced")
    if mode not in BUILD_MODES:
        fail(f"mode must be one of {', '.join(BUILD_MODES)}, not {mode!r}")
    p = dss.get_project(key)
    jd = p.new_job(BUILD_MODES[mode])
    jd.with_output(name, partition=args.get("partitions") or None)
    job = jd.start()
    return job_result(base, key, job, args.get("wait", True))


def job(base, dss, args):
    key, job_id = required(args, "project_key"), required(args, "job_id")
    out = job_result(base, key, dss.get_project(key).get_job(job_id), False)
    if args.get("log") and "log_tail" not in out:
        out["log_tail"] = short(dss.get_project(key).get_job(job_id).get_log(), MAX_LOG)
    return out


def run_summary(run):
    info = run.run
    result = info.get("result") or {}
    return {"run_id": run.id, "outcome": result.get("outcome"), "running": run.running,
            "start": info.get("start"), "end": info.get("end") or None}


def run_scenario(base, dss, args):
    key, sid = required(args, "project_key"), required(args, "scenario_id")
    scenario = dss.get_project(key).get_scenario(sid)
    params = args.get("params") if isinstance(args.get("params"), dict) else {}
    fire = scenario.run(params)
    run = fire.wait_for_scenario_run(no_fail=True)
    if run is None:
        return {"project_key": key, "scenario_id": sid, "started": False,
                "message": "DSS cancelled the manual trigger, most likely because the scenario is already running."}
    deadline = time.monotonic() + (WAIT_SECS if args.get("wait", True) else 0)
    while run.running and time.monotonic() < deadline:
        time.sleep(5)
        run.refresh()
    out = {"project_key": key, "scenario_id": sid, **run_summary(run),
           "url": f"{base}/projects/{key}/scenarios/{sid}/runs/list/"}
    if run.running:
        out["next"] = "Still running: check it later with action scenario and this scenario_id."
    return out


def scenario(base, dss, args):
    key, sid = required(args, "project_key"), required(args, "scenario_id")
    s = dss.get_project(key).get_scenario(sid)
    runs = [run_summary(r) for r in s.get_last_runs(limit=whole(args.get("limit"), 5, 1, 20))]
    return {"project_key": key, "scenario_id": sid, "runs": runs,
            "url": f"{base}/projects/{key}/scenarios/{sid}/runs/list/"}


# --- Cobuild ---------------------------------------------------------------------------------

def turn(base, key, conv, response):
    out = {"project_key": key, "conversation_id": conv.conversation_id,
           "type": response.type, "message": response.message, "is_error": bool(response.is_error),
           "url": project_url(base, key)}
    if response.is_confirmation_request:
        out["confirmation_id"] = response._raw.get("confirmationId")
        out["objects_to_delete"] = response.objects_to_delete
        out["deletion_impacts"] = response.deletion_impacts
        out["next"] = ("Cobuild waits for a confirmation. Show the user exactly what it would delete "
                       "and ask; then call cobuild_confirm with this confirmation_id and APPROVE or CANCEL.")
    if response.is_question_request:
        out["question_id"] = response.question_id
        question = {"title": response.title, "answers": response.predefined_answers,
                    "custom_answer_allowed": response.allow_custom_answer,
                    "multiple_answers_allowed": response.allow_multiple_answers}
        out["question"] = {k: v for k, v in question.items() if v is not None}
        out["next"] = "Cobuild asks a question: answer it with cobuild_answer and this question_id."
    return {k: v for k, v in out.items() if v is not None}


def conversation(dss, args, key):
    conversation_id = str(args.get("conversation_id") or "").strip()
    if conversation_id:
        if not KEY.match(conversation_id):
            fail(f"conversation_id {conversation_id!r} is not valid")
        return DSSCobuildConversation(dss, key, conversation_id)
    return dss.get_project(key).new_cobuild_conversation()


def cobuild(base, dss, args):
    key = required(args, "project_key")
    message = str(args.get("message") or "").strip()
    if not message:
        fail("message is required for action 'cobuild'")
    allow_edit = bool(args.get("allow_edit"))
    if allow_edit and truthy("DKU_READ_ONLY"):
        fail("This cedar server set DKU_READ_ONLY, so Cobuild may only inspect (allow_edit=false).")
    conv = conversation(dss, args, key)
    return turn(base, key, conv, conv.send_message(message, allow_edit_project=allow_edit))


def cobuild_confirm(base, dss, args):
    key, cid = required(args, "project_key"), required(args, "conversation_id")
    confirmation_id = required(args, "confirmation_id")
    choice = str(args.get("choice") or "").upper()
    if choice not in ("APPROVE", "CANCEL"):
        fail("choice must be APPROVE or CANCEL")
    if choice == "APPROVE" and truthy("DKU_READ_ONLY"):
        fail("This cedar server set DKU_READ_ONLY, so a deletion can only be cancelled.")
    conv = DSSCobuildConversation(dss, key, cid)
    conv._pending_confirmation_id = confirmation_id
    return turn(base, key, conv, conv.answer_confirmation(choice))


def cobuild_answer(base, dss, args):
    key, cid = required(args, "project_key"), required(args, "conversation_id")
    question_id = required(args, "question_id")
    answers = args.get("answers") or []
    if not isinstance(answers, list) or not all(isinstance(a, str) for a in answers):
        fail("answers must be a list of strings")
    reject = bool(args.get("reject"))
    if reject:
        answers = []
    elif not answers:
        fail("give at least one answer, or reject=true to let Cobuild go on without one")
    conv = DSSCobuildConversation(dss, key, cid)
    conv._pending_question_id = question_id
    return turn(base, key, conv, conv.answer_question(answers, rejected=reject,
                                                      used_custom_answer=bool(args.get("custom_answer"))))


def main():
    args = json.load(sys.stdin) or {}
    load_env(HERE / ".env")
    action = args.get("action")
    if action not in ACTIONS:
        fail(f"action must be one of {', '.join(ACTIONS)}, not {action!r}")
    if action in WRITES and truthy("DKU_READ_ONLY"):
        fail(f"This cedar server set DKU_READ_ONLY, so {action} is not allowed.")
    base, dss = client()
    try:
        result = globals()[action](base, dss, args)
    except DataikuException as e:
        fail(f"Dataiku refused {action}: {short(e, 1500)}")
    except OSError as e:
        fail(f"Could not reach {base}: {e}")
    print(json.dumps(result, ensure_ascii=False, default=str))


if __name__ == "__main__":
    main()
