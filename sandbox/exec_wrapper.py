# Runs one command for cedar inside an OpenShell sandbox. cedar passes this script to
# `python3 -I -S -c` (see src/sandbox.rs); it lives here so it can be read and tested on its own.
#
#     python3 -I -S -c "$WRAPPER" TIMEOUT_SECS MAX_OUTPUT sh  COMMAND
#     python3 -I -S -c "$WRAPPER" TIMEOUT_SECS MAX_OUTPUT run PROGRAM ARGS...
#
# Why a wrapper: OpenShell refuses signals to a whole process group, so `timeout` can stop the
# command but not what it started; and a background process that keeps stdout open would hold the
# exec open. This runs the command in a session of its own, stops it and every process in that
# session when the time is up (one by one, which OpenShell allows), stops reading shortly after
# the command exits, and keeps only the first MAX_OUTPUT bytes of each stream while counting all
# of them. It prints what it kept, then one last line on stderr:
#
#     \x1ecedar-exit:<code>:<stdout bytes>:<stderr bytes>
#
# where <code> is the exit status (128 + n for signal n), or 124 if the time ran out.
#
# The command must not be a descendant of this Python process: OpenShell lets a program's
# children use the network rules written for that program, so a command started by Python would
# get whatever a policy allows Python. It is started through a double fork instead, so its parent
# is the sandbox's init by the time it runs, and reports its exit status through a pipe.
import os, select, signal, sys, threading, time

timeout, cap, mode = float(sys.argv[1]), int(sys.argv[2]), sys.argv[3]
argv = ["sh", "-c", sys.argv[4]] if mode == "sh" else sys.argv[4:]
# `sh` runs the command and writes its status to fd 3; it is all that stands between the command
# and init.
runner = ["sh", "-c", '"$@" 3>&-; echo $? >&3', "sh", *argv]


def drain(fd, box):
    while True:
        try:
            chunk = os.read(fd, 65536)
        except OSError:
            return
        if not chunk:
            return
        box[1] += len(chunk)
        if len(box[0]) < cap:
            box[0] += chunk[: cap - len(box[0])]


def session(sid):
    """The live processes in session `sid` (a zombie is already gone, only not reaped yet)."""
    pids = []
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            with open(f"/proc/{entry}/stat") as f:
                fields = f.read().rsplit(")", 1)[1].split()
            if int(fields[3]) == sid and fields[0] != "Z":
                pids.append(int(entry))
        except (OSError, IndexError, ValueError):
            pass
    return pids


def stop(sid):
    for sig in (signal.SIGTERM, signal.SIGKILL):
        for pid in session(sid):
            try:
                os.kill(pid, sig)
            except OSError:
                pass
        deadline = time.monotonic() + 2
        while session(sid) and time.monotonic() < deadline:
            time.sleep(0.05)


out_r, out_w = os.pipe()
err_r, err_w = os.pipe()
status_r, status_w = os.pipe()
middle = os.fork()
if middle == 0:
    # The middle process: a new session for the command, then gone, so the command's parent is
    # init before it runs anything.
    try:
        os.setsid()
        gate_r, gate_w = os.pipe()
        if os.fork() == 0:
            os.close(gate_w)
            os.read(gate_r, 1)  # end of file once the middle process has exited
            os.dup2(out_w, 1)
            os.dup2(err_w, 2)
            os.dup2(status_w, 3)
            os.closerange(4, 1 << 16)
            try:
                os.execvp(runner[0], runner)
            except OSError as e:
                os.write(2, f"{argv[0]}: {e.strerror}\n".encode())
                os.write(3, b"127\n")
            os._exit(127)
    finally:
        os._exit(0)
# The session is named after the middle process, which led it.
sid = middle
os.waitpid(middle, 0)
for fd in (out_w, err_w, status_w):
    os.close(fd)
out, err = [b"", 0], [b"", 0]
readers = [
    threading.Thread(target=drain, args=(out_r, out), daemon=True),
    threading.Thread(target=drain, args=(err_r, err), daemon=True),
]
for r in readers:
    r.start()
status = b""
deadline = time.monotonic() + timeout
timed_out = False
while True:
    left = deadline - time.monotonic()
    if left <= 0:
        timed_out = True
        break
    ready, _, _ = select.select([status_r], [], [], left)
    if ready:
        chunk = os.read(status_r, 64)
        status += chunk
        if not chunk or status.endswith(b"\n"):
            break
if timed_out:
    stop(sid)
# What the command printed before it ended; a background process may keep the pipes open, so
# don't wait for the end of the streams for long.
for r in readers:
    r.join(0.5)
try:
    # No status at all: `sh` itself was killed.
    code = 124 if timed_out else int(status.strip() or b"137")
except ValueError:
    code = 137
sys.stdout.buffer.write(out[0])
sys.stdout.flush()
sys.stderr.buffer.write(err[0])
sys.stderr.write(f"\n\x1ecedar-exit:{code}:{out[1]}:{err[1]}\n")
sys.stderr.flush()
os._exit(code if 0 <= code < 256 else 1)
