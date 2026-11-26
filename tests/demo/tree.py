#!/usr/bin/env python3
"""Make a believable home folder for the README pictures: someone's
documents, photos, music, downloads and code, with plausible sizes and
dates. Files are sparse (no disk used) and everything is invented.

    python3 tests/demo/tree.py /home/alex
"""
import os, random, sys, time

rng = random.Random(7)
NOW = time.time()
DAY = 86400


def put(path, size, age_days):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.truncate(size)
    t = NOW - age_days * DAY - rng.random() * DAY
    os.utime(path, (t, t))


def kb(lo, hi):
    return int(rng.uniform(lo, hi) * 1024)


def mb(lo, hi):
    return int(rng.uniform(lo, hi) * 1024 * 1024)


def main(home):
    d = lambda *p: os.path.join(home, *p)

    # Paperwork.
    clients = ["Northwind", "Bluebird Studio", "Harbor & Pine", "Kestrel Labs", "Maple Street Bakery"]
    n = 1180
    for i in range(70):
        c = rng.choice(clients)
        age = 700 - i * 10
        year = time.localtime(NOW - age * DAY).tm_year
        put(d("Documents", "Work", "Invoices", str(year), f"invoice-{year}-{n + i:04d} {c}.pdf"), kb(40, 180), age)
    for y in (2024, 2025):
        for m in ("01", "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12"):
            age = (2026 - y) * 365 - int(m) * 30 + 300
            put(d("Documents", "Taxes", str(y), "Receipts", f"receipt-{y}-{m}-{rng.randrange(10, 99)}.pdf"), kb(30, 300), max(age, 1))
        put(d("Documents", "Taxes", str(y), f"tax-return-{y}.pdf"), kb(400, 900), (2026 - y) * 365 - 100)
        put(d("Documents", "Taxes", str(y), f"invoices-summary-{y}.xlsx"), kb(20, 60), (2026 - y) * 365 - 110)
    for y in (2024, 2025, 2026):
        put(d("Documents", "Budget", f"budget-{y}.xlsx"), kb(30, 90), (2026 - y) * 365 + 20)
    put(d("Documents", "Budget", "budget-2026 (copy).xlsx"), kb(30, 90), 3)
    for name, age in [("Quarterly report Q1.docx", 220), ("Quarterly report Q2.docx", 130), ("Quarterly report Q3.docx", 40),
                      ("Quarterly report Q3.pdf", 39), ("Pitch deck.pptx", 60), ("Team offsite notes.md", 25),
                      ("Contract - Kestrel Labs.pdf", 90), ("Contract - Northwind (signed).pdf", 300)]:
        put(d("Documents", "Work", name), kb(50, 4000), age)
    put(d("Documents", "Resume", "resume-2026.pdf"), kb(80, 120), 14)
    put(d("Documents", "Resume", "resume-2026.docx"), kb(30, 60), 14)
    for r in ["banana bread", "chili", "pad thai", "sourdough starter", "lemon tart", "shakshuka"]:
        put(d("Documents", "Recipes", f"{r}.md"), kb(1, 6), rng.randrange(30, 600))
    put(d("Documents", "Apartment", "lease-2025.pdf"), kb(300, 600), 400)
    put(d("Documents", "Apartment", "renters-insurance.pdf"), kb(100, 300), 380)
    put(d("Documents", "Car", "service-invoice-2025-09.pdf"), kb(60, 120), 430)

    # Photos.
    n = 1000
    for year, count in ((2024, 900), (2025, 1400), (2026, 1100)):
        for i in range(count):
            age = (2026 - year) * 365 + 330 - int(i * 330 / count)
            month = time.strftime("%m", time.localtime(NOW - age * DAY))
            put(d("Pictures", "Camera", str(year), month, f"IMG_{n:04d}.jpg"), mb(2, 7), max(age, 0))
            n += 1
            if rng.random() < 0.08:
                put(d("Pictures", "Camera", str(year), month, f"VID_{n:04d}.mp4"), mb(30, 400), max(age, 0))
                n += 1
    for i in range(140):
        age = rng.uniform(1, 330)
        when = time.strftime("%Y%m%d_%H%M%S", time.localtime(NOW - age * DAY))
        path = d("Pictures", "Screenshots", f"Screenshot_{when}.png")
        put(path, kb(80, 900), 0)
        os.utime(path, (NOW - age * DAY,) * 2)
    for w in ["aurora", "dunes", "fjord", "redwoods", "tidepool", "nebula"]:
        put(d("Pictures", "Wallpapers", f"{w}-3840x2160.jpg"), mb(3, 9), rng.randrange(100, 700))

    # Music.
    artists = {"The Paper Lanterns": ["Night Ferry", "Glasshouse"], "Mira Okafor": ["Low Tide"],
               "Static Orchard": ["Fieldwork", "Second Harvest"], "Juniper & The Hum": ["Small Hours"]}
    words = ["Lighthouse", "Copper", "Slow Train", "Saltwater", "Northern", "Paper Moon", "Undertow", "Overpass", "Embers",
             "Afterglow", "Parallel", "Riverbed", "Wildfire", "Echo Park", "Hollow", "Satellites"]
    for a, albums in artists.items():
        for al in albums:
            for t, title in enumerate(rng.sample(words, rng.randrange(8, 13)), 1):
                put(d("Music", a, al, f"{t:02d} - {title}.flac"), mb(20, 45), rng.randrange(200, 900))
            put(d("Music", a, al, "cover.jpg"), kb(300, 900), rng.randrange(200, 900))

    # Downloads.
    for name, size, age in [("trawlr-0.1.0-x86_64.AppImage", mb(9, 10), 0.2), ("invoice-2026-1251 Kestrel Labs.pdf", kb(60, 90), 1),
                            ("boarding-pass-YUL-LIS.pdf", kb(90, 140), 6), ("debian-13.2.0-amd64-netinst.iso", mb(650, 700), 20),
                            ("node-v24.11.1-linux-x64.tar.xz", mb(28, 30), 33), ("fonts.zip", mb(4, 6), 70),
                            ("IMG_3391 (1).jpg", mb(2, 4), 8), ("bank-statement-2026-10.pdf", kb(100, 200), 24),
                            ("Screenshot from 2026-11-02.png", kb(200, 600), 23), ("setup-notes.txt", kb(1, 3), 50)]:
        put(d("Downloads", name), size, age)

    # Code, with the usual heaps of dependencies.
    projects = {"weather-dashboard": "js", "trail-log": "rust", "budget-bot": "py", "dotfiles": None, "website": "js"}
    pkgs = ["react", "react-dom", "scheduler", "lodash", "date-fns", "chart.js", "axios", "zod", "vite", "esbuild", "rollup",
            "postcss", "tailwindcss", "typescript", "eslint", "prettier", "vitest", "@babel/core", "@babel/parser", "picocolors",
            "nanoid", "semver", "debug", "ms", "chalk", "commander", "glob", "minimatch", "yaml", "uuid"]
    stems = ["index", "utils", "helpers", "parse", "format", "config", "client", "server", "types", "report", "reporter",
             "compat", "invoke", "schema", "budget", "plugin", "loader", "cache", "core", "browser", "node"]
    for p, kind in projects.items():
        root = d("Projects", p)
        age = rng.randrange(1, 200)
        put(os.path.join(root, "README.md"), kb(2, 9), age)
        put(os.path.join(root, ".git", "HEAD"), 23, age)
        for i in range(rng.randrange(40, 160)):
            put(os.path.join(root, ".git", "objects", f"{rng.randrange(256):02x}", f"{rng.getrandbits(152):038x}"), kb(1, 30), age + rng.randrange(0, 300))
        if kind == "js":
            put(os.path.join(root, "package.json"), kb(1, 3), age)
            for s in rng.sample(stems, 10):
                put(os.path.join(root, "src", f"{s}.ts"), kb(1, 20), age + rng.randrange(0, 60))
            for pk in pkgs:
                for i in range(rng.randrange(60, 400)):
                    sub = rng.choice(["", "lib", "dist", "esm", "cjs", "src"])
                    put(os.path.join(root, "node_modules", pk, sub, f"{rng.choice(stems)}{i}.js"), kb(0.2, 40), age + rng.randrange(0, 400))
                put(os.path.join(root, "node_modules", pk, "package.json"), kb(1, 4), age + 100)
        elif kind == "rust":
            put(os.path.join(root, "Cargo.toml"), kb(0.5, 2), age)
            for s in ["main", "lib", "parse", "store", "report", "cli"]:
                put(os.path.join(root, "src", f"{s}.rs"), kb(2, 30), age + rng.randrange(0, 60))
            for i in range(3000):
                put(os.path.join(root, "target", "debug", "deps", f"{rng.choice(stems)}-{rng.getrandbits(64):016x}.d"), kb(0.5, 5), age)
        elif kind == "py":
            put(os.path.join(root, "pyproject.toml"), kb(0.5, 2), age)
            for s in ["bot", "budget", "categories", "import_bank", "report"]:
                put(os.path.join(root, "budget_bot", f"{s}.py"), kb(1, 12), age + rng.randrange(0, 60))
            for i in range(2500):
                put(os.path.join(root, ".venv", "lib", "python3.13", "site-packages", rng.choice(pkgs).replace("@", "").replace("/", "_"),
                                 f"{rng.choice(stems)}{i}.py"), kb(0.5, 30), age + 30)
        else:
            for f in [".bashrc", ".vimrc", ".gitconfig", "kitty.conf", "starship.toml", "install.sh"]:
                put(os.path.join(root, f), kb(0.5, 8), age)

    # The usual hidden clutter.
    for i in range(6000):
        put(d(".cache", rng.choice(["thumbnails/large", "thumbnails/normal", "fontconfig", "pip/http"]), f"{rng.getrandbits(128):032x}"),
            kb(1, 200), rng.randrange(5, 400))
    for f in [".bashrc", ".profile", ".gitconfig"]:
        put(d(f), kb(0.5, 4), rng.randrange(100, 700))

    # A folder's date is when something in it last came or went.
    for top, dirs, files in os.walk(home, topdown=False):
        t = max((os.lstat(os.path.join(top, n)).st_mtime for n in dirs + files), default=NOW - 400 * DAY)
        os.utime(top, (t, t))


if __name__ == "__main__":
    main(sys.argv[1])
