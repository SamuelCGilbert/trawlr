#pragma once
#include <QKeySequence>
#include <QString>

// The system-wide key that opens Trawlr from any program. The desktop owns
// it, not this program, so it keeps working while no window is open. Only KDE
// Plasma lets a program set it (through the same service System Settings uses);
// elsewhere available() is false and the user binds a key to `trawlr`
// in their desktop's keyboard settings.
namespace GlobalHotkey {

// The three on Options > Keyboard. Each is an entry of the
// launcher's desktop file, so the desktop can run it with nothing open.
enum Action {
    ShowWindow,   // `trawlr`: bring the window forward (or open one)
    NewWindow,    // `trawlr --new-window`
    ToggleWindow, // `trawlr --toggle`: minimize the window if it is in front, else show it
};
constexpr int ActionCount = 3;

bool available();

// False if the installed launcher entry predates this action (re-run install.sh).
bool supports(Action a);

// The key bound now, or an empty sequence if none (or not available).
QKeySequence current(Action a);

// Repairs keys an earlier version saved where, or so that, they do not work,
// and takes over the keys of the launchers from before the rename (once).
// Cheap when there is nothing to move; call once at startup.
void migrate();

// While on, the desktop leaves every global shortcut key to the focused
// window, so a key recorder can hear keys that are bound globally (including
// the ones being changed). Always turn it off again.
void block(bool on);

// Binds `key` (empty removes the binding). On failure returns false and says
// why in `error`.
bool set(Action a, const QKeySequence &key, QString *error);

} // namespace GlobalHotkey
