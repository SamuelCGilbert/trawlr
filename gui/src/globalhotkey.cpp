#include "globalhotkey.h"

#include <QCoreApplication>
#include <QProcess>
#include <QStandardPaths>
#include <QDBusArgument>
#include <QDBusConnection>
#include <QDBusConnectionInterface>
#include <QDBusMessage>
#include <QDBusObjectPath>
#include <QFile>
#include <QGuiApplication>
#include <QList>
#include <QSettings>
#include <QStringList>

static const char *const kService = "org.kde.kglobalaccel";
static const char *const kPath = "/kglobalaccel";
static const char *const kInterface = "org.kde.KGlobalAccel";

// Each hotkey runs a desktop file's main command ("_launch", what System
// Settings > Shortcuts > Add Application creates): the launcher itself for
// Show, and a hidden entry each for the others. Not desktop actions of the
// launcher: KDE reads those once per session, so one added by an update does
// nothing until the next login.
static QString desktopFile(GlobalHotkey::Action a)
{
    QString suffix;
    if (a == GlobalHotkey::NewWindow)
        suffix = QStringLiteral("-new-window");
    else if (a == GlobalHotkey::ToggleWindow)
        suffix = QStringLiteral("-toggle");
    return QGuiApplication::desktopFileName() + suffix + QStringLiteral(".desktop");
}

static QStringList actionId(GlobalHotkey::Action a)
{
    QString name = QGuiApplication::applicationDisplayName();
    if (a == GlobalHotkey::NewWindow)
        name = QCoreApplication::translate("GlobalHotkey", "Trawlr: New Window");
    else if (a == GlobalHotkey::ToggleWindow)
        name = QCoreApplication::translate("GlobalHotkey", "Trawlr: Show or Hide Window");
    return {desktopFile(a), QStringLiteral("_launch"), name, name};
}

// Where these keys were kept before the program was renamed: the launcher
// entries of "Everything for Linux".
static QStringList legacyId(GlobalHotkey::Action a)
{
    static const char *const files[] = {"everything-for-linux.desktop", "everything-for-linux-new-window.desktop",
                                        "everything-for-linux-toggle.desktop"};
    const QString file = QLatin1String(files[a]);
    return {file, QStringLiteral("_launch"), file, file};
}

// The key KDE saved for a launcher entry, read from its settings file: once
// the entry's desktop file is gone (an update removes the old ones), the
// service may no longer answer for it.
static QKeySequence savedLaunchKey(const QString &desktopFile)
{
    QFile f(QStandardPaths::writableLocation(QStandardPaths::GenericConfigLocation) + QStringLiteral("/kglobalshortcutsrc"));
    if (!f.open(QIODevice::ReadOnly | QIODevice::Text))
        return {};
    const QByteArray header = "[services][" + desktopFile.toUtf8() + ']';
    bool inside = false;
    while (!f.atEnd()) {
        const QByteArray line = f.readLine().trimmed();
        if (line.startsWith('[')) {
            inside = line == header;
            continue;
        }
        if (inside && line.startsWith("_launch=")) {
            // Several keys are separated by tabs; the first is the main one.
            const QString first = QString::fromUtf8(line.mid(8)).section(QLatin1Char('\t'), 0, 0);
            return first == QLatin1String("none") ? QKeySequence() : QKeySequence::fromString(first, QKeySequence::PortableText);
        }
    }
    return {};
}

static QDBusMessage call(const QString &method, const QVariantList &args)
{
    QDBusMessage m = QDBusMessage::createMethodCall(QLatin1String(kService), QLatin1String(kPath), QLatin1String(kInterface), method);
    m.setArguments(args);
    return QDBusConnection::sessionBus().call(m, QDBus::Block, 2000);
}

// KDE keeps a key as an int: Qt::Key plus modifier bits, the same as
// QKeyCombination::toCombined().
static QList<int> keysOf(const QDBusMessage &reply)
{
    QList<int> keys;
    if (reply.type() != QDBusMessage::ReplyMessage || reply.arguments().isEmpty())
        return keys;
    const QDBusArgument arg = reply.arguments().first().value<QDBusArgument>();
    arg.beginArray();
    while (!arg.atEnd()) {
        int k = 0;
        arg >> k;
        keys.append(k);
    }
    arg.endArray();
    return keys;
}

bool GlobalHotkey::available()
{
    QDBusConnectionInterface *bus = QDBusConnection::sessionBus().interface();
    return bus && bus->isServiceRegistered(QLatin1String(kService));
}

// Whether the desktop is listening for the keys of this entry.
static bool listening(const QString &component)
{
    const QDBusMessage path = call(QStringLiteral("getComponent"), {component});
    if (path.type() != QDBusMessage::ReplyMessage || path.arguments().isEmpty())
        return false;
    QDBusMessage m = QDBusMessage::createMethodCall(QLatin1String(kService), path.arguments().first().value<QDBusObjectPath>().path(),
                                                    QStringLiteral("org.kde.kglobalaccel.Component"), QStringLiteral("isActive"));
    const QDBusMessage r = QDBusConnection::sessionBus().call(m, QDBus::Block, 2000);
    return r.type() == QDBusMessage::ReplyMessage && !r.arguments().isEmpty() && r.arguments().first().toBool();
}

