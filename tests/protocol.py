#!/usr/bin/env python3
"""Hostile-client checks against a live trawlrd (Linux and macOS).

The daemon runs as root and answers every local user it allows to search,
so nothing a client sends may crash it, wedge it, or make it hold up the
others. Starts trawlrd on a scratch tree as the current user and throws at
it: bytes that are not JSON, JSON of the wrong shape, huge numbers, an
endless line, searches built to exhaust the stack or memory, too many
connections at once, and a client that never reads its replies while files
change. After each, the daemon must still answer and keep up with the disk.

    python3 tests/protocol.py
"""
import json, os, random, shutil, socket, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
BIN = os.path.join(HERE, "..", "target", "release")
fails = 0


def ok(what):
    print(f"  ok   {what}")


def bad(what):
    global fails
    fails += 1
    print(f"  FAIL {what}")


def connect(sock):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(30)
    s.connect(sock)
    return s


def call(sock, req, s=None):
    own = s is None
    s = s or connect(sock)
    s.sendall((json.dumps(req) + "\n").encode())
    f = s.makefile("rb")
    while True:
        line = f.readline()
        if not line:
            return None
        o = json.loads(line)
        if "event" not in o:
            break
    if own:
        s.close()
    return o


def alive(sock, what):
    try:
        r = call(sock, {"op": "status"})
        if r and r.get("ok"):
            ok(f"still serving after {what}")
            return True
    except Exception as e:
        bad(f"not serving after {what}: {e}")
        return False
    bad(f"no status after {what}: {r}")
    return False


def count(sock, q):
    r = call(sock, {"op": "query", "qid": 1, "q": q, "limit": 1})
    return r.get("total") if r else None


def main():
    # As the daemon spells it (on macOS /var is a link to /private/var).
    base = os.path.realpath(tempfile.mkdtemp(prefix="trawlr-proto-"))
    tree = os.path.join(base, "tree")
    os.makedirs(os.path.join(tree, "a", "b"))
    for i in range(200):
        open(os.path.join(tree, "a", f"file{i}.txt"), "w").close()
    sock = os.path.join(base, "s")
    log = open(os.path.join(base, "log"), "wb")
    d = subprocess.Popen([os.path.join(BIN, "trawlrd"), "--root", tree, "--socket", sock, "--db", os.path.join(base, "db")], stdout=log, stderr=log)
    try:
        for _ in range(200):
            if os.path.exists(sock):
                try:
                    if call(sock, {"op": "status"}).get("state") == "ready":
                        break
                except OSError:
                    pass
            time.sleep(0.05)
        else:
            raise SystemExit("daemon did not start")

        # Not JSON, not UTF-8, wrong types, odd ops.
        rng = random.Random(1)
        s = connect(sock)
        # Read the answers as they come (a client that stops reading is
        # dropped after a while, which is checked further down).
        replies = []
        reader = threading.Thread(target=lambda: replies.extend(s.makefile("rb")), daemon=True)
        reader.start()
        junk = [b"\n", b"{\n", b"null\n", b"[]\n", b"\xff\xfe\x00garbage\n", b'{"op": 5}\n', b'{"op":"query","q":7}\n',
                b'{"op":"query","qid":-1}\n', b'{"op":"rows","offset":18446744073709551615,"limit":18446744073709551615}\n',
                b'{"op":"query","q":"a","offset":18446744073709551615,"limit":0}\n', b'{"op":"query","exclude":"x"}\n',
                b'{"op":"query","sort":"\\u0000"}\n', b'{"op":"nonsense"}\n', b'{"op":"query","q":"\\ud800"}\n']
        junk += [bytes(rng.randrange(256) for _ in range(rng.randrange(1, 300))) + b"\n" for _ in range(300)]
        for j in junk:
            s.sendall(j)
        s.shutdown(socket.SHUT_WR)
        reader.join(30)
        s.close()
        (ok if len(replies) >= len(junk) else bad)(f"every malformed request answered ({len(replies)} replies to {len(junk)} lines)")
        alive(sock, "malformed requests")

        # A line with no end: dropped at 1 MB, not buffered forever.
        s = connect(sock)
        try:
            for _ in range(8):
                s.sendall(b"a" * (1 << 20))
        except OSError:
            pass
        s.close()
        alive(sock, "an endless line")

        # Searches built to exhaust the stack, memory or the regex engine.
        hostile = ["!" * 400_000, "<" * 300_000 + "a", " ".join(f"d{i}/" for i in range(20_000)), "|".join(f"*{i}*" for i in range(5000)),
                   "regex:(a{1000}){1000}", "regex:" + "(" * 50_000 + "a" + ")" * 50_000, " ".join(["ext:" + ";".join(["x"] * 1000)] * 100)]
        for q in hostile:
            for opts in ({}, {"regex": True}, {"path": True}):
                r = call(sock, dict({"op": "query", "qid": 2, "q": q, "limit": 10}, **opts))
                if r is None:
                    bad(f"connection dropped on a hostile search ({q[:20]}...)")
        r = call(sock, {"op": "query", "qid": 3, "q": "file", "exclude": [tree + "/a"] + [f"/x{i}" for i in range(60_000)]})
        (ok if r and r.get("total") == 0 else bad)(f"a huge exclude list is answered ({r and r.get('total')})")
        alive(sock, "hostile searches")

        # More connections than it serves at once: the extra ones are told
        # so, and the daemon carries on.
        held = []
        refused = 0
        for _ in range(150):
            try:
                c = connect(sock)
                c.settimeout(0.3)
                try:
                    msg = c.recv(4096)
                    if b"too many" in msg:
                        refused += 1
                except socket.timeout:
                    pass
                held.append(c)
            except OSError:
                refused += 1
        (ok if refused > 0 else bad)(f"connections beyond the limit refused ({refused} of 150)")
        for c in held:
            c.close()
        time.sleep(0.5)
        alive(sock, "a connection flood")

        # A client that searches and then never reads, while files change:
        # the others must still see changes promptly.
        stuck = connect(sock)
        stuck.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 4096)
        for i in range(50):
            stuck.sendall((json.dumps({"op": "query", "qid": i, "q": "", "limit": 5000}) + "\n").encode())
        t0 = time.time()
        for i in range(20):
            open(os.path.join(tree, "a", f"late{i}.txt"), "w").close()
            time.sleep(0.05)
        seen = None
        while time.time() - t0 < 10:
            if count(sock, "late") == 20:
                seen = time.time() - t0
                break
            time.sleep(0.1)
        (ok if seen is not None and seen < 6 else bad)(f"changes reach other clients while one stops reading ({seen})")
        stuck.close()

        # Rescans on demand are limited.
        a = call(sock, {"op": "rescan", "deep": True})
        b = call(sock, {"op": "rescan", "deep": True})
        (ok if a and a.get("ok") and b and not b.get("ok") else bad)(f"a second rescan within a minute is refused ({a}, {b})")
        time.sleep(1)
        alive(sock, "a rescan")
        (ok if count(sock, "file") == 200 else bad)("the index is intact")
    finally:
        d.terminate()
        d.wait()
        log.close()
        if fails:
            print(open(os.path.join(base, "log"), errors="replace").read()[-3000:])
        shutil.rmtree(base, ignore_errors=True)
    print("PASS" if not fails else f"FAIL ({fails})")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
