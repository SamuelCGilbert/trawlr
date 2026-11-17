// macOS: Finder, the launchd daemon's socket, and a login item.
#include "platform.h"

#include <QCoreApplication>
#include <QDesktopServices>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QProcess>
#include <QStandardPaths>
#include <QUrl>

#import <AppKit/AppKit.h>
#include <unistd.h>

QString Platform::daemonSocket()
{
    const QByteArray env = qgetenv("TRAWLRD_SOCKET");
    if (!env.isEmpty())
        return QString::fromLocal8Bit(env);
    const QString system = QStringLiteral("/var/run/trawlrd/trawlrd.sock");
    if (QFileInfo::exists(system))
        return system;
    // A daemon run as this user (trawlrd without sudo).
    const QByteArray runtime = qgetenv("XDG_RUNTIME_DIR");
    if (!runtime.isEmpty())
        return QString::fromLocal8Bit(runtime) + QStringLiteral("/trawlrd/trawlrd.sock");
    const QString user = QStringLiteral("/tmp/trawlrd-%1/trawlrd/trawlrd.sock").arg(getuid());
    return QFileInfo::exists(user) ? user : system;
}

QChar Platform::separator()
{
    return QLatin1Char('/');
}

QString Platform::startServiceHint()
{
    return QStringLiteral("sudo launchctl kickstart system/io.trawlr.trawlrd");
}

void Platform::showInFileManager(QObject *, const QStringList &paths, const QString &fallbackDir)
{
    @autoreleasepool {
        NSMutableArray<NSURL *> *urls = [NSMutableArray arrayWithCapacity:paths.size()];
        for (const QString &p : paths)
            [urls addObject:[NSURL fileURLWithPath:p.toNSString()]];
        if (urls.count > 0) {
            [[NSWorkspace sharedWorkspace] activateFileViewerSelectingURLs:urls];
            return;
        }
    }
    if (!fallbackDir.isEmpty())
        QDesktopServices::openUrl(QUrl::fromLocalFile(fallbackDir));
}

// Finder's Get Info windows, through AppleScript. macOS asks once whether
// Trawlr may control Finder.
void Platform::showProperties(QObject *, const QStringList &paths)
{
    QStringList args;
    for (const QString &p : paths.mid(0, 5)) {
        QString quoted = p;
        quoted.replace(QLatin1Char('\\'), QLatin1String("\\\\")).replace(QLatin1Char('"'), QLatin1String("\\\""));
        args << QStringLiteral("-e") << QStringLiteral("tell application \"Finder\" to open information window of (POSIX file \"%1\" as alias)").arg(quoted);
    }
    if (args.isEmpty())
        return;
    args << QStringLiteral("-e") << QStringLiteral("tell application \"Finder\" to activate");
    QProcess::startDetached(QStringLiteral("/usr/bin/osascript"), args);
}

void Platform::allowForeground() {}

void Platform::openFullDiskAccessSettings()
{
    QDesktopServices::openUrl(QUrl(QStringLiteral("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")));
}

// A launchd agent of the user's own: macOS starts it at every sign-in.
static QString agentPath()
{
    return QDir::homePath() + QStringLiteral("/Library/LaunchAgents/io.trawlr.Trawlr.plist");
}

bool Platform::hasStartAtLogin()
{
    return true;
}

bool Platform::startsAtLogin()
{
    return QFile::exists(agentPath());
}

void Platform::setStartAtLogin(bool on)
{
    if (!on) {
        QFile::remove(agentPath());
        return;
    }
    QString exe = QCoreApplication::applicationFilePath().toHtmlEscaped();
    const QString plist = QStringLiteral(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n"
        "<plist version=\"1.0\">\n<dict>\n"
        "\t<key>Label</key>\n\t<string>io.trawlr.Trawlr</string>\n"
        "\t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>%1</string>\n\t\t<string>--background</string>\n\t</array>\n"
        "\t<key>RunAtLoad</key>\n\t<true/>\n"
        "\t<key>LimitLoadToSessionType</key>\n\t<string>Aqua</string>\n"
        "\t<key>ProcessType</key>\n\t<string>Interactive</string>\n"
        "</dict>\n</plist>\n").arg(exe);
    QDir().mkpath(QFileInfo(agentPath()).path());
    QFile f(agentPath());
    if (f.open(QIODevice::WriteOnly | QIODevice::Truncate))
        f.write(plist.toUtf8());
}
