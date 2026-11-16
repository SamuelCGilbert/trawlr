#include "platform.h"

#include <QDBusConnection>
#include <QDBusMessage>
#include <QDBusPendingCallWatcher>
#include <QDesktopServices>
#include <QFileInfo>
#include <QUrl>

QString Platform::daemonSocket()
{
    const QByteArray env = qgetenv("TRAWLRD_SOCKET");
    if (!env.isEmpty())
        return QString::fromLocal8Bit(env);
    const QString system = QStringLiteral("/run/trawlrd/trawlrd.sock");
    if (QFileInfo::exists(system))
        return system;
    const QByteArray runtime = qgetenv("XDG_RUNTIME_DIR");
    if (!runtime.isEmpty())
        return QString::fromLocal8Bit(runtime) + QStringLiteral("/trawlrd/trawlrd.sock");
    return system;
}

QChar Platform::separator()
{
    return QLatin1Char('/');
}

QString Platform::startServiceHint()
{
    return QStringLiteral("systemctl start trawlrd");
}

// Ask the file manager, over the desktop-neutral FileManager1 interface, to
// show the items in their folders (or their properties). Falls back to
// simply opening the folder if nobody answers.
static void fileManagerCall(QObject *ctx, const QString &method, const QStringList &paths, const QString &fallbackDir)
{
    QStringList urls;
    for (const QString &p : paths)
        urls << QUrl::fromLocalFile(p).toString();
    QDBusMessage m = QDBusMessage::createMethodCall(QStringLiteral("org.freedesktop.FileManager1"), QStringLiteral("/org/freedesktop/FileManager1"),
                                                    QStringLiteral("org.freedesktop.FileManager1"), method);
    m << urls << QString();
    auto *w = new QDBusPendingCallWatcher(QDBusConnection::sessionBus().asyncCall(m), ctx);
    QObject::connect(w, &QDBusPendingCallWatcher::finished, ctx, [w, fallbackDir] {
        if (w->isError() && !fallbackDir.isEmpty())
            QDesktopServices::openUrl(QUrl::fromLocalFile(fallbackDir));
        w->deleteLater();
    });
}

void Platform::showInFileManager(QObject *ctx, const QStringList &paths, const QString &fallbackDir)
{
    fileManagerCall(ctx, QStringLiteral("ShowItems"), paths, fallbackDir);
}

void Platform::showProperties(QObject *ctx, const QStringList &paths)
{
    fileManagerCall(ctx, QStringLiteral("ShowItemProperties"), paths, QString());
}

void Platform::allowForeground() {}

bool Platform::hasStartAtLogin()
{
    return false;
}

bool Platform::startsAtLogin()
{
    return false;
}

void Platform::setStartAtLogin(bool) {}
