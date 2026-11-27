#include "shortcuts.h"

#include <QSettings>
#include <QStringList>

static QString settingsKey(const QString &id)
{
    return QStringLiteral("shortcuts/") + id;
}

Shortcuts::Keys Shortcuts::effective(const QString &id, const Keys &defaults)
{
    QSettings s;
    if (!s.contains(settingsKey(id)))
        return defaults;
    // Older versions stored one key; that parses as a list of one.
    Keys keys;
    for (const QKeySequence &k : QKeySequence::listFromString(s.value(settingsKey(id)).toString(), QKeySequence::PortableText))
        if (!k.isEmpty() && !keys.contains(k))
            keys.append(k);
    return keys;
}

void Shortcuts::save(const QString &id, const Keys &keys, const Keys &defaults)
{
    QSettings s;
    if (keys == defaults)
        s.remove(settingsKey(id));
    else
        s.setValue(settingsKey(id), QKeySequence::listToString(keys, QKeySequence::PortableText));
}

QString Shortcuts::text(const Keys &keys)
{
    QStringList parts;
    for (const QKeySequence &k : keys)
        parts << k.toString(QKeySequence::NativeText);
    return parts.join(QStringLiteral(", "));
}
