//! The `trawlr` command: prints the names that match a search, or, with no
//! search (or --gui, --new-window, --toggle), hands over to the window,
//! which is a program of its own, `trawlr-gui`.

use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;

/// A Windows named pipe opens like a file. Reads and writes take turns here,
/// so one handle, cloned, serves both directions.
#[cfg(windows)]
type UnixStream = std::fs::File;

use trawlr::config::client_socket;
use serde_json::{json, Value};

const HELP: &str = "\
trawlr - search the trawlrd file index

Usage: trawlr [options] search terms      search, and print what matches
       trawlr                             open the search window
       trawlr --gui [search terms]        open the window with a search typed in
       trawlr --new-window | --toggle     another window / show or hide the window

Search syntax (as in voidtools' Everything):
  foo bar          both words, anywhere in the name
  foo|bar          either word            !foo        not foo
  \"foo bar\"        a phrase with spaces   <a|b> c     grouping
  *.pdf  inv?ice   wildcards match the whole name
  docs/ report     a term containing / matches against the full path
  ext:jpg;png      size:>10mb   size:1mb..2gb   dm:today   dm:2024-03
  file:  folder:   case:  regex:  path:  ww:  wfn:  startwith:  endwith:
  audio: video: pic: doc: zip: exe:   parent:/some/dir   empty:

Options:
  -n, --limit N    print at most N results
  -s, --sort KEY   name (default), path, size, date, ext
  -d, --desc       descending order
  -c, --case       match case
  -p, --path       match the full path, not only the name
  -r, --regex      treat the search as a regular expression
  -w, --word       match whole words only
  -l, --long       also print size and modification time
  -0, --null       separate results with NUL instead of newline
      --count      print only the number of results
      --json       print raw JSON rows
      --status     show what the daemon is indexing and how
      --rescan     check the whole index against the disk
      --rebuild    throw the index away and scan from scratch
  -h, --help       show this help
";

struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

#[cfg(target_os = "macos")]
const SERVICE_HINT: &str = "sudo launchctl print system/io.trawlr.trawlrd";
#[cfg(all(unix, not(target_os = "macos")))]
const SERVICE_HINT: &str = "systemctl status trawlrd";

impl Client {
    fn connect() -> Result<Client, String> {
        let path = client_socket();
        #[cfg(unix)]
        let s = UnixStream::connect(&path).map_err(|e| format!("cannot reach trawlrd at {}: {e}\nIs the service running?  {SERVICE_HINT}", path.display()))?;
        #[cfg(windows)]
        let s = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| format!("cannot reach trawlrd at {}: {e}\nIs the Trawlr service running?  sc query trawlrd", path.display()))?;
        // Unless told which socket to use, only believe the real daemon.
        if std::env::var_os("TRAWLRD_SOCKET").is_none() {
            #[cfg(unix)]
            trawlr::peer::check_daemon(std::os::fd::AsRawFd::as_raw_fd(&s))?;
            #[cfg(windows)]
            trawlr::peer::check_daemon(&s)?;
        }
        Ok(Client { reader: BufReader::new(s.try_clone().map_err(|e| e.to_string())?), writer: s })
    }

    fn call(&mut self, req: Value) -> Result<Value, String> {
        let mut line = serde_json::to_vec(&req).unwrap();
        line.push(b'\n');
        self.writer.write_all(&line).map_err(|e| e.to_string())?;
        loop {
            let mut buf = String::new();
            if self.reader.read_line(&mut buf).map_err(|e| e.to_string())? == 0 {
                return Err("trawlrd closed the connection".into());
            }
            let v: Value = serde_json::from_str(&buf).map_err(|e| e.to_string())?;
            // Change notifications can arrive between a request and its reply.
            if v.get("event").is_some() {
                continue;
            }
            if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
                return Err(e.to_string());
            }
            return Ok(v);
        }
    }
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()).collect()
}

fn human(n: u64) -> String {
    const U: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n}B") } else { format!("{v:.1}{}", U[i]) }
}

