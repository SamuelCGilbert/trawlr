# Security

## Reporting a problem

Please email **security@trawlr.io** rather than opening a public issue. Say
what you found, how to reproduce it, and which version and system you used.
You will get an answer within a few days, and credit in the release notes if
you would like it.

## What Trawlr protects, and how

`trawlrd` runs with the highest privileges on the machine (root on Linux and
macOS, the system account on Windows), because only that lets it see every
file name. It never reads file contents, only names, sizes and dates. What it
guards is that list of names, which includes other users' files, and the
machine itself, which a fault in a privileged program puts at risk.

**Who may search.** Every connection is identified by the operating system,
not by anything the client says: the peer's user id on a Unix socket, the
client's access token (by impersonating it briefly) on the Windows pipe. Only
root or SYSTEM, the users and groups the configuration names (administrators
by default), and the user running the daemon get answers. The saved index is
readable by root or administrators only, the Windows pipe refuses remote
clients, and the Windows service drops every privilege it does not need. On
Linux, systemd confines the daemon to two capabilities, no network and a
read-only view of the system (see `dist/trawlrd.service`).

**What a client can do.** Search, page through results, ask for status, and
ask for a rescan (at most once a minute). Requests are single JSON lines of at
most 1 MB. Searches are limited in nesting depth, number of terms, path terms,
patterns and regular expression size, so no search can exhaust the stack or
memory. At most 64 connections are served at once; a Windows client must say
something within 10 seconds; a client that stops reading is dropped, and it
cannot slow down indexing or other clients.

**Clients trust only the real daemon.** The window and the `trawlr` command
check who answers before believing a socket: on Linux and macOS it must run as
root or as you, on Windows the pipe must be served from the services session.
So another local user cannot pose as the daemon and feed you false results.

**When it falls behind.** If changes arrive faster than they can be applied,
they are not queued without limit: past a bound the daemon checks the index
against the disk instead (Linux), or leaves them in the system's change
history until it catches up (Windows, macOS).

## How it is tested

Continuous integration runs, on Linux, Windows and macOS: the unit tests
(including damaged index files and hostile searches), a stress test that
compares the live index with the disk after random changes and restarts,
`tests/protocol.py` (malformed requests, oversized lines, hostile searches,
connection floods, a client that never reads), whole-disk live tests, and on
macOS an install, upgrade and uninstall of the real package. Dependencies are
checked with `cargo audit`.
