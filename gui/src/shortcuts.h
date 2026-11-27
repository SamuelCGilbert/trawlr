#pragma once
#include <QKeySequence>
#include <QList>
#include <QString>

// In-window keyboard shortcuts the user can change (Tools > Options > Keyboard).
// A command can have several keys, by default too. Only the differences from
// each command's default are stored, so a default that changes in a later
// version still reaches everyone who never touched it.
namespace Shortcuts {
using Keys = QList<QKeySequence>;
}

struct CommandInfo {
    QString id;             // stable name, the settings key
    QString label;          // "File > Copy Full Path", for the options list
    Shortcuts::Keys defaultKeys;
};

namespace Shortcuts {

// The keys the command currently has: the saved ones if any, else the default.
// A saved empty list means the user removed every shortcut on purpose.
Keys effective(const QString &id, const Keys &defaults);

// Saves `keys` for the command; saving the default removes the override.
void save(const QString &id, const Keys &keys, const Keys &defaults);

// "Ctrl+I, Ctrl+Alt+M" for showing a list of keys.
QString text(const Keys &keys);

} // namespace Shortcuts
