//! Search syntax, matching and sorting.
//!
//! The syntax follows voidtools' Everything: space is AND, `|` is OR (binding
//! tighter than AND), `!` negates, `< >` group, `"..."` quotes spaces, `*` and
//! `?` are wildcards that must match the whole name, a term containing `/`
//! matches against the full path, and `name:` prefixes are modifiers
//! (`case:`, `regex:`, `path:`, `ww:`, `file:`, `folder:` ...) or filters
//! (`ext:`, `size:`, `dm:`, `parent:` ...).
//!
//! A query runs in two steps. Matching produces a bitmap over entry ids, in
//! parallel; when the query has a plain-text term, that term is found with a
//! single substring search over the contiguous name buffer and only the hits
//! are checked against the rest of the query. The bitmap is then read out in
//! name order by walking the index's pre-sorted permutation, so no sort is
//! needed for the default order however many results there are.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU8, Ordering};

use memchr::memmem::Finder;
use rayon::prelude::*;
use regex::bytes::{Regex, RegexBuilder};

use crate::index::{Index, F_DEAD, F_ROOT, F_SYMLINK, NONE, SEP};
use crate::util::to_lower;

#[derive(Clone, Copy, Default, Debug)]
pub struct QueryOpts {
    pub case: bool,
    pub path: bool,
    pub regex: bool,
    pub word: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sort {
    Name,
    Path,
    Size,
    Mtime,
    Ext,
}

impl Sort {
    pub fn parse(s: &str) -> Sort {
        match s {
            "path" => Sort::Path,
            "size" => Sort::Size,
            "date" | "mtime" | "dm" => Sort::Mtime,
            "ext" | "extension" => Sort::Ext,
            _ => Sort::Name,
        }
    }
}

// ---- matchers ----------------------------------------------------------

enum NameKind {
    Substr(Finder<'static>),
    Prefix(Vec<u8>),
    Suffix(Vec<u8>),
    Exact(Vec<u8>),
    Word(Finder<'static>),
    Regex(Regex),
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b >= 0x80
}

impl NameKind {
    fn is_match(&self, s: &[u8]) -> bool {
        match self {
            NameKind::Substr(f) => f.find(s).is_some(),
            NameKind::Prefix(p) => s.starts_with(p),
            NameKind::Suffix(p) => s.ends_with(p),
            NameKind::Exact(p) => s == &p[..],
            NameKind::Word(f) => {
                let n = f.needle().len();
                f.find_iter(s).any(|at| (at == 0 || !is_word_byte(s[at - 1])) && (at + n == s.len() || !is_word_byte(s[at + n])))
            }
            NameKind::Regex(r) => r.is_match(s),
        }
    }
    /// Text that must appear in any name this matches.
    fn literal(&self) -> Option<&[u8]> {
        match self {
            NameKind::Substr(f) | NameKind::Word(f) => Some(f.needle()),
            NameKind::Prefix(p) | NameKind::Suffix(p) | NameKind::Exact(p) => Some(p),
            NameKind::Regex(_) => None,
        }
    }
}

/// A literal matched against the full path, without building any path.
///
/// The text is split at `/` into components `c1/../ck`. An entry "hits" when
/// its own name starts with `ck`, its ancestors are named `c(k-1)`..`c2`, and
/// the next one up ends with `c1`. An entry then matches if it hits or lies
/// beneath a directory that does, which is remembered per directory so each
/// one is examined once per query.
struct PathLit {
    slot: usize,
    exact: bool,
    comps: Vec<Vec<u8>>,
    /// `joined[j - 1]` is `c1/../cj`.
    joined: Vec<Vec<u8>>,
    full: Finder<'static>,
}

impl PathLit {
    fn new(slot: usize, text: &[u8], exact: bool) -> PathLit {
        let comps: Vec<Vec<u8>> = text.split(|&b| b == SEP).map(|c| c.to_vec()).collect();
        let joined = (1..=comps.len()).map(|j| comps[..j].join(&SEP)).collect();
        PathLit { slot, exact, comps, joined, full: Finder::new(text).into_owned() }
    }

    #[inline]
    fn nm<'a>(&self, idx: &'a Index, e: u32) -> &'a [u8] {
        if self.exact { idx.orig_name(e) } else { idx.lower_name(e) }
    }

    fn self_hit(&self, idx: &Index, x: u32) -> bool {
        let name = self.nm(idx, x);
        let k = self.comps.len();
        if idx.flags(x) & F_ROOT != 0 || k == 1 {
            return self.full.find(name).is_some();
        }
        if !name.starts_with(&self.comps[k - 1]) {
            return false;
        }
        let mut cur = idx.parent(x);
        let mut j = k - 1;
        loop {
            if cur == NONE {
                return false;
            }
            let n = self.nm(idx, cur);
            if idx.flags(cur) & F_ROOT != 0 {
                return n.ends_with(&self.joined[j - 1]);
            }
            if j == 1 {
                return n.ends_with(&self.comps[0]);
            }
            if n != &self.comps[j - 1][..] {
                return false;
            }
            cur = idx.parent(cur);
            j -= 1;
        }
    }

    /// Whether everything under directory `d` matches.
    fn inherit(&self, idx: &Index, d: u32, memo: Option<&[AtomicU8]>) -> bool {
        let mut chain = [0u32; 64];
        let mut n = 0;
        let mut cur = d;
        let res = loop {
            if cur == NONE {
                break false;
            }
            if let Some(m) = memo {
                match m[cur as usize].load(Ordering::Relaxed) {
                    1 => break false,
                    2 => break true,
                    _ => {}
                }
            }
            if self.self_hit(idx, cur) {
                if let Some(m) = memo {
                    m[cur as usize].store(2, Ordering::Relaxed);
                }
                break true;
            }
            if n < chain.len() {
                chain[n] = cur;
                n += 1;
            }
            cur = idx.parent(cur);
        };
        if let Some(m) = memo {
            for &c in &chain[..n] {
                m[c as usize].store(if res { 2 } else { 1 }, Ordering::Relaxed);
            }
        }
        res
    }