fn local_time(t: i64) -> String {
    let tm = trawlr::query::local_tm(t);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
}

/// Options that are for the window, not for a search here.
const WINDOW_FLAGS: [&str; 4] = ["--gui", "-g", "--new-window", "--toggle"];

fn window_program() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TRAWLR_GUI") {
        return Some(p.into());
    }
    // Installed side by side (this may be a link to it, as on macOS, where
    // /usr/local/bin/trawlr points into the app); else wherever PATH finds it.
    let name = format!("trawlr-gui{}", std::env::consts::EXE_SUFFIX);
    if let Ok(exe) = std::env::current_exe().and_then(|e| e.canonicalize()) {
        let p = exe.with_file_name(&name);
        if p.is_file() {
            return Some(p);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(&name)).find(|p| p.is_file())
}

/// Replace this process with the window, so the desktop's activation token
/// and launch feedback go straight to it. Windows cannot replace a process,
/// so there the window is started and left running.
fn open_window(args: Vec<String>) -> Result<i32, String> {
    let Some(gui) = window_program() else {
        return Err("the search window (trawlr-gui) is not installed; search from here with: trawlr <search terms>".into());
    };
    #[cfg(unix)]
    let err = std::process::Command::new(&gui).args(args).exec();
    #[cfg(windows)]
    let err = match std::process::Command::new(&gui).args(args).spawn() {
        Ok(_) => return Ok(0),
        Err(e) => e,
    };
    Err(format!("cannot start {}: {err}", gui.display()))
}

