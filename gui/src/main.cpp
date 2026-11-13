#include "globalhotkey.h"
#include "instance.h"
#include "mainwindow.h"

#include <QApplication>
#include <QIcon>
#include <QLockFile>
#include <QSettings>
#include <QTimer>
#include <QToolButton>
#include <cstdlib>

// Before the rename the settings lived under "everything-for-linux". Copy
// them over once, so an update keeps the user's options and window layout.
static void adoptOldSettings()
{
    QSettings now;
    if (!now.allKeys().isEmpty())
        return;
    QSettings old(QStringLiteral("everything-for-linux"), QStringLiteral("everything"));
    const QStringList keys = old.allKeys();
    for (const QString &k : keys)
        now.setValue(k, old.value(k));
}

int main(int argc, char **argv)
{
    QApplication app(argc, argv);
    QApplication::setApplicationName(QStringLiteral("trawlr"));
    QApplication::setOrganizationName(QStringLiteral("trawlr"));
    QApplication::setOrganizationDomain(QStringLiteral("trawlr.io"));
    QApplication::setApplicationDisplayName(QStringLiteral("Trawlr"));
    QApplication::setApplicationVersion(QStringLiteral(APP_VERSION));
    // Ties the window to its launcher entry (icon, taskbar grouping) on Wayland.
    QApplication::setDesktopFileName(QStringLiteral("io.trawlr.Trawlr"));
    QApplication::setWindowIcon(QIcon::fromTheme(QStringLiteral("io.trawlr.Trawlr"), QIcon(QStringLiteral(":/icon.svg"))));
    adoptOldSettings();

    QStringList words;
    QString shot;
    bool newWindow = false;
    bool toggle = false;
    const QStringList args = QApplication::arguments();
    for (int i = 1; i < args.size(); ++i) {
        if (args[i] == QLatin1String("--version")) {
            printf("trawlr-gui %s\n", APP_VERSION);
            return 0;
        }
        if (args[i] == QLatin1String("--help") || args[i] == QLatin1String("-h")) {
            printf("Usage: trawlr-gui [--new-window | --toggle] [search terms]\n"
                   "   or: trawlr --gui [search terms]\n\n"
                   "Opens the search window, optionally with a search already typed in.\n"
                   "If Trawlr is already running, its window is brought to the front instead\n"
                   "(or another window is opened, if Tools > Options allows multiple windows).\n\n"
                   "  --new-window   always open another window\n"
                   "  --toggle       minimize the window if it is the active one, else show it\n");
            return 0;
        }
        if (args[i] == QLatin1String("--new-window") || args[i] == QLatin1String("-n")) {
            newWindow = true;
            continue;
        }
        if (args[i] == QLatin1String("--toggle")) {
            toggle = true;
            continue;
        }
        // Undocumented, for automated checks: render the window to a PNG and exit.
        if (args[i] == QLatin1String("--screenshot") && i + 1 < args.size()) {
            shot = args[++i];
            continue;
        }
        words << args[i];
    }

    // A screenshot run is a private, short-lived window: it neither hands over
    // to the user's window nor becomes the one later launches find.
    Instance instance;
    if (shot.isEmpty()) {
        const QString path = Instance::defaultPath();
        // Launches that start at the same moment would all find nobody listening
        // and all decide to be the first. One at a time through this part, then.
        // (If the lock cannot be had in time, carry on without it.)
        QLockFile startLock(path + QStringLiteral(".lock"));
        startLock.tryLock(3000);
        LaunchRequest req;
        req.search = words.join(QLatin1Char(' '));
        req.newWindow = newWindow;
        req.toggle = toggle;
        // The desktop sets one of these in the environment of a program it launches
        // for a keypress or click; it is what lets the running window take focus.
        req.activationToken = qEnvironmentVariable("XDG_ACTIVATION_TOKEN", qEnvironmentVariable("DESKTOP_STARTUP_ID"));
        switch (Instance::forward(path, req)) {
        case Instance::Handled:
            return 0;
        case Instance::NoInstance:
            if (instance.listen(path)) {
                QObject::connect(&instance, &Instance::requested, &app, [](const LaunchRequest &r) { MainWindow::handleLaunch(r); });
                QTimer::singleShot(0, &app, [] { GlobalHotkey::migrate(); });
            }
            else if (Instance::forward(path, req) == Instance::Handled)
                return 0; // two launches at once: the other one became the first
            break;
        case Instance::Unresponsive:
            break; // something is stuck holding the socket; just run on our own
        }
    }

    MainWindow *w = new MainWindow(words.join(QLatin1Char(' ')));
    w->setAttribute(Qt::WA_DeleteOnClose);
    // A screenshot run can render without ever putting a window on screen.
    if (!shot.isEmpty()) {
        if (!qEnvironmentVariableIsSet("TRAWLR_SCREENSHOT_HIDDEN"))
            w->show();
    } else {
        w->bringToFront();
    }
    if (!shot.isEmpty()) {
        const int delay = qEnvironmentVariableIntValue("TRAWLR_SCREENSHOT_DELAY_MS");
        // TRAWLR_SCREENSHOT_ACTION names a menu command to run first (its
        // dialog or menu is what gets captured), for checking those by eye.
        const QString action = qEnvironmentVariable("TRAWLR_SCREENSHOT_ACTION");
        if (!action.isEmpty()) {
            QTimer::singleShot(400, w, [w, action] {
                for (QAction *a : w->findChildren<QAction *>()) {
                    if (a->text().remove(QLatin1Char('&')) == action) {
                        a->trigger();
                        return;
                    }
                }
                if (action == QLatin1String("menu")) {
                    for (QToolButton *t : w->findChildren<QToolButton *>())
                        if (t->menu())
                            t->showMenu();
                }
            });
        }
        QTimer::singleShot(delay > 0 ? delay : 1500, w, [w, shot] {
            QWidget *target = QApplication::activePopupWidget();
            if (!target)
                target = QApplication::activeModalWidget();
            (target ? target : w)->grab().save(shot);
            QApplication::exit(0);
            QTimer::singleShot(0, [] { std::_Exit(0); }); // a dialog's own loop would hold quit back
        });
    }
    return app.exec();
}