    fn is_match(&self, idx: &Index, x: u32, memo: Option<&[AtomicU8]>) -> bool {
        self.self_hit(idx, x) || self.inherit(idx, idx.parent(x), memo)
    }
}

#[allow(clippy::large_enum_variant)]
enum Leaf {
    Name { fold: Option<NameKind>, exact: Option<NameKind> },
    PathLit(PathLit),
    /// Regex or wildcard over the whole path; has to build each path.
    PathRe { re: Regex, exact: bool },
    Ext(Vec<Vec<u8>>),
    /// A folder or link with this extension (a macOS bundle such as `.app`).
    Bundle(Vec<u8>),
    Size(u64, u64),
    Mtime(u32, u32),
    File,
    Dir,
    EmptyDir,
    Parent(u32),
    /// Inside (or one of) these folders; see [`Compiled::excluding`].
    Under { slot: usize, dirs: Vec<u32> },
    All,
}

#[allow(clippy::large_enum_variant)]
enum Node {
    And(Vec<Node>),
    Or(Vec<Node>),
    Not(Box<Node>),
    Leaf(Leaf),
}

pub struct Compiled {
    root: Node,
    driver: Option<Finder<'static>>,
    path_leaves: usize,
    /// Index epoch the query was compiled against (`parent:` holds an id).
    pub epoch: u64,
}

thread_local! {
    static PATH_BUF: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn ext_of(name: &[u8]) -> &[u8] {
    match memchr::memrchr(b'.', name) {
        Some(p) if p > 0 => &name[p + 1..],
        _ => &[],
    }
}

struct Ctx<'a> {
    idx: &'a Index,
    memo: &'a [&'a [AtomicU8]],
}

fn eval(node: &Node, c: &Ctx, i: u32) -> bool {
    match node {
        Node::And(v) => v.iter().all(|n| eval(n, c, i)),
        Node::Or(v) => v.iter().any(|n| eval(n, c, i)),
        Node::Not(n) => !eval(n, c, i),
        Node::Leaf(l) => eval_leaf(l, c, i),
    }
}

fn eval_leaf(l: &Leaf, c: &Ctx, i: u32) -> bool {
    let idx = c.idx;
    match l {
        Leaf::Name { fold, exact } => {
            fold.as_ref().is_none_or(|k| k.is_match(idx.lower_name(i))) && exact.as_ref().is_none_or(|k| k.is_match(idx.orig_name(i)))
        }
        Leaf::PathLit(p) => p.is_match(idx, i, c.memo.get(p.slot).copied()),
        Leaf::PathRe { re, exact } => PATH_BUF.with(|b| {
            let mut b = b.borrow_mut();
            idx.match_path_into(i, *exact, &mut b);
            re.is_match(&b)
        }),
        Leaf::Ext(list) => {
            if idx.is_dir(i) {
                return false;
            }
            let e = ext_of(idx.lower_name(i));
            !e.is_empty() && list.iter().any(|x| x == e)
        }
        Leaf::Bundle(e) => {
            let n = idx.lower_name(i);
            (idx.is_dir(i) || idx.flags(i) & F_SYMLINK != 0) && n.len() > e.len() + 1 && n.ends_with(e) && n[n.len() - e.len() - 1] == b'.'
        }
        Leaf::Size(lo, hi) => !idx.is_dir(i) && (*lo..=*hi).contains(&idx.file_size(i)),
        Leaf::Mtime(lo, hi) => (*lo..=*hi).contains(&idx.mtime(i)),
        Leaf::File => !idx.is_dir(i),
        Leaf::Dir => idx.is_dir(i),
        Leaf::EmptyDir => idx.is_dir(i) && !idx.is_dead(i) && idx.children(i).is_empty(),
        Leaf::Parent(p) => idx.parent(i) == *p,
        Leaf::Under { slot, dirs } => under(idx, dirs, i, c.memo.get(*slot).copied()),
        Leaf::All => true,
    }
}

/// Whether `x` is one of `dirs` or inside one. The answer for each folder
/// on the way up is remembered (1 no, 2 yes), so each is looked at once.
fn under(idx: &Index, dirs: &[u32], x: u32, memo: Option<&[AtomicU8]>) -> bool {
    let mut chain = [0u32; 64];
    let mut n = 0;
    let mut cur = x;
    let res = loop {
        if cur == NONE {
            break false;
        }
        if dirs.contains(&cur) {
            break true;
        }
        if let Some(m) = memo {
            match m[cur as usize].load(Ordering::Relaxed) {
                1 => break false,
                2 => break true,
                _ => {}
            }
        }
        if n < chain.len() {
            chain[n] = cur;
            n += 1;
        }
        cur = idx.parent(cur);
    };
    if let Some(m) = memo {
        for &c in &chain[..n] {
            m[c as usize].store(if res { 2 } else { 1 }, Ordering::Relaxed);
        }
    }
    res
}

// ---- parsing -----------------------------------------------------------

enum Tok {
    Term { text: String, quoted_from: usize },
    Or,
    Not,
    Open,
    Close,
}

fn lex(s: &str) -> Vec<Tok> {
    let chars: Vec<char> = s.chars().collect();
    let mut toks = Vec::new();
    let mut depth = 0;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '|' {
            toks.push(Tok::Or);
            i += 1;
        } else if c == '!' {
            toks.push(Tok::Not);
            i += 1;
        } else if c == '<' {
            toks.push(Tok::Open);
            depth += 1;
            i += 1;
        } else if c == '>' && depth > 0 {
            toks.push(Tok::Close);
            depth -= 1;
            i += 1;
        } else {
            let mut text = String::new();
            let mut quoted_from = usize::MAX;
            let mut in_quote = false;
            while i < chars.len() {
                let c = chars[i];
                if c == '"' {
                    in_quote = !in_quote;
                    quoted_from = quoted_from.min(text.len());
                    i += 1;
                    continue;
                }
                if !in_quote {
                    if c.is_whitespace() {
                        break;
                    }
                    // Inside `regex:` the operators are part of the pattern,
                    // so `regex:\.(jpg|png)$` needs no quoting.
                    let in_regex = text.len() >= 6 && text.to_ascii_lowercase().contains("regex:");
                    // `>` closes a group unless it is a comparison, as in `size:>1mb`.
                    if !in_regex && (c == '|' || (c == '>' && depth > 0 && !text.ends_with([':', '<', '>', '=']))) {
                        break;
                    }
                }
                text.push(c);
                i += 1;
            }
            toks.push(Tok::Term { text, quoted_from });
        }
    }
    toks
}

