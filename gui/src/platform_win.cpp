#include "platform.h"

#include <QCoreApplication>
#include <QDesktopServices>
#include <QDir>
#include <QMap>
#include <QSettings>
#include <QUrl>

#include <windows.h>
#include <shellapi.h>
#include <shlobj.h>

QString Platform::daemonSocket()
{
    const QByteArray env = qgetenv("TRAWLRD_SOCKET");
    if (!env.isEmpty())
        return QString::fromLocal8Bit(env);
    // QLocalSocket turns a bare name into \\.\pipe\trawlrd.
    return QStringLiteral("trawlrd");
}

QChar Platform::separator()
{
    return QLatin1Char('\\');
}

QString Platform::startServiceHint()
{
    return QStringLiteral("sc start trawlrd");
}

void Platform::showInFileManager(QObject *, const QStringList &paths, const QString &fallbackDir)
{
    // Explorer selects items one folder at a time: group them by folder.
    QMap<QString, QStringList> byDir;
    for (const QString &p : paths) {
        const int cut = p.lastIndexOf(QLatin1Char('\\'));
        if (cut > 0)
            byDir[p.left(cut == 2 ? 3 : cut)] << p; // "C:" is written "C:\"
    }
    bool shown = false;
    for (auto it = byDir.cbegin(); it != byDir.cend(); ++it) {
        PIDLIST_ABSOLUTE folder = ILCreateFromPathW(reinterpret_cast<const wchar_t *>(QDir::toNativeSeparators(it.key()).utf16()));
        if (!folder)
            continue;
        QList<PIDLIST_ABSOLUTE> items;
        for (const QString &p : it.value())
            if (PIDLIST_ABSOLUTE item = ILCreateFromPathW(reinterpret_cast<const wchar_t *>(p.utf16())))
                items << item;
        if (!items.isEmpty())
            shown = SUCCEEDED(SHOpenFolderAndSelectItems(folder, UINT(items.size()), const_cast<LPCITEMIDLIST *>(reinterpret_cast<const LPITEMIDLIST *>(items.constData())), 0)) || shown;
        for (PIDLIST_ABSOLUTE item : items)
            ILFree(item);
        ILFree(folder);
    }
    if (!shown && !fallbackDir.isEmpty())
        QDesktopServices::openUrl(QUrl::fromLocalFile(fallbackDir));
}

void Platform::showProperties(QObject *, const QStringList &paths)
{
    // Explorer's properties dialog takes one item at a time this way.
    for (const QString &p : paths.mid(0, 5)) {
        const std::wstring w = p.toStdWString();
        SHELLEXECUTEINFOW info = {};
        info.cbSize = sizeof info;
        info.fMask = SEE_MASK_INVOKEIDLIST;
        info.lpVerb = L"properties";
        info.lpFile = w.c_str();
        info.nShow = SW_SHOW;
        ShellExecuteExW(&info);
    }
}

void Platform::openFullDiskAccessSettings() {}

void Platform::allowForeground()
{
    AllowSetForegroundWindow(ASFW_ANY);
}

static const char *const kRunKey = "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run";

bool Platform::hasStartAtLogin()
{
    return true;
}

bool Platform::startsAtLogin()
{
    return QSettings(QLatin1String(kRunKey), QSettings::NativeFormat).contains(QStringLiteral("Trawlr"));
}

void Platform::setStartAtLogin(bool on)
{
    QSettings run(QLatin1String(kRunKey), QSettings::NativeFormat);
    if (on)
        run.setValue(QStringLiteral("Trawlr"), QStringLiteral("\"%1\" --background").arg(QDir::toNativeSeparators(QCoreApplication::applicationFilePath())));
    else
        run.remove(QStringLiteral("Trawlr"));
}