fn run() -> Result<i32, String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        if window_program().is_some() {
            return open_window(argv);
        }
        eprint!("{HELP}");
        return Ok(2);
    }
    if argv.iter().take_while(|a| *a != "--").any(|a| WINDOW_FLAGS.contains(&a.as_str())) {
        return open_window(argv.into_iter().filter(|a| a != "--gui" && a != "-g").collect());
    }

    let mut terms: Vec<String> = Vec::new();
    let (mut limit, mut sort, mut desc) = (u64::MAX, String::from("name"), false);
    let (mut case, mut path, mut regex, mut word) = (false, false, false, false);
    let (mut long, mut null, mut count, mut as_json) = (false, false, false, false);
    let mut action = "";
    let mut args = std::env::args().skip(1);
    let mut literal = false;
    while let Some(a) = args.next() {
        if literal || !a.starts_with('-') || a == "-" {
            terms.push(a);
            continue;
        }
        match a.as_str() {
            "--" => literal = true,
            "-n" | "--limit" => limit = args.next().and_then(|v| v.parse().ok()).ok_or("--limit needs a number")?,
            "-s" | "--sort" => sort = args.next().ok_or("--sort needs a key")?,
            "-d" | "--desc" => desc = true,
            "-c" | "--case" => case = true,
            "-p" | "--path" => path = true,
            "-r" | "--regex" => regex = true,
            "-w" | "--word" => word = true,
            "-l" | "--long" => long = true,
            "-0" | "--null" => null = true,
            "--count" => count = true,
            "--json" => as_json = true,
            "--status" => action = "status",
            "--rescan" => action = "rescan",
            "--rebuild" => action = "rebuild",
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(0);
            }
            "-V" | "--version" => {
                println!("trawlr {}", env!("CARGO_PKG_VERSION"));
                return Ok(0);
            }
            other => return Err(format!("unknown option {other:?} (try --help)")),
        }
    }

    let mut c = Client::connect()?;
    match action {
        "status" => {
            let s = c.call(json!({"op": "status"}))?;
            if as_json {
                println!("{s}");
                return Ok(0);
            }
            let n = |k: &str| s[k].as_u64().unwrap_or(0);
            println!("trawlrd {} ({} mode), {}", s["version"].as_str().unwrap_or("?"), s["mode"].as_str().unwrap_or("?"), s["state"].as_str().unwrap_or("?"));
            println!("  {} files, {} folders, {} MB in memory", n("files"), n("folders"), n("memory_mb"));
            for f in s["filesystems"].as_array().into_iter().flatten() {
                println!("  {} ({}): watched {}", f["mount"].as_str().unwrap_or("?"), f["type"].as_str().unwrap_or("?"), f["watch"].as_str().unwrap_or("?"));
            }
            println!("  {} change events handled since start", n("events"));
            if n("unreadable_folders") > 0 {
                println!("  {} folders could not be read, such as:", n("unreadable_folders"));
                for p in s["unreadable"].as_array().into_iter().flatten().take(5) {
                    println!("    {}", p.as_str().unwrap_or("?"));
                }
                if s["needs_full_disk_access"].as_bool() == Some(true) {
                    println!("  Give trawlrd Full Disk Access to index them (System Settings > Privacy & Security).");
                }
            }
            if n("unwatched_folders") > 0 {
                println!("  warning: {} folders are not watched (fs.fanotify.max_user_marks reached)", n("unwatched_folders"));
            }
            return Ok(0);
        }
        "rescan" | "rebuild" => {
            c.call(json!({"op": "rescan", "deep": true, "rebuild": action == "rebuild"}))?;
            println!("{}", if action == "rebuild" { "rebuilding the index from scratch" } else { "checking the index against the disk" });
            return Ok(0);
        }
        _ => {}
    }

    let q = terms.join(" ");
    const PAGE: u64 = 2000;
    let first = c.call(json!({
        "op": "query", "qid": 1, "q": q, "sort": sort, "desc": desc,
        "case": case, "path": path, "regex": regex, "word": word,
        "offset": 0, "limit": if count { 1 } else { PAGE.min(limit) },
    }))?;
    let total = first["total"].as_u64().unwrap_or(0);
    if count {
        println!("{total}");
        return Ok(if total > 0 { 0 } else { 1 });
    }
    if first["state"].as_str().is_some_and(|s| s != "ready") {
        eprintln!("trawlr: the index is still being {}; results may be incomplete", if first["state"] == "scanning" { "built" } else { "checked" });
    }

    let out = std::io::stdout();
    let mut out = std::io::BufWriter::new(out.lock());
    let want = total.min(limit);
    let mut page = first;
    let mut done = 0u64;
    loop {
        let rows = page["rows"].as_array().cloned().unwrap_or_default();
        if rows.is_empty() {
            break;
        }
        for r in &rows {
            if done >= want {
                break;
            }
            done += 1;
            if as_json {
                let _ = writeln!(out, "{r}");
                continue;
            }
            let full: Vec<u8> = match r["raw"].as_str() {
                Some(hex) => unhex(hex),
                None => {
                    let (p, n) = (r["p"].as_str().unwrap_or(""), r["n"].as_str().unwrap_or(""));
                    let sep = trawlr::index::SEP as char;
                    format!("{}{sep}{n}", if p == "/" { "" } else { p.trim_end_matches(sep) }).into_bytes()
                }
            };
            if long {
                let size = if r["d"].as_bool().unwrap_or(false) { "<dir>".to_string() } else { human(r["s"].as_u64().unwrap_or(0)) };
                let _ = write!(out, "{size:>8}  {}  ", local_time(r["m"].as_i64().unwrap_or(0)));
            }
            let _ = out.write_all(&full);
            // A closed pipe (`trawlr foo | head`) is a normal way to stop.
            if out.write_all(if null { b"\0" } else { b"\n" }).is_err() {
                return Ok(0);
            }
        }
        if done >= want {
            break;
        }
        page = c.call(json!({"op": "rows", "qid": 1, "offset": done, "limit": PAGE.min(want - done)}))?;
        if page["stale"].as_bool().unwrap_or(false) {
            eprintln!("trawlr: the index was reorganised while listing; run the search again for the rest");
            break;
        }
    }
    let _ = out.flush();
    Ok(if total > 0 { 0 } else { 1 })
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("trawlr: {e}");
            std::process::exit(2);
        }
    }
}
