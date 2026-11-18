#include "terminal.h"

#include <QCoreApplication>
#include <QFile>
#include <QHash>
#include <QFileInfo>
#include <QProcess>
#include <QProcessEnvironment>
#include <QSettings>
#include <QStandardPaths>
#include <functional>

namespace Terminal {

static const QString kProgram = QStringLiteral("terminal/program");
static const QString kCustom = QStringLiteral("terminal/custom");

static QString tr(const char *s)
{
    return QCoreApplication::translate("Terminal", s);
}

QString savedKey()
{
    return QSettings().value(kProgram, QStringLiteral("auto")).toString();
}

QString savedCustom()
{
    return QSettings().value(kCustom).toString();
}

[[maybe_unused]] static bool installed(const QString &program)
{
    return !QStandardPaths::findExecutable(program).isEmpty();
}

// For a shell command line: 'it''s' style quoting that any POSIX shell reads back as one word.
static QString shellQuote(const QString &s)
{
    QString out = s;
    out.replace(QLatin1Char('\''), QStringLiteral("'\\''"));
    return QLatin1Char('\'') + out + QLatin1Char('\'');
}

[[maybe_unused]] static QString shellJoin(const QStringList &argv)
{
    QStringList quoted;
    for (const QString &a : argv)
        quoted << shellQuote(a);
    return quoted.join(QLatin1Char(' '));
}

#if !defined(Q_OS_WIN)
// What to put in front of a script that is not executable: the interpreter
// its #! line names, or one that goes with its extension.
static QStringList interpreterFor(const QString &file)
{
    const QFileInfo fi(file);
    if (fi.isExecutable())
        return {};
    QFile f(file);
    if (f.open(QIODevice::ReadOnly)) {
        const QByteArray first = f.read(256).split('\n').first().trimmed();
        if (first.startsWith("#!")) {
            const QStringList parts = QString::fromUtf8(first.mid(2)).split(QLatin1Char(' '), Qt::SkipEmptyParts);
            if (!parts.isEmpty())
                return parts;
        }
    }
    const QString ext = fi.suffix().toLower();
    static const QHash<QString, QStringList> byExt = {
        {"sh", {"sh"}},       {"bash", {"bash"}},    {"zsh", {"zsh"}},   {"fish", {"fish"}}, {"py", {"python3"}},
        {"pl", {"perl"}},     {"rb", {"ruby"}},      {"js", {"node"}},   {"mjs", {"node"}},  {"lua", {"lua"}},
        {"php", {"php"}},     {"ps1", {"pwsh", "-File"}}, {"jar", {"java", "-jar"}},
    };
    return byExt.value(ext);
}
#endif

QStringList runCommand(const QString &file)
{
#ifdef Q_OS_WIN
    return {file};
#else
    // Runs the command, says how it ended, then hands over to an interactive
    // shell in the same folder: the output stays readable and one can carry on.
    static const QString wrapper = QStringLiteral(
        "\"$@\"; s=$?; printf '\\n\\033[2m[exited with code %s]\\033[0m\\n' \"$s\"; exec \"${SHELL:-/bin/sh}\" -i");
    return QStringList{QStringLiteral("sh"), QStringLiteral("-c"), wrapper, QStringLiteral("sh")} + interpreterFor(file) + QStringList{file};
#endif
}

#if defined(Q_OS_LINUX) || (defined(Q_OS_UNIX) && !defined(Q_OS_MACOS))
// Each terminal's own way of saying "start in this folder" and "run this".
struct Known {
    const char *key;
    const char *name;
    std::function<QStringList(const QString &)> open;
    std::function<QStringList(const QString &, const QStringList &)> run;
};

static const QList<Known> &known()
{
    using L = QStringList;
    auto eq = [](const char *flag) { return [flag](const QString &d) { return L{QLatin1String(flag) + d}; }; };
    static const QList<Known> list = {
        {"konsole", "Konsole", [](const QString &d) { return L{"--workdir", d}; },
         [](const QString &d, const L &c) { return L{"--workdir", d, "-e"} + c; }},
        {"ptyxis", "Ptyxis", [](const QString &d) { return L{"--new-window", "--working-directory=" + d}; },
         [](const QString &d, const L &c) { return L{"--new-window", "--working-directory=" + d, "--"} + c; }},
        {"kgx", "GNOME Console", eq("--working-directory="),
         [](const QString &d, const L &c) { return L{"--working-directory=" + d, "--"} + c; }},
        {"gnome-terminal", "GNOME Terminal", eq("--working-directory="),
         [](const QString &d, const L &c) { return L{"--working-directory=" + d, "--"} + c; }},
        {"xfce4-terminal", "Xfce Terminal", eq("--working-directory="),
         [](const QString &d, const L &c) { return L{"--working-directory=" + d, "-x"} + c; }},
        {"mate-terminal", "MATE Terminal", eq("--working-directory="),
         [](const QString &d, const L &c) { return L{"--working-directory=" + d, "-x"} + c; }},
        {"qterminal", "QTerminal", [](const QString &d) { return L{"-w", d}; },
         [](const QString &d, const L &c) { return L{"-w", d, "-e", shellJoin(c)}; }},
        {"lxterminal", "LXTerminal", eq("--working-directory="),
         [](const QString &d, const L &c) { return L{"--working-directory=" + d, "-e", shellJoin(c)}; }},
        {"tilix", "Tilix", eq("--working-directory="),
         [](const QString &d, const L &c) { return L{"--working-directory=" + d, "-e", shellJoin(c)}; }},
        {"terminator", "Terminator", eq("--working-directory="),
         [](const QString &d, const L &c) { return L{"--working-directory=" + d, "-x"} + c; }},
        {"cosmic-term", "COSMIC Terminal", [](const QString &) { return L{}; },
         [](const QString &, const L &c) { return L{"-e"} + c; }},
        {"ghostty", "Ghostty", eq("--working-directory="),
         [](const QString &d, const L &c) { return L{"--working-directory=" + d, "-e"} + c; }},
        {"alacritty", "Alacritty", [](const QString &d) { return L{"--working-directory", d}; },
         [](const QString &d, const L &c) { return L{"--working-directory", d, "-e"} + c; }},
        {"kitty", "kitty", [](const QString &d) { return L{"--directory", d}; },
         [](const QString &d, const L &c) { return L{"--directory", d} + c; }},
        {"foot", "foot", eq("--working-directory="),
         [](const QString &d, const L &c) { return L{"--working-directory=" + d} + c; }},
        {"wezterm", "WezTerm", [](const QString &d) { return L{"start", "--cwd", d}; },
         [](const QString &d, const L &c) { return L{"start", "--cwd", d, "--"} + c; }},
        {"xdg-terminal-exec", "Default terminal (xdg-terminal-exec)", [](const QString &) { return L{}; },
         [](const QString &, const L &c) { return c; }},
        {"x-terminal-emulator", "Default terminal (x-terminal-emulator)", [](const QString &) { return L{}; },
         [](const QString &, const L &c) { return L{"-e"} + c; }},
        {"xterm", "XTerm", [](const QString &) { return L{}; }, [](const QString &, const L &c) { return L{"-e"} + c; }},
    };
    return list;
}

static const Known *findKnown(const QString &key)
{
    for (const Known &k : known())
        if (key == QLatin1String(k.key))
            return &k;
    return nullptr;
}
#endif

QString autoKey()
{
#ifdef Q_OS_WIN
    return QStringLiteral("powershell");
#elif defined(Q_OS_MACOS)
    return QStringLiteral("terminal");
#else
    // The user's own choice first, then the desktop's terminal, then the
    // desktop-neutral launchers, then whatever is there.
    const QString env = QFileInfo(qEnvironmentVariable("TERMINAL")).fileName();
    if (findKnown(env) && installed(env))
        return env;
    const QStringList desktops = qEnvironmentVariable("XDG_CURRENT_DESKTOP").toLower().split(QLatin1Char(':'));
    QStringList preferred;
    for (const QString &d : desktops) {
        if (d == QLatin1String("kde"))
            preferred << "konsole";
        else if (d == QLatin1String("gnome") || d == QLatin1String("ubuntu") || d == QLatin1String("unity") || d == QLatin1String("budgie"))
            preferred << "ptyxis" << "kgx" << "gnome-terminal";
        else if (d == QLatin1String("xfce"))
            preferred << "xfce4-terminal";
        else if (d == QLatin1String("mate"))
            preferred << "mate-terminal";
        else if (d == QLatin1String("lxqt"))
            preferred << "qterminal";
        else if (d == QLatin1String("lxde"))
            preferred << "lxterminal";
        else if (d == QLatin1String("x-cinnamon") || d == QLatin1String("cinnamon"))
            preferred << "gnome-terminal";
        else if (d == QLatin1String("cosmic"))
            preferred << "cosmic-term";
    }
    preferred << "xdg-terminal-exec" << "x-terminal-emulator";
    for (const QString &k : std::as_const(preferred))
        if (installed(k))
            return k;
    for (const Known &k : known())
        if (installed(QLatin1String(k.key)))
            return QLatin1String(k.key);
    return QString();
#endif
}

QList<Choice> choices()
{
    QList<Choice> out;
    const QString a = autoKey();
    out.append({QStringLiteral("auto"), a.isEmpty() ? tr("Automatic (none found)") : tr("Automatic (%1)").arg(displayName(a))});
#ifdef Q_OS_WIN
    out.append({QStringLiteral("powershell"), displayName(QStringLiteral("powershell"))});
    out.append({QStringLiteral("cmd"), displayName(QStringLiteral("cmd"))});
    if (installed(QStringLiteral("wt")))
        out.append({QStringLiteral("wt"), displayName(QStringLiteral("wt"))});
#elif defined(Q_OS_MACOS)
    out.append({QStringLiteral("terminal"), displayName(QStringLiteral("terminal"))});
#else
    for (const Known &k : known())
        if (installed(QLatin1String(k.key)))
            out.append({QLatin1String(k.key), QLatin1String(k.name)});
#endif
    out.append({QStringLiteral("custom"), tr("Custom command…")});
    return out;
}

QString displayName(const QString &key)
{
    if (key == QLatin1String("auto"))
        return tr("Automatic");
    if (key == QLatin1String("custom"))
        return tr("Custom command");
#ifdef Q_OS_WIN
    if (key == QLatin1String("powershell"))
        return installed(QStringLiteral("pwsh")) ? tr("PowerShell 7") : tr("Windows PowerShell");
    if (key == QLatin1String("cmd"))
        return tr("Command Prompt");
    if (key == QLatin1String("wt"))
        return tr("Windows Terminal");
#elif defined(Q_OS_MACOS)
    if (key == QLatin1String("terminal"))
        return tr("Terminal");
#else
    if (const Known *k = findKnown(key))
        return QLatin1String(k->name);
#endif
    return key;
}

// The custom command line: {dir} becomes the folder, a lone {cmd} the
// command to run (added at the end if it is not there).
static Plan customPlan(const QString &custom, const QString &dir, const QStringList &runArgv)
{
    QStringList parts = QProcess::splitCommand(custom);
    if (parts.isEmpty())
        return {};
    QStringList args;
    bool placed = false;
    for (int i = 1; i < parts.size(); ++i) {
        if (parts[i] == QLatin1String("{cmd}")) {
            args << runArgv;
            placed = true;
            continue;
        }
        QString a = parts[i];
        a.replace(QLatin1String("{dir}"), dir);
        args << a;
    }
    if (!placed)
        args << runArgv;
    QString program = parts.first();
    program.replace(QLatin1String("{dir}"), dir);
    return {program, args, QString(), dir};
}

Plan plan(const QString &keyIn, const QString &custom, const QString &dir, const QStringList &runArgv)
{
    if (keyIn == QLatin1String("custom"))
        return customPlan(custom, dir, runArgv);
    const QString key = keyIn == QLatin1String("auto") || keyIn.isEmpty() ? autoKey() : keyIn;
    const bool running = !runArgv.isEmpty();
#ifdef Q_OS_WIN
    const QString ps = installed(QStringLiteral("pwsh")) ? QStringLiteral("pwsh.exe") : QStringLiteral("powershell.exe");
    // PowerShell's own quoting: inside '...' a quote is written twice. It
    // takes the typographic single quotes for quotes too, so a name with one
    // in it must not end the string early and run the rest as commands.
    QString psRun;
    if (running) {
        QString f;
        for (const QChar ch : runArgv.first()) {
            f += ch;
            if (ch == QLatin1Char('\'') || ch == QChar(0x2018) || ch == QChar(0x2019) || ch == QChar(0x201A) || ch == QChar(0x201B))
                f += ch;
        }
        psRun = QStringLiteral("& '") + f + QLatin1Char('\'');
    }
    if (key == QLatin1String("cmd")) {
        Plan p{QStringLiteral("cmd.exe"), {}, QString(), dir};
        if (running)
            p.nativeArguments = QStringLiteral("/K \"\"") + runArgv.first() + QStringLiteral("\"\"");
        return p;
    }
    if (key == QLatin1String("wt")) {
        QStringList args{QStringLiteral("-d"), dir};
        if (running) {
            QString cmd = psRun;
            cmd.replace(QLatin1Char(';'), QStringLiteral("\\;")); // wt splits its command line at ;
            args << ps << QStringLiteral("-NoLogo") << QStringLiteral("-NoExit") << QStringLiteral("-Command") << cmd;
        }
        return {QStringLiteral("wt.exe"), args, QString(), dir};
    }
    QStringList args{QStringLiteral("-NoLogo"), QStringLiteral("-NoExit")};
    if (running)
        args << QStringLiteral("-Command") << psRun;
    return {ps, args, QString(), dir};
#elif defined(Q_OS_MACOS)
    Q_UNUSED(key);
    if (!running)
        return {QStringLiteral("open"), {QStringLiteral("-a"), QStringLiteral("Terminal"), dir}, QString(), dir};
    QString script = QStringLiteral("cd ") + shellQuote(dir) + QStringLiteral(" && ") + shellJoin(runArgv);
    script.replace(QLatin1Char('\\'), QStringLiteral("\\\\"));
    script.replace(QLatin1Char('"'), QStringLiteral("\\\""));
    return {QStringLiteral("osascript"),
            {QStringLiteral("-e"), QStringLiteral("tell application \"Terminal\" to do script \"") + script + QLatin1Char('"'),
             QStringLiteral("-e"), QStringLiteral("tell application \"Terminal\" to activate")},
            QString(), dir};
#else
    if (key.isEmpty())
        return {};
    if (const Known *k = findKnown(key))
        return {QLatin1String(k->key), running ? k->run(dir, runArgv) : k->open(dir), QString(), dir};
    // A terminal we know nothing about (from $TERMINAL): -e is the old convention.
    return {key, running ? QStringList{QStringLiteral("-e")} + runArgv : QStringList(), QString(), dir};
#endif
}

static bool start(const Plan &p, QString *error)
{
    if (p.isNull()) {
        *error = tr("No terminal program was found. Choose one in Tools > Options > Terminal.");
        return false;
    }
    QProcess proc;
    proc.setProgram(p.program);
    proc.setArguments(p.arguments);
#ifdef Q_OS_WIN
    if (!p.nativeArguments.isEmpty())
        proc.setNativeArguments(p.nativeArguments);
#endif
    proc.setWorkingDirectory(p.workingDirectory);
    // Launch tokens we were started with are spent; they must not leak on.
    QProcessEnvironment env = QProcessEnvironment::systemEnvironment();
    env.remove(QStringLiteral("XDG_ACTIVATION_TOKEN"));
    env.remove(QStringLiteral("DESKTOP_STARTUP_ID"));
    proc.setProcessEnvironment(env);
    if (!proc.startDetached()) {
        *error = tr("Could not start %1: %2").arg(p.program, proc.errorString());
        return false;
    }
    return true;
}

bool openIn(const QString &dir, QString *error)
{
    return start(plan(savedKey(), savedCustom(), dir, {}), error);
}

bool run(const QString &file, QString *error)
{
    return start(plan(savedKey(), savedCustom(), QFileInfo(file).absolutePath(), runCommand(file)), error);
}

} // namespace Terminal