void GlobalHotkey::block(bool on)
{
    // Test runs share the user's session bus; leave their desktop alone.
    if (QGuiApplication::platformName() == QLatin1String("offscreen") || !available())
        return;
    QDBusMessage m = QDBusMessage::createMethodCall(QLatin1String(kService), QLatin1String(kPath), QLatin1String(kInterface), QStringLiteral("blockGlobalShortcuts"));
    m.setArguments({on});
    QDBusConnection::sessionBus().call(m, QDBus::Block, 2000);
}

bool GlobalHotkey::supports(Action a)
{
    return a == ShowWindow || !QStandardPaths::locate(QStandardPaths::ApplicationsLocation, desktopFile(a)).isEmpty();
}

static QKeySequence keyOf(const QStringList &id)
{
    const QList<int> keys = keysOf(call(QStringLiteral("shortcut"), {id}));
    return keys.isEmpty() || keys.first() == 0 ? QKeySequence() : QKeySequence(keys.first());
}

QKeySequence GlobalHotkey::current(Action a)
{
    if (!available())
        return {};
    return keyOf(actionId(a));
}

bool GlobalHotkey::set(Action a, const QKeySequence &key, QString *error)
{
    auto fail = [error](const QString &why) {
        if (error)
            *error = why;
        return false;
    };
    if (!available())
        return fail(QCoreApplication::translate("GlobalHotkey", "The desktop does not offer a way to set a global hotkey."));
    if (key.count() > 1)
        return fail(QCoreApplication::translate("GlobalHotkey", "A global hotkey must be a single key combination, not a sequence."));

    const QStringList id = actionId(a);
    // The earlier place for this key: free it (it may hold the very key being
    // set) so only the new entry has it.
    call(QStringLiteral("unregister"), {legacyId(a).at(0), legacyId(a).at(1)});
    // KDE decides once, when it first hears of an entry, whether it is a
    // launcher it can run. If its application cache does not have the entry
    // yet (just installed), it never would; so refresh the cache first.
    const QDBusMessage known = call(QStringLiteral("getComponent"), {id.first()});
    if (known.type() != QDBusMessage::ReplyMessage) {
        const QString tool = QStandardPaths::findExecutable(QStringLiteral("kbuildsycoca6"));
        if (!tool.isEmpty())
            QProcess::execute(tool, {});
    }
    // Creates the entry if the desktop has never heard of this launcher.
    call(QStringLiteral("doRegister"), {id});

    QList<int> want;
    if (!key.isEmpty())
        want.append(int(key[0].toCombined()));

    // The desktop counts a key as taken even by the entry that has it.
    if (!want.isEmpty() && keyOf(id) != key) {
        const QDBusMessage free = call(QStringLiteral("isGlobalShortcutAvailable"), {want.first(), id.first()});
        if (free.type() == QDBusMessage::ReplyMessage && !free.arguments().first().toBool())
            return fail(QCoreApplication::translate("GlobalHotkey", "%1 is already used by another global shortcut.").arg(key.toString(QKeySequence::NativeText)));
    }

    // 2 = present: the desktop listens for the key. Without it the key is
    // stored and shown in System Settings but pressing it does nothing, until
    // the next login, when KDE loads the entry itself.
    // 4 = do not replace this with whatever the desktop remembered earlier.
    QDBusMessage m = QDBusMessage::createMethodCall(QLatin1String(kService), QLatin1String(kPath), QLatin1String(kInterface), QStringLiteral("setShortcut"));
    QDBusArgument keys;
    keys.beginArray(qMetaTypeId<int>());
    for (int k : want)
        keys << k;
    keys.endArray();
    m.setArguments({id, QVariant::fromValue(keys), uint(2 | 4)});
    const QDBusMessage reply = QDBusConnection::sessionBus().call(m, QDBus::Block, 2000);
    if (reply.type() != QDBusMessage::ReplyMessage)
        return fail(reply.errorMessage().isEmpty() ? QCoreApplication::translate("GlobalHotkey", "The desktop did not answer.") : reply.errorMessage());
    if (keysOf(reply) != want)
        return fail(QCoreApplication::translate("GlobalHotkey", "The desktop did not accept that key."));
    if (!want.isEmpty() && !listening(id.first()))
        return fail(QCoreApplication::translate("GlobalHotkey", "The desktop saved %1 but is not responding to it. Logging out and back in should fix that.")
                        .arg(key.toString(QKeySequence::NativeText)));
    return true;
}

void GlobalHotkey::migrate()
{
    // Test runs share the user's session bus; leave their desktop alone.
    if (QGuiApplication::platformName() == QLatin1String("offscreen") || !available())
        return;
    // Keys of the old name are taken over once; after that, a key the user
    // removes stays removed.
    QSettings settings;
    const bool adopt = !settings.value(QStringLiteral("hotkeys/adoptedOldNames"), false).toBool();
    for (Action a : {ShowWindow, NewWindow, ToggleWindow}) {
        if (!supports(a))
            continue;
        const QStringList id = actionId(a);
        const QKeySequence k = keyOf(id);
        if (!k.isEmpty()) {
            // Saved by an earlier version without telling the desktop to listen.
            if (!listening(id.first()))
                set(a, k, nullptr);
        } else if (adopt) {
            QKeySequence old = keyOf(legacyId(a));
            if (old.isEmpty())
                old = savedLaunchKey(legacyId(a).first());
            if (!old.isEmpty())
                set(a, old, nullptr);
        }
    }
    settings.setValue(QStringLiteral("hotkeys/adoptedOldNames"), true);
}

// KDE runs the launcher for a key; nothing here hears it.
void GlobalHotkey::setHandler(std::function<void(Action)>) {}