#[derive(Clone, Copy)]
struct TermCtx {
    case: bool,
    path: bool,
    regex: bool,
    word: bool,
    whole: bool,
    start: bool,
    end: bool,
}

struct Parser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    opts: QueryOpts,
    idx: &'a Index,
    path_leaves: usize,
}

const AUDIO: &str = "aac;ac3;aif;aifc;aiff;au;cda;dts;fla;flac;it;m1a;m2a;m3u;m4a;mid;midi;mka;mod;mp2;mp3;mpa;ogg;opus;ra;rmi;spc;snd;umx;voc;wav;wma;xm";
const ZIP: &str = "7z;ace;arj;bz2;cab;gz;gzip;jar;rar;tar;tgz;z;zip;xz;zst;lz4;tbz2;txz";
const DOC: &str = "c;chm;cpp;csv;cxx;doc;docm;docx;dot;dotm;dotx;h;hpp;htm;html;hxx;ini;java;lua;md;mht;mhtml;odp;ods;odt;pdf;potx;potm;ppam;ppsm;ppsx;pps;ppt;pptm;pptx;rtf;sldm;sldx;thmx;txt;vsd;wpd;wps;wri;xlam;xls;xlsb;xlsm;xlsx;xltm;xltx;xml";
const EXE: &str = "appimage;bat;bin;cmd;deb;exe;flatpak;msi;msp;rpm;run;scr;sh";
const PIC: &str = "ani;avif;bmp;gif;heic;ico;jpe;jpeg;jpg;pcx;png;psd;svg;tga;tif;tiff;webp;wmf";
const VIDEO: &str = "3g2;3gp;3gp2;3gpp;amr;amv;asf;avi;bdmv;bik;d2v;divx;drc;dsa;dsm;dss;dsv;evo;f4v;flc;fli;flic;flv;hdmov;ifo;ivf;m1v;m2p;m2t;m2ts;m2v;m4b;m4p;m4v;mkv;mp2v;mp4;mp4v;mpe;mpeg;mpg;mpls;mpv2;mpv4;mov;mts;ogm;ogv;pss;pva;qt;ram;ratdvd;rm;rmm;rmvb;roq;smil;smk;swf;tp;tpr;ts;vob;vp6;webm;wm;wmp;wmv";

fn ext_list(s: &str) -> Leaf {
    Leaf::Ext(s.split(';').map(|e| to_lower(e.trim().trim_start_matches('.').as_bytes())).filter(|e| !e.is_empty()).collect())
}

fn wildcard_regex(pat: &str) -> String {
    let mut re = String::from("(?s)^");
    let mut lit = String::new();
    for ch in pat.chars() {
        if ch == '*' || ch == '?' {
            re.push_str(&regex::escape(&lit));
            lit.clear();
            re.push_str(if ch == '*' { ".*" } else { "." });
        } else {
            lit.push(ch);
        }
    }
    re.push_str(&regex::escape(&lit));
    re.push('$');
    re
}

