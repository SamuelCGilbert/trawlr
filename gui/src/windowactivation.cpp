#include "windowactivation.h"

#include <QCoreApplication>
#include <QDBusConnection>
#include <QDBusConnectionInterface>
#include <QDBusMessage>
#include <QDir>
#include <QFile>
#include <QJsonArray>
#include <QJsonDocument>
#include <QStandardPaths>
#include <QTimer>

static const char *const kService = "org.kde.KWin";
static const char *const kScriptName = "trawlr-activate";

static QDBusMessage call(const QString &path, const QString &iface, const QString &method, const QVariantList &args)
{
    QDBusMessage m = QDBusMessage::createMethodCall(QLatin1String(kService), path, iface, method);
    m.setArguments(args);
    return QDBusConnection::sessionBus().call(m, QDBus::Block, 1500);
}

bool WindowActivation::viaKWin(const QString &title)
{
    QDBusConnectionInterface *bus = QDBusConnection::sessionBus().interface();
    if (!bus || !bus->isServiceRegistered(QLatin1String(kService)))
        return false;

    QString dir = QStandardPaths::writableLocation(QStandardPaths::RuntimeLocation);
    if (dir.isEmpty())
        dir = QDir::tempPath();
    const QString file = dir + QStringLiteral("/trawlr-activate-%1.js").arg(QCoreApplication::applicationPid());
    QFile f(file);
    if (!f.open(QIODevice::WriteOnly | QIODevice::Truncate))
        return false;
    // The title goes in as a JSON string, which is also a valid JavaScript one,
    // so whatever characters it holds cannot break out of the script.
    const QByteArray array = QJsonDocument(QJsonArray{title}).toJson(QJsonDocument::Compact);
    const QByteArray quoted = array.mid(1, array.size() - 2);
    f.write("var pid = " + QByteArray::number(QCoreApplication::applicationPid()) + ";\nvar title = " + quoted + ";\n"
            "var all = workspace.windowList();\n"
            "for (var i = 0; i < all.length; i++) {\n"
            "    if (all[i].pid == pid && all[i].caption == title) { workspace.activeWindow = all[i]; break; }\n"
            "}\n");
    f.close();

    const QString script = QLatin1String(kScriptName);
    call(QStringLiteral("/Scripting"), QStringLiteral("org.kde.kwin.Scripting"), QStringLiteral("unloadScript"), {script});
    const QDBusMessage loaded = call(QStringLiteral("/Scripting"), QStringLiteral("org.kde.kwin.Scripting"), QStringLiteral("loadScript"), {file, script});
    bool ok = false;
    if (loaded.type() == QDBusMessage::ReplyMessage && !loaded.arguments().isEmpty()) {
        const int id = loaded.arguments().first().toInt();
        if (id >= 0)
            ok = call(QStringLiteral("/Scripting/Script%1").arg(id), QStringLiteral("org.kde.kwin.Script"), QStringLiteral("run"), {}).type() == QDBusMessage::ReplyMessage;
    }
    QFile::remove(file); // KWin has read it by now
    // Leave nothing loaded in KWin afterwards, once the script has had time to finish.
    QTimer::singleShot(1000, qApp, [script] {
        call(QStringLiteral("/Scripting"), QStringLiteral("org.kde.kwin.Scripting"), QStringLiteral("unloadScript"), {script});
    });
    return ok;
}
