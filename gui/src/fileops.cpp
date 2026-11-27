#include "fileops.h"

#include <QCoreApplication>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QRandomGenerator>

#ifdef Q_OS_WIN
#include <windows.h>
#else
#include <cerrno>
#include <cstdio>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>
#endif

static QString tr(const char *s)
{
    return QCoreApplication::translate("FileOps", s);
}

QString FileOps::invalidNameReason(const QString &name)
{
    if (name.trimmed().isEmpty())
        return tr("Type a name.");
    if (name == QLatin1String(".") || name == QLatin1String(".."))
        return tr("“.” and “..” cannot be used as names.");
#ifdef Q_OS_WIN
    static const QString bad = QStringLiteral("\\/:*?\"<>|");
    for (const QChar c : name) {
        if (bad.contains(c))
            return tr("A name cannot contain any of these characters: \\ / : * ? \" < > |");
        if (c.unicode() < 32)
            return tr("A name cannot contain control characters.");
    }
    if (name.endsWith(QLatin1Char(' ')) || name.endsWith(QLatin1Char('.')))
        return tr("A name cannot end with a space or a dot.");
    // Device names, with or without an extension, are not files on Windows.
    const QString stem = name.section(QLatin1Char('.'), 0, 0).trimmed().toUpper();
    static const QStringList devices = {"CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
                                        "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9"};
    if (devices.contains(stem))
        return tr("“%1” is reserved by Windows and cannot be used as a name.").arg(stem);
#else
    if (name.contains(QLatin1Char('/')))
        return tr("A name cannot contain “/”.");
    if (name.contains(QChar(0)))
        return tr("A name cannot contain a null character.");
#ifdef Q_OS_MACOS
    // Finder shows a ":" in a name as "/", and refuses to make one.
    if (name.contains(QLatin1Char(':')))
        return tr("A name cannot contain “:”.");
#endif
#endif
    return QString();
}

static QString alreadyThere()
{
    return tr("Another item with that name is already in the folder.");
}

#ifdef Q_OS_WIN

bool FileOps::rename(const QString &from, const QString &to, QString *error)
{
    // Without MOVEFILE_REPLACE_EXISTING this refuses to replace anything,
    // and a change of case only is an ordinary rename.
    if (MoveFileExW(reinterpret_cast<const wchar_t *>(QDir::toNativeSeparators(from).utf16()),
                    reinterpret_cast<const wchar_t *>(QDir::toNativeSeparators(to).utf16()), 0))
        return true;
    const DWORD e = GetLastError();
    *error = e == ERROR_ALREADY_EXISTS || e == ERROR_FILE_EXISTS ? alreadyThere() : qt_error_string(int(e));
    return false;
}

#else

static bool fail(QString *error, int e)
{
    *error = e == EEXIST || e == ENOTEMPTY ? alreadyThere() : qt_error_string(e);
    return false;
}

bool FileOps::rename(const QString &from, const QString &to, QString *error)
{
    const QByteArray f = QFile::encodeName(from);
    const QByteArray t = QFile::encodeName(to);
    struct stat a, b;
    if (::lstat(f.constData(), &a) != 0)
        return fail(error, errno);
    if (::lstat(t.constData(), &b) == 0) {
        // The new name finds the item itself: only its case changes, on a file
        // system that ignores case (FAT, exFAT, most of macOS). Renaming a
        // file onto itself does nothing, so go by way of a temporary name.
        // (A hard link that differs only in case is another name, not this.)
        const QString toName = QFileInfo(to).fileName();
        const bool caseOnly = a.st_dev == b.st_dev && a.st_ino == b.st_ino && from != to
            && from.compare(to, Qt::CaseInsensitive) == 0
            && !QDir(QFileInfo(to).path()).entryList(QDir::AllEntries | QDir::Hidden | QDir::System | QDir::NoDotAndDotDot).contains(toName);
        if (!caseOnly)
            return fail(error, EEXIST);
        const QByteArray tmp = QFile::encodeName(from + QStringLiteral(".trawlr-rename-") + QString::number(QRandomGenerator::global()->generate(), 36));
        if (::rename(f.constData(), tmp.constData()) != 0)
            return fail(error, errno);
        if (::rename(tmp.constData(), t.constData()) != 0) {
            const int e = errno;
            ::rename(tmp.constData(), f.constData());
            return fail(error, e);
        }
        return true;
    }
    // Refuse to replace an item that appears at the new name meanwhile.
#if defined(Q_OS_MACOS)
    if (::renamex_np(f.constData(), t.constData(), RENAME_EXCL) == 0)
        return true;
    if (errno != ENOTSUP && errno != EINVAL)
        return fail(error, errno);
#elif defined(RENAME_NOREPLACE)
    if (::renameat2(AT_FDCWD, f.constData(), AT_FDCWD, t.constData(), RENAME_NOREPLACE) == 0)
        return true;
    if (errno != EINVAL && errno != ENOSYS)
        return fail(error, errno);
#endif
    // A file system without an exclusive rename: the check above has to do.
    if (::rename(f.constData(), t.constData()) == 0)
        return true;
    return fail(error, errno);
}

#endif