fn build_regex(pat: &str, insensitive: bool) -> Result<Regex, String> {
    RegexBuilder::new(pat).case_insensitive(insensitive).size_limit(8 << 20).build().map_err(|e| {
        let msg = e.to_string();
        format!("regex: {}", msg.lines().last().unwrap_or("invalid pattern").trim())
    })
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn parse_and(&mut self) -> Result<Node, String> {
        let mut items = Vec::new();
        while let Some(t) = self.peek() {
            if matches!(t, Tok::Close) {
                break;
            }
            if matches!(t, Tok::Or) {
                self.pos += 1;
                continue;
            }
            items.push(self.parse_or()?);
        }
        Ok(match items.len() {
            0 => Node::Leaf(Leaf::All),
            1 => items.pop().unwrap(),
            _ => Node::And(items),
        })
    }

    fn parse_or(&mut self) -> Result<Node, String> {
        let mut alts = vec![self.parse_unary()?];
        while matches!(self.peek(), Some(Tok::Or)) {
            self.pos += 1;
            if matches!(self.peek(), None | Some(Tok::Close) | Some(Tok::Or)) {
                continue;
            }
            alts.push(self.parse_unary()?);
        }
        Ok(if alts.len() == 1 { alts.pop().unwrap() } else { Node::Or(alts) })
    }

    fn parse_unary(&mut self) -> Result<Node, String> {
        let Some(tok) = self.toks.get(self.pos) else { return Ok(Node::Leaf(Leaf::All)) };
        match tok {
            Tok::Not => {
                self.pos += 1;
                if matches!(self.peek(), None | Some(Tok::Close) | Some(Tok::Or)) {
                    return Ok(Node::Leaf(Leaf::All));
                }
                Ok(Node::Not(Box::new(self.parse_unary()?)))
            }
            Tok::Open => {
                self.pos += 1;
                let n = self.parse_and()?;
                if matches!(self.peek(), Some(Tok::Close)) {
                    self.pos += 1;
                }
                Ok(n)
            }
            Tok::Term { text, quoted_from } => {
                let (text, q) = (text.clone(), *quoted_from);
                self.pos += 1;
                self.term(&text, q)
            }
            Tok::Or | Tok::Close => Ok(Node::Leaf(Leaf::All)),
        }
    }

    fn term(&mut self, text: &str, quoted_from: usize) -> Result<Node, String> {
        let mut c = TermCtx { case: self.opts.case, path: self.opts.path, regex: self.opts.regex, word: self.opts.word, whole: false, start: false, end: false };
        let mut filters: Vec<Node> = Vec::new();
        let mut rest = text;
        let mut limit = quoted_from.min(text.len());
        while let Some(ci) = rest[..limit].find(':') {
            let key = rest[..ci].to_ascii_lowercase();
            let value = &rest[ci + 1..];
            match key.as_str() {
                "case" => c.case = true,
                "nocase" => c.case = false,
                "regex" => c.regex = true,
                "noregex" => c.regex = false,
                "path" => c.path = true,
                "nopath" => c.path = false,
                "ww" | "wholeword" | "wholewords" => c.word = true,
                "noww" | "nowholeword" => c.word = false,
                "wfn" | "wholefilename" | "exact" => c.whole = true,
                "startwith" => c.start = true,
                "endwith" => c.end = true,
                "file" | "files" => filters.push(Node::Leaf(Leaf::File)),
                "folder" | "folders" | "dir" => filters.push(Node::Leaf(Leaf::Dir)),
                "audio" => filters.push(Node::Leaf(ext_list(AUDIO))),
                "video" => filters.push(Node::Leaf(ext_list(VIDEO))),
                "pic" => filters.push(Node::Leaf(ext_list(PIC))),
                "doc" => filters.push(Node::Leaf(ext_list(DOC))),
                "zip" => filters.push(Node::Leaf(ext_list(ZIP))),
                // On a Mac a program is an .app bundle, which is a folder.
                "exe" if cfg!(target_os = "macos") => {
                    filters.push(Node::Or(vec![Node::Leaf(ext_list(EXE)), Node::Leaf(Leaf::Bundle(b"app".to_vec()))]))
                }
                "exe" => filters.push(Node::Leaf(ext_list(EXE))),
                "empty" => filters.push(Node::Leaf(Leaf::EmptyDir)),
                "ext" => {
                    filters.push(Node::Leaf(ext_list(value)));
                    rest = "";
                    break;
                }
                "size" => {
                    let (lo, hi) = parse_size(value)?;
                    filters.push(Node::Leaf(Leaf::Size(lo, hi)));
                    rest = "";
                    break;
                }
                "dm" | "datemodified" => {
                    let (lo, hi) = parse_date(value)?;
                    filters.push(Node::Leaf(Leaf::Mtime(lo, hi)));
                    rest = "";
                    break;
                }
                "parent" | "infolder" => {
                    let id = self.idx.resolve_path(value.as_bytes()).filter(|&d| self.idx.is_dir(d));
                    // An unknown folder has no children: match nothing.
                    filters.push(Node::Leaf(Leaf::Parent(id.unwrap_or(NONE - 1))));
                    rest = "";
                    break;
                }
                _ => break,
            }
            rest = value;
            limit = limit.saturating_sub(ci + 1);
        }
        if !rest.is_empty() {
            filters.push(Node::Leaf(self.text_leaf(rest, c)?));
        }
        Ok(match filters.len() {
            0 => Node::Leaf(Leaf::All),
            1 => filters.pop().unwrap(),
            _ => Node::And(filters),
        })
    }

    fn text_leaf(&mut self, t: &str, c: TermCtx) -> Result<Leaf, String> {
        let wild = t.contains(['*', '?']);
        let path_mode = c.path || (!c.regex && (t.contains('/') || (cfg!(windows) && t.contains('\\'))));
        // Windows paths are written with either slash.
        let slashed;
        let t = if cfg!(windows) && path_mode && !c.regex {
            slashed = t.replace('/', "\\");
            slashed.as_str()
        } else {
            t
        };
        if c.regex || (wild && path_mode) {
            let pat = if c.regex { t.to_string() } else { wildcard_regex(t) };
            let re = build_regex(&pat, !c.case)?;
            return Ok(if path_mode {
                Leaf::PathRe { re, exact: c.case }
            } else if c.case {
                Leaf::Name { fold: None, exact: Some(NameKind::Regex(re)) }
            } else {
                Leaf::Name { fold: Some(NameKind::Regex(re)), exact: None }
            });
        }
        if path_mode {
            let slot = self.path_leaves;
            self.path_leaves += 1;
            let text = if c.case { t.as_bytes().to_vec() } else { to_lower(t.as_bytes()) };
            return Ok(Leaf::PathLit(PathLit::new(slot, &text, c.case)));
        }
        let kind = |text: &[u8]| -> Result<NameKind, String> {
            let s = std::str::from_utf8(text).unwrap_or("");
            if wild {
                let stars = s.matches('*').count();
                let inner = s.trim_matches('*');
                let simple = !inner.contains(['*', '?']) && !inner.is_empty();
                return Ok(match (simple, s.starts_with('*'), s.ends_with('*'), stars) {
                    (true, true, true, 2) => NameKind::Substr(Finder::new(inner.as_bytes()).into_owned()),
                    (true, true, false, 1) => NameKind::Suffix(inner.as_bytes().to_vec()),
                    (true, false, true, 1) => NameKind::Prefix(inner.as_bytes().to_vec()),
                    _ => NameKind::Regex(build_regex(&wildcard_regex(s), false)?),
                });
            }
            Ok(if c.whole {
                NameKind::Exact(text.to_vec())
            } else if c.start {
                NameKind::Prefix(text.to_vec())
            } else if c.end {
                NameKind::Suffix(text.to_vec())
            } else if c.word {
                NameKind::Word(Finder::new(text).into_owned())
            } else {
                NameKind::Substr(Finder::new(text).into_owned())
            })
        };
        // The folded form is always checked first: it is the cheap, contiguous
        // one, and anything that matches exactly also matches when folded.
        let fold = kind(&to_lower(t.as_bytes()))?;
        let exact = if c.case { Some(kind(t.as_bytes())?) } else { None };
        Ok(Leaf::Name { fold: Some(fold), exact })
    }
}

fn parse_size_value(s: &str) -> Result<(u64, u64), String> {
    let s = s.trim().to_ascii_lowercase();
    let split = s.find(|ch: char| !(ch.is_ascii_digit() || ch == '.')).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let v: f64 = num.parse().map_err(|_| format!("size: cannot read {s:?}"))?;
    let mult: u64 = match unit {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1 << 10,
        "m" | "mb" | "mib" => 1 << 20,
        "g" | "gb" | "gib" => 1 << 30,
        "t" | "tb" | "tib" => 1 << 40,
        _ => return Err(format!("size: unknown unit {unit:?}")),
    };
    Ok(((v * mult as f64) as u64, mult))
}

