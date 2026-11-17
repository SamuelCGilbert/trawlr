#pragma once
#include <QString>

// Wayland (and, less strictly, X11) will not let a program that is not in
// the foreground take focus for itself, so a plain QWidget::raise() from a
// background process is ignored or only makes the taskbar entry flash.
//
// KDE's window manager can be asked to do it: the same scripting interface its
// own window switchers use. This runs there and does nothing elsewhere.
namespace WindowActivation {

// Asks KWin to activate this process's window whose title is `title`.
// Returns false if KWin is not there to ask.
bool viaKWin(const QString &title);

// macOS: make this the active application, so its window comes forward.
void activateApp();

// macOS: show the Dock icon (and menu bar) while a window is open, and drop
// them while Trawlr only waits in the menu bar for its hotkeys.
void setDockIcon(bool shown);

} // namespace WindowActivation
