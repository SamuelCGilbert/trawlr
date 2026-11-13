#!/usr/bin/env python3
"""End-to-end check that the live index stays identical to the disk.

Starts trawlrd (as the current user, per-folder watches) on a scratch
tree, hammers the tree with random creates, writes, renames, moves and
deletes, and after each round compares what `trawlr` reports with what is
actually on disk. Also restarts the daemon, cleanly and by SIGKILL, with
changes made while it was down.

    python3 tests/stress.py [--rounds N] [--seed N] [--dir DIR]
"""
import argparse, os, random, shutil, signal, subprocess, sys, tempfile, time

HERE = os.path.dirname(os.path.abspath(__file__))
BIN = os.path.join(HERE, "..", "target", "release")
NAMES = ["alpha", "Beta", "gamma.txt", "DELTA.PDF", "notes", "src", "Übung", "a b", "x.tar.gz", "données", "IMG_001.JPG", "readme.MD", "файл", "日本語.txt"]


class Daemon:
    def __init__(self, root, run):
        self.root, self.run = root, run
        self.sock = os.path.join(run, "s")
        self.env = dict(os.environ, TRAWLRD_SOCKET=self.sock)
        self.proc = None

    def start(self):
        log = open(os.path.join(self.run, "log"), "ab")
        self.proc = subprocess.Popen(
            [os.path.join(BIN, "trawlrd"), "--root", self.root, "--socket", self.sock, "--db", os.path.join(self.run, "index.db")],
            stdout=log, stderr=log)
        for _ in range(200):
            if os.path.exists(self.sock) and self.status().get("state") == "ready":
                return
            time.sleep(0.05)
        raise SystemExit("daemon did not become ready; see " + self.run + "/log")

    def stop(self, sig=signal.SIGTERM):
        self.proc.send_signal(sig)
        self.proc.wait()

    def status(self):
        import json
        r = subprocess.run([os.path.join(BIN, "trawlr"), "--status", "--json"], env=self.env, capture_output=True)
        return json.loads(r.stdout) if r.returncode == 0 else {}

    def listing(self):
        """{path: size or None for folders} as the index sees it."""
        import json
        r = subprocess.run([os.path.join(BIN, "trawlr"), "--json"], env=self.env, capture_output=True)
        out = {}
        for line in r.stdout.splitlines():
            row = json.loads(line)
            p = bytes.fromhex(row["raw"]).decode("utf-8", "surrogateescape") if "raw" in row else row["p"].rstrip("/") + "/" + row["n"]
            out[p] = None if row.get("d") else row["s"]
        return out


def disk(root):
    out = {}
    for d, dirs, files in os.walk(root):
        for n in dirs:
            p = os.path.join(d, n)
            out[p] = os.lstat(p).st_size if os.path.islink(p) else None
        for n in files:
            p = os.path.join(d, n)
            out[p] = os.lstat(p).st_size
    return out


def all_dirs(root):
    return [root] + [os.path.join(d, n) for d, dirs, _ in os.walk(root) for n in dirs]


def mutate(root, rng, ops):
    for _ in range(ops):
        dirs = all_dirs(root)
        d = rng.choice(dirs)
        name = rng.choice(NAMES) + rng.choice(["", "", str(rng.randrange(50))])
        p = os.path.join(d, name)
        op = rng.random()
        try:
            if op < 0.30:                                   # create or overwrite a file
                if not os.path.isdir(p):
                    with open(p, "wb") as f:
                        f.write(b"x" * rng.randrange(0, 5000))
            elif op < 0.45:                                 # a new folder with contents
                os.makedirs(os.path.join(p, "sub", "leaf"), exist_ok=True)
                for i in range(rng.randrange(0, 6)):
                    open(os.path.join(p, "sub", f"f{i}.dat"), "w").close()
            elif op < 0.60 and len(dirs) > 1:               # move a folder somewhere else
                src = rng.choice(dirs[1:])
                dst = os.path.join(rng.choice(dirs), name)
                if not dst.startswith(src + os.sep) and dst != src and not os.path.exists(dst):
                    os.rename(src, dst)
            elif op < 0.72:                                 # rename a file, maybe over another
                files = [os.path.join(d, n) for n in os.listdir(d) if os.path.isfile(os.path.join(d, n))]
                if files and not os.path.isdir(p):
                    os.rename(rng.choice(files), p)
            elif op < 0.82 and len(dirs) > 1:               # delete a whole tree
                shutil.rmtree(rng.choice(dirs[1:]))
            elif op < 0.90:                                 # delete a file
                files = [n for n in os.listdir(d) if os.path.isfile(os.path.join(d, n))]
                if files:
                    os.remove(os.path.join(d, rng.choice(files)))
            elif op < 0.95:                                 # symlink
                if not os.path.lexists(p):
                    os.symlink(rng.choice(dirs), p)
            else:                                           # mkdir, fill, rename at once
                os.makedirs(p, exist_ok=True)
                open(os.path.join(p, "inside.txt"), "w").close()
                q = p + ".moved"
                if not os.path.exists(q):
                    os.rename(p, q)
                    open(os.path.join(q, "after.txt"), "w").close()
        except (FileNotFoundError, NotADirectoryError, FileExistsError, OSError):
            pass


def check(daemon, root, what):
    want = disk(root)
    for _ in range(100):                                    # give events time to drain
        got = daemon.listing()
        if got == want:
            print(f"  ok   {what}: {len(want)} entries match")
            return True
        time.sleep(0.05)
    missing = sorted(set(want) - set(got))
    extra = sorted(set(got) - set(want))
    wrong = sorted(p for p in want if p in got and want[p] != got[p])
    print(f"  FAIL {what}: {len(missing)} missing, {len(extra)} stale, {len(wrong)} wrong size")
    for label, items in (("missing", missing), ("stale", extra), ("size", wrong)):
        for p in items[:8]:
            print(f"       {label}: {p}  disk={want.get(p)} index={got.get(p)}")
    return False


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rounds", type=int, default=30)
    ap.add_argument("--ops", type=int, default=120)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--dir", default=None, help="scratch location (default: a temp dir under ~/.cache)")
    ap.add_argument("--run-dir", default=None, help="where the daemon's socket and index go, if the scratch filesystem cannot hold a socket (FAT)")
    a = ap.parse_args()
    base = tempfile.mkdtemp(prefix="evt", dir=a.dir or os.path.expanduser("~/.cache"))
    root = os.path.join(base, "tree")
    run = tempfile.mkdtemp(prefix="evr", dir=a.run_dir) if a.run_dir else os.path.join(base, "r")
    os.makedirs(root), os.makedirs(run, exist_ok=True)
    rng = random.Random(a.seed)
    d = Daemon(root, run)
    ok = True
    try:
        d.start()
        for r in range(a.rounds):
            mutate(root, rng, a.ops)
            ok &= check(d, root, f"round {r + 1} (live)")
            if r % 10 == 4:
                d.stop()
                mutate(root, rng, a.ops)
                d.start()
                ok &= check(d, root, "after a clean restart with offline changes")
            if r % 10 == 9:
                d.stop(signal.SIGKILL)
                mutate(root, rng, a.ops)
                d.start()
                ok &= check(d, root, "after a crash with offline changes")
    finally:
        if d.proc and d.proc.poll() is None:
            d.stop()
        if ok:
            shutil.rmtree(base, ignore_errors=True)
            shutil.rmtree(run, ignore_errors=True)
        else:
            print("kept for inspection:", base)
    print("PASS" if ok else "FAIL")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