fn parse_size(s: &str) -> Result<(u64, u64), String> {
    const KB: u64 = 1 << 10;
    const MB: u64 = 1 << 20;
    match s.to_ascii_lowercase().as_str() {
        "empty" => return Ok((0, 0)),
        "tiny" => return Ok((0, 10 * KB)),
        "small" => return Ok((10 * KB + 1, 100 * KB)),
        "medium" => return Ok((100 * KB + 1, MB)),
        "large" => return Ok((MB + 1, 16 * MB)),
        "huge" => return Ok((16 * MB + 1, 128 * MB)),
        "gigantic" => return Ok((128 * MB + 1, u64::MAX)),
        _ => {}
    }
    if let Some((a, b)) = s.split_once("..") {
        return Ok((parse_size_value(a)?.0, parse_size_value(b)?.0));
    }
    for (op, f) in [(">=", 0), ("<=", 1), (">", 2), ("<", 3), ("=", 4)] {
        if let Some(rest) = s.strip_prefix(op) {
            let v = parse_size_value(rest)?.0;
            return Ok(match f {
                0 => (v, u64::MAX),
                1 => (0, v),
                2 => (v.saturating_add(1), u64::MAX),
                3 => (0, v.saturating_sub(1)),
                _ => (v, v),
            });
        }
    }
    // `size:2mb` means "about 2 MB": anything that would display as such.
    let (v, unit) = parse_size_value(s)?;
    Ok((v, v.saturating_add(unit - 1)))
}

pub fn local_tm(t: i64) -> libc::tm {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let tt = t as libc::time_t;
    #[cfg(unix)]
    unsafe {
        libc::localtime_r(&tt, &mut tm)
    };
    #[cfg(windows)]
    unsafe {
        libc::localtime_s(&mut tm, &tt)
    };
    tm
}

#[cfg(windows)]
extern "C" {
    #[link_name = "_mktime64"]
    fn mktime(tm: *mut libc::tm) -> i64;
}
#[cfg(unix)]
use libc::mktime;

/// Local midnight of a calendar date; out-of-range months and days roll over.
fn mk(year: i32, mon0: i32, day: i32) -> i64 {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = mon0;
    tm.tm_mday = day;
    tm.tm_isdst = -1;
    unsafe { mktime(&mut tm) as i64 }
}

/// A date expression as a half-open interval of seconds.
fn date_interval(s: &str, now: i64) -> Result<(i64, i64), String> {
    let s = s.trim().to_ascii_lowercase();
    let tm = local_tm(now);
    let (y, m, d) = (tm.tm_year + 1900, tm.tm_mon, tm.tm_mday);
    let monday = d - (tm.tm_wday + 6) % 7;
    Ok(match s.as_str() {
        "today" => (mk(y, m, d), mk(y, m, d + 1)),
        "yesterday" => (mk(y, m, d - 1), mk(y, m, d)),
        "thisweek" => (mk(y, m, monday), mk(y, m, monday + 7)),
        "lastweek" => (mk(y, m, monday - 7), mk(y, m, monday)),
        "thismonth" => (mk(y, m, 1), mk(y, m + 1, 1)),
        "lastmonth" => (mk(y, m - 1, 1), mk(y, m, 1)),
        "thisyear" => (mk(y, 0, 1), mk(y + 1, 0, 1)),
        "lastyear" => (mk(y - 1, 0, 1), mk(y, 0, 1)),
        _ => {
            if let Some(rest) = s.strip_prefix("last") {
                let split = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
                let n: i64 = if split == 0 { 1 } else { rest[..split].parse().map_err(|_| format!("dm: cannot read {s:?}"))? };
                let secs = match rest[split..].trim_end_matches('s') {
                    "sec" | "second" => 1,
                    "min" | "minute" => 60,
                    "hour" => 3600,
                    "day" => 86400,
                    "week" => 7 * 86400,
                    "month" => 30 * 86400,
                    "year" => 365 * 86400,
                    u => return Err(format!("dm: unknown unit {u:?}")),
                };
                return Ok((now - n * secs, now + 1));
            }
            let parts: Vec<&str> = s.split(['-', '/']).collect();
            let num = |i: usize| parts[i].parse::<i32>().map_err(|_| format!("dm: cannot read {s:?}"));
            match parts.len() {
                1 => (mk(num(0)?, 0, 1), mk(num(0)? + 1, 0, 1)),
                2 => (mk(num(0)?, num(1)? - 1, 1), mk(num(0)?, num(1)?, 1)),
                3 => (mk(num(0)?, num(1)? - 1, num(2)?), mk(num(0)?, num(1)? - 1, num(2)? + 1)),
                _ => return Err(format!("dm: cannot read {s:?}")),
            }
        }
    })
}

fn parse_date(s: &str) -> Result<(u32, u32), String> {
    let now = crate::util::now_secs();
    let clamp = |v: i64| v.clamp(0, u32::MAX as i64) as u32;
    let (lo, hi) = if let Some((a, b)) = s.split_once("..") {
        (date_interval(a, now)?.0, date_interval(b, now)?.1)
    } else if let Some(r) = s.strip_prefix(">=") {
        (date_interval(r, now)?.0, i64::MAX)
    } else if let Some(r) = s.strip_prefix("<=") {
        (0, date_interval(r, now)?.1)
    } else if let Some(r) = s.strip_prefix('>') {
        (date_interval(r, now)?.1, i64::MAX)
    } else if let Some(r) = s.strip_prefix('<') {
        (0, date_interval(r, now)?.0)
    } else {
        date_interval(s.strip_prefix('=').unwrap_or(s), now)?
    };
    Ok((clamp(lo), clamp(hi.saturating_sub(1))))
}

