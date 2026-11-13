#pragma once
#include <QKeySequence>
#include <QList>
#include <QString>

// In-window keyboard shortcuts the user can change (Tools > Options > Keyboard).
// A command can have several keys. Only the differences from
// each command's default are stored, so a default that changes in a later
// version still reaches everyone who never touched it.
struct CommandInfo {
    QString id;             // stable name, the settings key
    QString label;          // "File > Copy Full Path", for the options list
    QKeySequence defaultKey;
};

namespace Shortcuts {

using Keys = QList<QKeySequence>;

// The command's default as a list: empty if it has no default key.
Keys defaults(const QKeySequence &defaultKey);

// The keys the command currently has: the saved ones if any, else the default.
// A saved empty list means the user removed every shortcut on purpose.
Keys effective(const QString &id, const QKeySequence &defaultKey);

// Saves `keys` for the command; saving the default removes the override.
void save(const QString &id, const Keys &keys, const QKeySequence &defaultKey);
inline void save(const QString &id, const QKeySequence &key, const QKeySequence &defaultKey)
{
    save(id, key.isEmpty() ? Keys() : Keys{key}, defaultKey);
}

// "Ctrl+I, Ctrl+Alt+M" for showing a list of keys.
QString text(const Keys &keys);

} // namespace Shortcuts
