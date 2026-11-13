<img src="assets/app-icon.svg" width="64" height="64" alt="">

# Trawlr

Instant file-name search for Linux with an index that is always current, in the spirit of
[voidtools' Everything](https://www.voidtools.com/) for Windows.

Type a few letters and every matching file and folder on the machine is listed before you finish the
word. Files appear in the results the moment they are created, and vanish when they are deleted, with
no rescanning and no "update database" step, however long the program has been closed.

Not affiliated with voidtools.

## How it works

voidtools' Everything is fast on Windows because it never walks the disk: it reads the NTFS change journal and
keeps an index in memory. This does the same with what Linux offers.

- **`trawlrd`** is a small system service. It holds every file and folder name in memory and
  watches each disk with a single filesystem-wide [fanotify](https://man7.org/linux/man-pages/man7/fanotify.7.html)
  mark, so the kernel reports every create, delete and rename no matter how many folders there are.
  (inotify, which most tools use, needs one watch per folder and runs out on a real home directory.)
- The index is saved at shutdown. At the next start it is loaded in a fraction of a second and
  checked against the disk in the background, so changes made while the service was not running
  are picked up too.
- **The window** (`trawlr-gui`) and **the `trawlr` command** are thin clients. They hold no index
  of their own and open instantly. `trawlr` on its own opens the window; `trawlr` followed by a
  search prints the matches in the terminal.

## Install

Needs Linux 5.9 or newer, systemd, a Rust toolchain, and Qt 6 for the window.

```sh
# Debian / Ubuntu / KDE neon
sudo apt install cargo build-essential qt6-base-dev
# Fedora:  sudo dnf install cargo gcc-c++ make qt6-qtbase-devel
# Arch:    sudo pacman -S rust base-devel qt6-base

./install.sh
```

`install.sh` builds as you, then asks for your sudo password once to install to `/usr/local`, write
`/etc/trawlrd/config.toml` and start the service. The first scan takes from a few seconds to a
few minutes depending on the disk; you can search while it runs.

Afterwards open **Trawlr** from the application menu. To get a search window on a hotkey, set one under **Tools → Options → Keyboard → Global hotkeys** (KDE
Plasma), or bind a key to the command `trawlr` in your desktop's keyboard settings.

`./install.sh --no-gui` installs only the service and `trawlr`. `./uninstall.sh` removes it again
(`--purge` also deletes the configuration and the saved index).

## Using it

| Key | Action |
| --- | --- |
| type | search as you type |
| `↓` / `↑` | move into and through the results |
| `Enter` | open with the default application |
| `Ctrl+N` | new window |
| `Ctrl+,` | options |
| `Ctrl+Enter` | open the containing folder with the item selected |
| `Shift+Enter` | open, then put the window away (to the tray, or minimized) |
| `Ctrl+T` | open a terminal in the item's folder |
| `Ctrl+Shift+T` | run the file in a terminal, which stays open afterwards so you can read the output |
| `Ctrl+C` | copy the files (paste them into a file manager) |
| `Ctrl+Shift+C` | copy the full path |
| `Alt+Enter` | properties |
| `Delete` | move to the trash (asks first) |
| `Ctrl+I` `Ctrl+B` `Ctrl+U` `Ctrl+R` | match case, whole word, path, regex |
| `Esc` | clear the search; close the window if it is already empty |

Double-click a result to open it, or double-click its path to open the folder it is in (this can be
changed under **Options → General**). Shift+double-click opens it and puts the window away. Click a
column header to sort; drag results out of the window into other applications. `Ctrl+M` shows the
menu bar; everything in it is also in the ☰ menu.

### One window or many

Starting Trawlr while a window is already open (from the hotkey, the launcher or a terminal)
brings that window to the front with its search selected, so you can type straight over it.
`trawlr --gui some words` puts those words in the search box of the window that comes forward.
Under **Tools → Options → General**, *Allow multiple windows* makes every start open another window
instead, and `trawlr --new-window` does that once, whatever the setting. **File → New Window**
(`Ctrl+N`) does the same from inside.

On KDE Plasma the window is brought forward by asking KWin to activate it, because Wayland does not
let a program that is not in the foreground raise itself. Other desktops get a plain raise request,
which they may only show as a flashing taskbar entry.

### Options

**Tools → Options** (`Ctrl+,`) has pages on the left (General, View,
Search, Terminal, Keyboard), and OK, Cancel, Apply and Restore Defaults (for the page you are on) at the
bottom. Nothing changes until OK or Apply. Press `/` anywhere in the window to jump to the search
box above the pages: it hides pages with nothing that matches, highlights the matching settings,
and on the Keyboard page narrows the command list to matching commands. `Enter` goes to the first
match, `Esc` clears the search.

- **General**: allow multiple windows, system tray icon, what double-clicking a path does.
- **View**: theme (System, Light, Dark), accent colour, row density, menu bar.
- **Search**: match case, whole word, path and regex for every open window, and whether a window
  brought to the front keeps its last search or starts empty.
- **Terminal**: which terminal Open/Run in Terminal use. *Automatic* picks your desktop's own
  (Konsole, GNOME Console, Ptyxis, …); any installed terminal or a custom command can be chosen.
- **Keyboard**: global hotkeys and the shortcuts in the window.

### Changing the keys

**Tools → Options → Keyboard** lists every command in the window with its shortcuts. Pick one and
use **Add…**, **Edit…** or **Remove**; a command can have several keys. The Add box says which
command has the key now; that command loses it. **Default** puts back one command's key, and
Restore Defaults all of them. `Enter`, `Esc` and the arrow keys have fixed meanings and are not
listed.

The same page holds three **global hotkeys**: *Show window* (`trawlr`),
*New window* (`trawlr --new-window`) and *Toggle window* (`trawlr --toggle`, which
hides the window if it is the active one and brings it forward otherwise). A hidden window is off
the screen, the taskbar and Alt+Tab, but Trawlr keeps running with an icon in the system tray:
click it to show or hide the window, or right-click it for New Window, Options and Exit. The icon
can be turned off under **General**. The desktop owns
these keys, so they work with no window open and show up in System Settings → Shortcuts as well.
Only KDE Plasma lets a program set them; on other desktops the boxes are disabled and you bind
keys to those commands yourself.

On the command line (`trawlr --help` lists every option):

```sh
trawlr invoice pdf                    # names containing both words
trawlr -l -s date -d dm:today         # what changed today, newest first, with size and time
trawlr -n 20 -s size -d 'size:>1gb'   # the 20 largest files (quote < and > for the shell)
trawlr --count ext:jpg                # how many
trawlr -0 ext:flac | xargs -0 du -ch | tail -1   # NUL-separated, for scripting
trawlr --status                       # what is indexed and how it is watched
```

## Search syntax

The syntax follows that of voidtools' Everything, so it will be familiar to its users.

| Search | Finds |
| --- | --- |
| `foo bar` | names containing both `foo` and `bar` |
| `foo\|bar` | either |
| `!foo` | not `foo` |
| `"foo bar"` | the phrase, space included |
| `<a\|b> c` | grouping |
| `*.pdf`, `inv?ice*` | wildcards; they must match the whole name |
| `docs/ report` | a term containing `/` is matched against the full path |
| `ext:jpg;png` | by extension |
| `size:>10mb`, `size:1mb..2gb`, `size:empty` | by size (`tiny` `small` `medium` `large` `huge` `gigantic`) |
| `dm:today`, `dm:2024-03`, `dm:last2weeks`, `dm:>2023` | by date modified (`yesterday` `thisweek` `lastmonth` `a..b`) |
| `file:` `folder:` | only files, only folders |
| `audio:` `video:` `pic:` `doc:` `zip:` `exe:` | by kind |
| `parent:/some/folder` | directly inside that folder |
| `empty:` | empty folders |
| `case:` `path:` `regex:` `ww:` `wfn:` `startwith:` `endwith:` | per-term: match case, match the path, regular expression, whole word, whole name, prefix, suffix |

Matching is case-insensitive unless asked otherwise. Results are sorted by name by default.

## Configuration

`/etc/trawlrd/config.toml` (see [`dist/config.toml`](dist/config.toml) for every option):

```toml
roots = ["/"]                                   # what to index
exclude = ["/home/timeshift", "/var/lib/docker"]  # folders to leave out
exclude_names = ["node_modules"]                # names to leave out everywhere
allowed_users = ["sam"]                         # who may search
```

Apply changes with `sudo systemctl restart trawlrd`.

Local disk filesystems are indexed across mount points automatically, and drives are added and
removed as they are mounted and unmounted. Virtual filesystems (`/proc`, tmpfs, overlay, snap
images) and network mounts are skipped unless listed in `extra_fs_types` or named as a root.
The installer excludes Timeshift and Snapper snapshot folders it finds, because they contain a
second copy of every file name on the system.

## Who can search

The service runs as root, since only root may watch a whole filesystem, and it therefore knows the
name of every file on the machine, including files in other users' homes. Names only: it never
reads file contents.

- Only users listed in `allowed_users` (plus root) can query it. The installer adds the user who
  ran it. A listed user can see names in folders they could not otherwise list, so on a shared
  machine treat that list like membership of an admin group.
- The service is confined by systemd to exactly what it needs: two capabilities
  (`CAP_SYS_ADMIN` for fanotify, `CAP_DAC_READ_SEARCH` to list folders), no network, and a read-only
  view of the system apart from its own state directory. See [`dist/trawlrd.service`](dist/trawlrd.service).

## Without root

`./install.sh --user` installs a per-user service instead. It indexes only your home folder and
needs no privileges, but the kernel then only allows one watch per folder, capped by
`fs.fanotify.max_user_marks` (typically a few hundred thousand, shared by all your programs) and
costing kernel memory for each. It suits a modest home directory. `trawlr --status` reports if folders
had to be left unwatched.

## What to expect

Measured on a Ryzen 5 7640U laptop with an NVMe SSD and 2.8 million files and folders indexed:

| | |
| --- | --- |
| First scan | 1.5 s with a warm cache, 8 s cold |
| Start with a saved index | 0.3 s to load, 0.5 s to check 268,000 folders against the disk |
| Typical search | 1 to 15 ms |
| Listing all 2.8 million entries, sorted by name | 9 ms |
| Sorting all of them by size or date | 55 to 90 ms |
| Memory | about 190 MB, roughly 70 bytes per entry |
| A new file showing up in an open window | under 0.2 s (the service knows within 15 ms) |

Things to know:

- **Size and date update when a file is closed**, not while it is still being written. A download
  shows its final size when it finishes.
- **Changes made while the service is not running** (another operating system, a rescue disk) are
  found at the next start if they added, removed or renamed something. A file that was only
  rewritten in place keeps its old size and date in the index until it changes again or you run
  `trawlr --rescan`. After a crash or power loss everything is re-read automatically.
- **Tested on ext4, btrfs and FAT**, all with filesystem-wide watching, on Linux 7.0. XFS, F2FS, ZFS,
  bcachefs and NTFS are indexed too but have not been tried; if a filesystem does not accept a
  filesystem-wide mark the service falls back to watching it folder by folder and says so in its
  log. A btrfs volume with several subvolumes mounted separately is untested.
- **Network and FUSE filesystems** can be indexed by naming them as roots, but changes made from
  other machines are not reported by the kernel, so they are only as fresh as the last rescan.
- The window opens, copies, trashes and shows properties. It does not rename files or search
  inside them.
- The index is limited to 4 GB of name text, which is in the region of 150 million files.

## Troubleshooting

```sh
systemctl status trawlrd      # is it running?
journalctl -u trawlrd -e      # what it has been doing
trawlr --status                # what it indexes and how each filesystem is watched
trawlr --rescan                # compare the whole index with the disk now
trawlr --rebuild               # throw the index away and start over
```

"this user is not allowed to search the system index": add the user to `allowed_users` in
`/etc/trawlrd/config.toml` and restart the service.

## Development

```sh
cargo test --release           # index, query language and sorting
python3 tests/stress.py        # random creates, renames, moves and deletes against a live daemon,
                               # comparing the index with the disk after every round
tests/vm/run.sh                # the same and more as root inside a throwaway QEMU VM: filesystem-wide
                               # watches on ext4/btrfs/FAT, hot-plugged drives, the systemd confinement

# the window itself, driven off screen against a real daemon
(mkdir -p gui/tests/build && cd gui/tests/build && qmake6 ../tests.pro && make && QT_QPA_PLATFORM=offscreen ./tst_gui)
```

The layout: [`src/index.rs`](src/index.rs) is the in-memory index, [`src/query.rs`](src/query.rs)
the search language and matcher, [`src/scan.rs`](src/scan.rs) reads directories,
[`src/watch.rs`](src/watch.rs) wraps fanotify, [`src/indexer.rs`](src/indexer.rs) is the single
writer that ties them together, [`src/server.rs`](src/server.rs) the socket protocol, and
[`gui/`](gui) the Qt client.

Created in [T3 Code](https://t3.codes).