/// The longest plain text that every match must contain in its name.
fn pick_driver(node: &Node) -> Option<Vec<u8>> {
    match node {
        Node::Leaf(Leaf::Name { fold: Some(k), .. }) => k.literal().filter(|l| !l.is_empty()).map(|l| l.to_vec()),
        Node::Leaf(Leaf::Ext(list)) if list.len() == 1 => {
            let mut v = vec![b'.'];
            v.extend_from_slice(&list[0]);
            Some(v)
        }
        Node::And(items) => items.iter().filter_map(pick_driver).max_by_key(|l| l.len()),
        _ => None,
    }
}

pub fn compile(text: &str, opts: QueryOpts, idx: &Index) -> Result<Compiled, String> {
    // With the regex option on, the whole search box is one pattern.
    let toks = if opts.regex && !text.trim().is_empty() { vec![Tok::Term { text: text.to_string(), quoted_from: 0 }] } else { lex(text) };
    let mut p = Parser { toks, pos: 0, opts, idx, path_leaves: 0 };
    let mut root = p.parse_and()?;
    // Stray closers: keep going so `a > b` still searches for both.
    while p.pos < p.toks.len() {
        p.pos += 1;
        let more = p.parse_and()?;
        root = Node::And(vec![root, more]);
    }
    let driver = pick_driver(&root).map(|l| Finder::new(&l).into_owned());
    Ok(Compiled { root, driver, path_leaves: p.path_leaves, epoch: idx.epoch })
}

impl Compiled {
    /// Leave out these folders and everything in them. Folders that do not
    /// exist (now) are ignored.
    pub fn excluding(mut self, folders: &[String], idx: &Index) -> Compiled {
        let dirs: Vec<u32> = folders.iter().filter_map(|f| idx.resolve_path(f.as_bytes())).filter(|&d| idx.is_dir(d)).collect();
        if !dirs.is_empty() {
            let slot = self.path_leaves;
            self.path_leaves += 1;
            let root = std::mem::replace(&mut self.root, Node::And(Vec::new()));
            self.root = Node::And(vec![root, Node::Not(Box::new(Node::Leaf(Leaf::Under { slot, dirs })))]);
        }
        self
    }
}

// ---- execution ---------------------------------------------------------

const CHUNK_WORDS: usize = 512;

fn scan_range(idx: &Index, q: &Compiled, c: &Ctx, a: usize, b: usize, words: &mut [u64]) {
    let skip = F_DEAD | F_ROOT;
    let mut hit = |i: usize| {
        if idx.flags[i] & skip == 0 && eval(&q.root, c, i as u32) {
            words[(i - a) / 64] |= 1u64 << ((i - a) % 64);
        }
    };
    let Some(finder) = &q.driver else {
        (a..b).for_each(&mut hit);
        return;
    };
    let offs = &idx.name_off;
    let blob = &idx.lower;
    let end = offs[b] as usize;
    let mut cur = a;
    while cur < b {
        let pos = offs[cur] as usize;
        let Some(m) = finder.find(&blob[pos..end]) else { break };
        let at = (pos + m) as u32;
        // Find the entry owning offset `at`: gallop forward, then bisect.
        let mut lo = cur;
        let mut step = 1;
        while lo + step < b && offs[lo + step] <= at {
            lo += step;
            step *= 2;
        }
        let hi = (lo + step).min(b);
        let i = lo + offs[lo..hi].partition_point(|&o| o <= at) - 1;
        hit(i);
        cur = i + 1;
    }
}

fn ext_key(name: &[u8]) -> u64 {
    let e = ext_of(name);
    let mut k = [0u8; 8];
    let l = e.len().min(8);
    k[..l].copy_from_slice(&e[..l]);
    u64::from_be_bytes(k)
}

/// Run a query, returning matching entry ids in the requested order
/// (ascending; the caller reverses for descending).
pub fn execute(idx: &Index, q: &Compiled, sort: Sort) -> Vec<u32> {
    let n = idx.len() as usize;
    if n == 0 {
        return Vec::new();
    }
    // One byte of scratch per entry for each path term, zero pages until touched.
    let mut memo_store: Vec<Vec<u8>> = (0..q.path_leaves).map(|_| vec![0u8; n]).collect();
    let memo: Vec<&[AtomicU8]> =
        memo_store.iter_mut().map(|v| unsafe { std::slice::from_raw_parts(v.as_mut_ptr() as *const AtomicU8, v.len()) }).collect();
    let ctx = Ctx { idx, memo: &memo };

    let mut bits = vec![0u64; n.div_ceil(64)];
    bits.par_chunks_mut(CHUNK_WORDS).enumerate().for_each(|(ci, words)| {
        let a = ci * CHUNK_WORDS * 64;
        let b = (a + words.len() * 64).min(n);
        scan_range(idx, q, &ctx, a, b, words);
    });
    let bit = |i: u32| bits[i as usize / 64] >> (i % 64) & 1 != 0;
    let collect = |from: usize| -> Vec<u32> {
        let mut out = Vec::new();
        for (w, &word) in bits.iter().enumerate().skip(from / 64) {
            let mut x = word;
            while x != 0 {
                let i = w * 64 + x.trailing_zeros() as usize;
                if i >= from {
                    out.push(i as u32);
                }
                x &= x - 1;
            }
        }
        out
    };

    let count: usize = bits.par_iter().map(|w| w.count_ones() as usize).sum();
    let upto = idx.sorted_upto as usize;
    let mut ids = if count <= 2048 {
        let mut v = collect(0);
        idx.sort_by_name(&mut v);
        v
    } else {
        // The permutation is already in name order: keep the ids whose bit is set.
        let main: Vec<u32> = idx.perm.par_iter().copied().filter(|&i| bit(i)).collect();
        let mut delta = collect(upto);
        if delta.is_empty() {
            main
        } else {
            idx.sort_by_name(&mut delta);
            idx.merge_sorted(&main, &delta)
        }
    };

    // Stable sorts on top of name order, so ties stay alphabetical.
    match sort {
        Sort::Name => {}
        Sort::Size => ids.par_sort_by_cached_key(|&i| idx.file_size(i)),
        Sort::Mtime => ids.par_sort_by_cached_key(|&i| idx.mtime(i)),
        Sort::Ext => ids.par_sort_by_cached_key(|&i| if idx.is_dir(i) { 0 } else { ext_key(idx.lower_name(i)) }),
        Sort::Path => {
            let ranks = idx.dir_ranks();
            ids.par_sort_by_cached_key(|&i| {
                let p = idx.parent(i);
                if p == NONE || idx.is_dead(p) { 0 } else { ranks[idx.dir_slot(p)] }
            });
        }
    }
    ids
}

