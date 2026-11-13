// Opening a terminal in a folder, or running a file in one and keeping the
// window open afterwards so its output can be read.
//
// Linux: the desktop's own terminal (or one chosen in Options), started with
// that terminal's own flags for the working folder and the command.
// Windows: PowerShell (default), Command Prompt or Windows Terminal.
// macOS: Terminal.app.
#pragma once
#include <QList>
#include <QString>
#include <QStringList>

namespace Terminal {

struct Choice {
    QString key;  // saved in the settings
    QString name; // shown in Options
};

// What to start: program, its arguments, and the folder to start it in.
struct Plan {
    QString program;
    QStringList arguments;
    QString nativeArguments; // Windows only: appended verbatim (cmd.exe quoting)
    QString workingDirectory;
    bool isNull() const { return program.isEmpty(); }
};

// "auto", every terminal found on this system, and "custom".
QList<Choice> choices();
// The terminal "auto" stands for here, e.g. "konsole".
QString autoKey();
QString displayName(const QString &key);

// The saved choice and custom command line.
QString savedKey();
QString savedCustom();

// The command that runs `file` and then leaves a shell open (Linux and
// macOS), or the file itself (Windows, whose shells stay open by flag).
QStringList runCommand(const QString &file);

// Plans for a given terminal; `custom` is used when key is "custom".
// An empty runArgv means "just open a terminal in dir".
Plan plan(const QString &key, const QString &custom, const QString &dir, const QStringList &runArgv);

// Start a terminal in `dir`. False, with `error` set, if none could be started.
bool openIn(const QString &dir, QString *error);
// Run `file` in a terminal started in its folder.
bool run(const QString &file, QString *error);

} // namespace Terminal