/// Whether one entry satisfies the query. Works on tombstones too, which is
/// how a session learns that something it was showing went away.
pub fn matches_entry(idx: &Index, q: &Compiled, id: u32) -> bool {
    if q.epoch != idx.epoch || id >= idx.len() {
        return true;
    }
    if idx.flags(id) & F_ROOT != 0 {
        return false;
    }
    eval(&q.root, &Ctx { idx, memo: &[] }, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::to_lower;

    /// A small tree; returns the index with its name order installed.
    fn sample() -> Index {
        let mut idx = Index::new();
        let root = idx.add_root(b"/", 0, b"r");
        let mut h = 0u32;
        let mut dir = |idx: &mut Index, p: u32, n: &str| {
            h += 1;
            idx.add_dir(p, n.as_bytes(), &to_lower(n.as_bytes()), 0, 1_700_000_000, 0, &h.to_le_bytes(), true)
        };
        let home = dir(&mut idx, root, "home");
        let user = dir(&mut idx, home, "sal");
        let dl = dir(&mut idx, user, "Downloads");
        let src = dir(&mut idx, user, "src");
        let proj = dir(&mut idx, src, "project");
        dir(&mut idx, user, "Empty");
        let file = |idx: &mut Index, p: u32, n: &str, size: u64, mtime: u32| {
            idx.add_file(p, n.as_bytes(), &to_lower(n.as_bytes()), 0, size, mtime, true);
        };
        file(&mut idx, dl, "Invoice 2024.PDF", 300_000, 1_700_000_000);
        file(&mut idx, dl, "holiday.jpg", 4_000_000, 1_600_000_000);
        file(&mut idx, dl, "song.mp3", 5_000_000, 1_650_000_000);
        file(&mut idx, proj, "main.rs", 1200, 1_710_000_000);
        file(&mut idx, proj, "README.md", 800, 1_705_000_000);
        file(&mut idx, proj, "Makefile", 0, 1_705_000_000);
        file(&mut idx, user, "notes.txt", 50, 1_690_000_000);
        file(&mut idx, user, "invoice_draft.txt", 70, 1_690_000_001);
        let (perm, upto) = idx.merged_perm();
        idx.install_perm(perm, upto);
        idx
    }

    fn run_opts(idx: &Index, q: &str, opts: QueryOpts, sort: Sort) -> Vec<String> {
        let c = compile(q, opts, idx).unwrap();
        execute(idx, &c, sort).iter().map(|&i| String::from_utf8(idx.orig_name(i).to_vec()).unwrap()).collect()
    }
    fn run(idx: &Index, q: &str) -> Vec<String> {
        run_opts(idx, q, QueryOpts::default(), Sort::Name)
    }

    #[test]
    fn plain_terms() {
        let idx = sample();
        assert_eq!(run(&idx, "invoice"), ["Invoice 2024.PDF", "invoice_draft.txt"]);
        assert_eq!(run(&idx, "INVOICE pdf"), ["Invoice 2024.PDF"]);
        assert_eq!(run(&idx, "\"invoice 2024\""), ["Invoice 2024.PDF"]);
        assert_eq!(run(&idx, "zzz"), [] as [&str; 0]);
        // Everything, minus the root itself, in name order.
        assert_eq!(run(&idx, "").len(), 14);
        assert_eq!(run(&idx, "")[0], "Downloads");
    }

    #[test]
    fn operators() {
        let idx = sample();
        assert_eq!(run(&idx, "main|notes"), ["main.rs", "notes.txt"]);
        assert_eq!(run(&idx, "invoice !pdf"), ["invoice_draft.txt"]);
        // OR binds tighter than AND.
        assert_eq!(run(&idx, "txt notes|draft"), ["invoice_draft.txt", "notes.txt"]);
        assert_eq!(run(&idx, "<main|notes> !rs"), ["notes.txt"]);
    }

    #[test]
    fn wildcards_and_modifiers() {
        let idx = sample();
        assert_eq!(run(&idx, "*.rs"), ["main.rs"]);
        assert_eq!(run(&idx, "m*"), ["main.rs", "Makefile"]);
        assert_eq!(run(&idx, "?ain.rs"), ["main.rs"]);
        // A wildcard must match the whole name.
        assert_eq!(run(&idx, "ain*"), [] as [&str; 0]);
        assert_eq!(run(&idx, "case:README"), ["README.md"]);
        assert_eq!(run(&idx, "case:readme"), [] as [&str; 0]);
        assert_eq!(run(&idx, "regex:^[mn].*\\.(rs|txt)$"), ["main.rs", "notes.txt"]);
        assert_eq!(run(&idx, "ww:invoice"), ["Invoice 2024.PDF", "invoice_draft.txt"]);
        assert_eq!(run(&idx, "ww:invo"), [] as [&str; 0]);
        assert_eq!(run(&idx, "wfn:makefile"), ["Makefile"]);
        assert_eq!(run(&idx, "folder:s"), ["Downloads", "sal", "src"]);
        assert!(!run(&idx, "file:s").contains(&"src".to_string()));
        assert_eq!(run(&idx, "empty:"), ["Empty"]);
        assert!(compile("regex:(", QueryOpts::default(), &idx).is_err());
        // The regex option makes the whole box one pattern, spaces included.
        let re = QueryOpts { regex: true, ..Default::default() };
        assert_eq!(run_opts(&idx, "^invoice \\d+\\.pdf$", re, Sort::Name), ["Invoice 2024.PDF"]);
    }

    #[test]
    fn filters() {
        let idx = sample();
        assert_eq!(run(&idx, "ext:rs;md"), ["main.rs", "README.md"]);
        assert_eq!(run(&idx, "ext:pdf"), ["Invoice 2024.PDF"]);
        assert_eq!(run(&idx, "audio:"), ["song.mp3"]);
        assert_eq!(run(&idx, "pic: holiday"), ["holiday.jpg"]);
        assert_eq!(run(&idx, "size:>1mb"), ["holiday.jpg", "song.mp3"]);
        assert_eq!(run(&idx, "size:1kb..1mb"), ["Invoice 2024.PDF", "main.rs"]);
        assert_eq!(run(&idx, "<size:>4mb>"), ["song.mp3"]);
        assert_eq!(run(&idx, "size:empty file:"), ["Makefile"]);
        assert_eq!(run(&idx, "dm:2020"), ["holiday.jpg"]);
        assert_eq!(run(&idx, "dm:>2023 file:"), ["main.rs", "Makefile", "README.md"]);
        assert_eq!(run(&idx, "parent:/home/sal file:"), ["invoice_draft.txt", "notes.txt"]);
        assert_eq!(run(&idx, "parent:/nowhere"), [] as [&str; 0]);
    }

    #[test]
    fn path_matching() {
        let idx = sample();
        assert_eq!(run(&idx, "downloads/"), ["holiday.jpg", "Invoice 2024.PDF", "song.mp3"]);
        assert_eq!(run(&idx, "downloads/ pdf"), ["Invoice 2024.PDF"]);
        assert_eq!(run(&idx, "src/project/ma"), ["main.rs", "Makefile"]);
        assert_eq!(run(&idx, "rc/proj"), ["main.rs", "Makefile", "project", "README.md"]);
        assert_eq!(run(&idx, "/home/sal/n"), ["notes.txt"]);
        assert_eq!(run(&idx, "/sal/src"), ["main.rs", "Makefile", "project", "README.md", "src"]);
        assert_eq!(run(&idx, "x/home"), [] as [&str; 0]);
        // `path:` and the match-path option search every component.
        assert_eq!(run(&idx, "path:project file:"), ["main.rs", "Makefile", "README.md"]);
        let opts = QueryOpts { path: true, ..Default::default() };
        assert_eq!(run_opts(&idx, "downloads song", opts, Sort::Name), ["song.mp3"]);
        assert_eq!(run(&idx, "*/project/*.rs"), ["main.rs"]);
        // A regular expression sees the path as the system writes it.
        let re = if cfg!(windows) { r"regex:path:^\\home\\sal\\[^\\]+\.txt$" } else { r"regex:path:^/home/sal/[^/]+\.txt$" };
        assert_eq!(run(&idx, re), ["invoice_draft.txt", "notes.txt"]);
    }

    #[test]
    fn excluded_folders() {
        let idx = sample();
        let ex = |q: &str, folders: &[&str]| -> Vec<String> {
            let folders: Vec<String> = folders.iter().map(|f| f.replace('/', std::str::from_utf8(&[SEP]).unwrap())).collect();
            let c = compile(q, QueryOpts::default(), &idx).unwrap().excluding(&folders, &idx);
            execute(&idx, &c, Sort::Name).into_iter().map(|i| String::from_utf8_lossy(idx.orig_name(i)).into_owned()).collect()
        };
        assert_eq!(ex("o", &[]), run(&idx, "o"));
        // The folder itself goes too, and folders that are not there are ignored.
        assert_eq!(ex("project", &["/home/sal/src"]), [] as [&str; 0]);
        assert_eq!(ex("file:", &["/home/sal/src", "/home/sal/nope"]), run(&idx, "file: !path:src/"));
        assert_eq!(ex("ma", &["/home/sal/src/project"]), [] as [&str; 0]);
        assert_eq!(ex("ma", &["/home/sal/downloads"]), ["main.rs", "Makefile"]);
    }

    #[test]
    fn sorting() {
        let idx = sample();
        let by = |s| run_opts(&idx, "file:", QueryOpts::default(), s);
        assert_eq!(by(Sort::Size)[..3], ["Makefile", "notes.txt", "invoice_draft.txt"]);
        assert_eq!(by(Sort::Size).last().unwrap(), "song.mp3");
        assert_eq!(by(Sort::Mtime)[0], "holiday.jpg");
        assert_eq!(by(Sort::Mtime).last().unwrap(), "main.rs");
        // Ties keep name order: Makefile and README.md share a timestamp.
        let m = by(Sort::Mtime);
        let (a, b) = (m.iter().position(|x| x == "Makefile").unwrap(), m.iter().position(|x| x == "README.md").unwrap());
        assert_eq!(a + 1, b);
        assert_eq!(by(Sort::Ext)[0], "Makefile");
        // Tree order: a folder's own files, then its subfolders' contents.
        assert_eq!(by(Sort::Path)[..5], ["invoice_draft.txt", "notes.txt", "holiday.jpg", "Invoice 2024.PDF", "song.mp3"]);
    }

    #[test]
    fn unsorted_delta_and_tombstones() {
        let mut idx = sample();
        let user = idx.resolve_path(b"/home/sal").unwrap();
        // Enough new entries to take the permutation-walk path.
        for i in 0..3000 {
            let n = format!("bulk{i:05}.dat");
            idx.add_file(user, n.as_bytes(), n.as_bytes(), 0, 1, 1, true);
        }
        idx.add_file(user, b"aaa-first.dat", b"aaa-first.dat", 0, 1, 1, true);
        let r = run(&idx, "dat|invoice");
        assert_eq!(r.len(), 3003);
        assert_eq!(r[0], "aaa-first.dat");
        assert_eq!(r[1], "bulk00000.dat");
        assert_eq!(r.last().unwrap(), "invoice_draft.txt");

        let victim = idx.resolve_path(b"/home/sal/Downloads").unwrap();
        let c = compile("song", QueryOpts::default(), &idx).unwrap();
        let song = execute(&idx, &c, Sort::Name)[0];
        let mut t = crate::index::Touched::default();
        idx.remove_subtree(victim, &mut t);
        assert_eq!(run(&idx, "song"), [] as [&str; 0]);
        // The tombstone still answers "would this have matched?".
        assert!(matches_entry(&idx, &c, song));
        assert!(t.ids.contains(&song));
    }
}
