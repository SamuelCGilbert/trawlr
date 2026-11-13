// End-to-end test of the window: starts trawlrd on a scratch folder,
// then types, navigates, sorts and changes files on disk, checking what the
// window shows. Run with QT_QPA_PLATFORM=offscreen; set TRAWLRD_BIN to
// the daemon binary (default ../../../target/release/trawlrd).
#include "instance.h"
#include "mainwindow.h"
#include "optionsdialog.h"
#include "resultstore.h"
#include "resultsview.h"
#include "shortcuts.h"
#include "terminal.h"

#include <QAction>
#include <QCheckBox>
#include <QClipboard>
#include <QDesktopServices>
#include <QHeaderView>
#include <QKeySequenceEdit>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QMessageBox>
#include <QProcess>
#include <QPushButton>
#include <QStatusBar>
#include <QTemporaryDir>
#include <QTreeWidget>
#include <QtTest>
#include <atomic>
#include <thread>

// Stands in for the desktop when the window opens a file:// URL.
class UrlCatcher : public QObject {
    Q_OBJECT
public:
    QList<QUrl> urls;
public slots:
    void open(const QUrl &u) { urls << u; }
};

class GuiTest : public QObject {
    Q_OBJECT
    QTemporaryDir m_dir;
    QProcess m_daemon;
    MainWindow *m_win = nullptr;
    QLineEdit *m_search = nullptr;
    ResultsView *m_view = nullptr;
    ResultStore *m_store = nullptr;

    QString tree() const { return m_dir.path() + "/tree"; }
    void write(const QString &rel, int bytes)
    {
        QFile f(tree() + "/" + rel);
        QDir().mkpath(QFileInfo(f).path());
        QVERIFY(f.open(QIODevice::WriteOnly));
        f.write(QByteArray(bytes, 'x'));
    }
    QString status() const { return m_win->statusBar()->findChildren<QLabel *>().first()->text(); }
    QString nameAt(qint64 r) const { const Row *row = m_store->cached(r); return row ? row->name : QStringLiteral("<not loaded>"); }
    QAction *action(const QString &text) const
    {
        for (QAction *a : m_win->findChildren<QAction *>())
            if (a->text().remove('&') == text)
                return a;
        return nullptr;
    }

    static int windowCount()
    {
        int n = 0;
        for (QWidget *w : QApplication::topLevelWidgets())
            if (qobject_cast<MainWindow *>(w) && w->isVisible())
                ++n;
        return n;
    }
    // Answer the next question box with Yes.
    static void answerYes()
    {
        auto *t = new QTimer;
        QObject::connect(t, &QTimer::timeout, [t] {
            if (auto *box = qobject_cast<QMessageBox *>(QApplication::activeModalWidget())) {
                box->button(QMessageBox::Yes)->click();
                t->deleteLater();
            }
        });
        t->start(10);
    }
    static void closeExtraWindows(QWidget *keep)
    {
        for (QWidget *w : QApplication::topLevelWidgets())
            if (w != keep && qobject_cast<MainWindow *>(w) && w->isVisible())
                w->close();
    }

private slots:
    void initTestCase()
    {
        QVERIFY(m_dir.isValid());
        write("docs/alpha report.pdf", 3000);
        write("docs/beta.txt", 10);
        write("src/Alpha.rs", 500);
        write("src/gamma.rs", 20);
        write("music/song.mp3", 9000);
        QDir().mkpath(m_dir.path() + "/run");
        const QString sock = m_dir.path() + "/run/s";
        qputenv("TRAWLRD_SOCKET", sock.toLocal8Bit());
        QString bin = qEnvironmentVariable("TRAWLRD_BIN", QCoreApplication::applicationDirPath() + "/../../../target/release/trawlrd");
        m_daemon.setProcessChannelMode(QProcess::ForwardedErrorChannel);
        m_daemon.start(bin, {"--root", tree(), "--socket", sock, "--db", m_dir.path() + "/run/index.db"});
        QVERIFY2(m_daemon.waitForStarted(), qPrintable("cannot start " + bin));
        QTRY_VERIFY(QFile::exists(sock));

        QSettings().clear();
        m_win = new MainWindow(QString());
        m_win->resize(1000, 600);
        m_win->show();
        QVERIFY(QTest::qWaitForWindowExposed(m_win));
        m_search = m_win->findChild<QLineEdit *>();
        m_view = m_win->findChild<ResultsView *>();
        m_store = m_win->findChild<ResultStore *>();
        QVERIFY(m_search && m_view && m_store);
        // 5 files + 3 folders
        QTRY_COMPARE_WITH_TIMEOUT(status(), QStringLiteral("8 items"), 10000);
    }

    void searchAsYouType()
    {
        QTest::keyClicks(m_search, "alpha");
        QTRY_COMPARE(status(), QStringLiteral("2 items"));
        QCOMPARE(nameAt(0), QStringLiteral("alpha report.pdf"));
        QCOMPARE(nameAt(1), QStringLiteral("Alpha.rs"));
        QCOMPARE(m_win->windowTitle(), QStringLiteral("alpha - Trawlr"));
    }

    void keyboardNavigationAndSelection()
    {
        QTest::keyClick(m_search, Qt::Key_Down);
        QVERIFY(m_view->hasFocus());
        QCOMPARE(m_view->selection().count(), 1);
        QCOMPARE(m_view->currentRow(), 0);
        QCOMPARE(status(), tree() + "/docs/alpha report.pdf");
        QTest::keyClick(m_view, Qt::Key_Down, Qt::ShiftModifier);
        QCOMPARE(m_view->selection().count(), 2);
        QCOMPARE(status(), QStringLiteral("2 of 2 selected"));
        QTest::keyClick(m_view, Qt::Key_Up);
        QCOMPARE(m_view->selection().count(), 1);
        QCOMPARE(m_view->currentRow(), 0);
    }

    void copyFullPath()
    {
        QApplication::clipboard()->clear();
        QTest::keyClick(m_view, Qt::Key_C, Qt::ControlModifier | Qt::ShiftModifier);
        QTRY_COMPARE(QApplication::clipboard()->text(), tree() + "/docs/alpha report.pdf");
        QTest::keyClick(m_view, Qt::Key_C, Qt::ControlModifier);
        QTRY_COMPARE(QApplication::clipboard()->mimeData()->urls().value(0), QUrl::fromLocalFile(tree() + "/docs/alpha report.pdf"));
    }

    void typingInTheListContinuesTheSearch()
    {
        QTest::keyClicks(m_view, " rs");
        QVERIFY(m_search->hasFocus());
        QCOMPARE(m_search->text(), QStringLiteral("alpha rs"));
        QTRY_COMPARE(status(), QStringLiteral("1 item"));
        QCOMPARE(nameAt(0), QStringLiteral("Alpha.rs"));
    }

    void escapeClearsThenFocuses()
    {
        QTest::keyClick(m_search, Qt::Key_Escape);
        QCOMPARE(m_search->text(), QString());
        QTRY_COMPARE(status(), QStringLiteral("8 items"));
    }

    void sortByClickingAHeader()
    {
        QHeaderView *h = m_view->header();
        const int x = h->sectionViewportPosition(ResultsView::ColSize) + 10;
        QTest::mouseClick(h->viewport(), Qt::LeftButton, {}, QPoint(x, h->height() / 2));
        // Size sorts largest first on the first click.
        QTRY_COMPARE(nameAt(0), QStringLiteral("song.mp3"));
        QCOMPARE(nameAt(1), QStringLiteral("alpha report.pdf"));
        QCOMPARE(h->sortIndicatorSection(), int(ResultsView::ColSize));
        QCOMPARE(h->sortIndicatorOrder(), Qt::DescendingOrder);
        QTest::mouseClick(h->viewport(), Qt::LeftButton, {}, QPoint(x, h->height() / 2));
        QTRY_COMPARE(h->sortIndicatorOrder(), Qt::AscendingOrder);
        QTRY_COMPARE(nameAt(7), QStringLiteral("song.mp3"));
        QTest::mouseClick(h->viewport(), Qt::LeftButton, {}, QPoint(h->sectionViewportPosition(ResultsView::ColName) + 10, h->height() / 2));
        QTRY_COMPARE(nameAt(0), QStringLiteral("alpha report.pdf"));
    }

    void columnsFitTheirContents()
    {
        QHeaderView *h = m_view->header();
        const int vw = m_view->viewport()->width();
        // On by default: everything fits the window, Name takes the slack.
        QVERIFY(m_view->autoFitColumns());
        QTRY_COMPARE(h->length(), vw);
        const int path = h->sectionSize(ResultsView::ColPath);
        QVERIFY2(path > m_view->fontMetrics().horizontalAdvance(tree()) && path < vw / 2, qPrintable(QString::number(path)));
        QVERIFY(h->sectionSize(ResultsView::ColSize) < 140);
        QVERIFY(h->sectionSize(ResultsView::ColName) >= vw - path - h->sectionSize(ResultsView::ColSize) - h->sectionSize(ResultsView::ColDate));
        // A wider window goes to Name alone.
        const int nameBefore = h->sectionSize(ResultsView::ColName);
        m_win->resize(m_win->width() + 200, m_win->height());
        QTRY_COMPARE(h->sectionSize(ResultsView::ColName), nameBefore + 200);
        QCOMPARE(h->sectionSize(ResultsView::ColPath), path);
        m_win->resize(m_win->width() - 200, m_win->height());
        QTRY_COMPARE(h->sectionSize(ResultsView::ColName), nameBefore);

        // Off: Path stretches, and double-clicking a column's edge fits it.
        QAction *fit = action("Fit Columns to Contents");
        QVERIFY(fit && fit->isChecked());
        fit->trigger();
        QVERIFY(!m_view->autoFitColumns());
        QCOMPARE(h->sectionResizeMode(ResultsView::ColPath), QHeaderView::Stretch);
        h->resizeSection(ResultsView::ColName, 60);
        const int edge = h->sectionViewportPosition(ResultsView::ColName) + h->sectionSize(ResultsView::ColName) - 1;
        QTest::mouseDClick(h->viewport(), Qt::LeftButton, {}, QPoint(edge, h->height() / 2));
        const int want = m_view->fontMetrics().horizontalAdvance("alpha report.pdf") + 24;
        QTRY_VERIFY2(h->sectionSize(ResultsView::ColName) >= want && h->sectionSize(ResultsView::ColName) < want + 60,
                     qPrintable(QString::number(h->sectionSize(ResultsView::ColName))));
        fit->trigger();
        QVERIFY(m_view->autoFitColumns());
        QTRY_COMPARE(h->length(), vw);
    }

    void mouseSelection()
    {
        const int rh = m_view->rowHeight();
        QTest::mouseClick(m_view->viewport(), Qt::LeftButton, {}, QPoint(60, rh * 2 + rh / 2));
        QCOMPARE(m_view->selection().rows(10), QList<qint64>{2});
        QTest::mouseClick(m_view->viewport(), Qt::LeftButton, Qt::ControlModifier, QPoint(60, rh * 4 + rh / 2));
        QCOMPARE(m_view->selection().rows(10), (QList<qint64>{2, 4}));
        QTest::mouseClick(m_view->viewport(), Qt::LeftButton, Qt::ShiftModifier, QPoint(60, rh * 6 + rh / 2));
        QCOMPARE(m_view->selection().rows(10), (QList<qint64>{4, 5, 6}));
        // Clicking below the last row clears the selection.
        QTest::mouseClick(m_view->viewport(), Qt::LeftButton, {}, QPoint(60, rh * 12));
        QVERIFY(m_view->selection().isEmpty());
    }

    void liveUpdatesKeepTheSelection()
    {
        m_search->setText("rs");
        QTRY_COMPARE(status(), QStringLiteral("2 items"));
        QTest::keyClick(m_search, Qt::Key_Down);
        QTest::keyClick(m_view, Qt::Key_Down);
        QCOMPARE(nameAt(m_view->currentRow()), QStringLiteral("gamma.rs"));

        // A file that sorts before the selected one appears on disk.
        write("src/delta.rs", 5);
        QTRY_COMPARE(m_store->total(), 3);
        QTRY_COMPARE(nameAt(1), QStringLiteral("delta.rs"));
        // The same file is still selected, though it moved down a row.
        QTRY_COMPARE(m_view->selection().rows(10), QList<qint64>{2});
        QCOMPARE(nameAt(2), QStringLiteral("gamma.rs"));

        QVERIFY(QFile::rename(tree() + "/src/delta.rs", tree() + "/src/renamed.rs"));
        QTRY_COMPARE(nameAt(2), QStringLiteral("renamed.rs"));
        QTRY_COMPARE(m_view->selection().rows(10), QList<qint64>{1});

        // The selected file is deleted: it goes, and so does the selection.
        QVERIFY(QFile::remove(tree() + "/src/gamma.rs"));
        QTRY_COMPARE(m_store->total(), 2);
        QTRY_VERIFY(m_view->selection().isEmpty());
        QCOMPARE(nameAt(0), QStringLiteral("Alpha.rs"));
        QCOMPARE(nameAt(1), QStringLiteral("renamed.rs"));
    }

    void optionsAndErrors()
    {
        m_search->setText("ALPHA");
        QTRY_COMPARE(status(), QStringLiteral("2 items"));
        action("Match Case")->trigger();
        QTRY_COMPARE(status(), QStringLiteral("0 items"));
        action("Match Case")->trigger();
        QTRY_COMPARE(status(), QStringLiteral("2 items"));

        action("Enable Regex")->trigger();
        m_search->setText("^(alpha|song).*\\.(pdf|mp3)$");
        QTRY_COMPARE(status(), QStringLiteral("2 items"));
        m_search->setText("(");
        QTRY_VERIFY2(status().startsWith("regex:"), qPrintable(status()));
        action("Enable Regex")->trigger();
        m_search->setText("ext:mp3");
        QTRY_COMPARE(status(), QStringLiteral("1 item"));

        action("Folder")->trigger();
        m_search->setText("s");
        QTRY_COMPARE(status(), QStringLiteral("3 items")); // docs, music, src
        action("All")->trigger();
        m_search->clear();
    }

    void doubleClickAndShiftOpen()
    {
        UrlCatcher catcher;
        QDesktopServices::setUrlHandler("file", &catcher, "open");
        m_search->setText("beta");
        QTRY_COMPARE(status(), QStringLiteral("1 item"));
        QTRY_COMPARE(nameAt(0), QStringLiteral("beta.txt"));
        const QUrl beta = QUrl::fromLocalFile(tree() + "/docs/beta.txt");
        const int y = m_view->rowHeight() / 2;
        QHeaderView *h = m_view->header();
        const QPoint inName(h->sectionViewportPosition(ResultsView::ColName) + 40, y);
        const QPoint inPath(h->sectionViewportPosition(ResultsView::ColPath) + 40, y);

        // The name opens the item, and the window stays.
        QTest::mouseDClick(m_view->viewport(), Qt::LeftButton, {}, inName);
        QTRY_COMPARE(catcher.urls, QList<QUrl>{beta});
        QTest::qWait(400);
        QVERIFY(m_win->isVisible() && !m_win->isMinimized());

        // The path opens the item too, when set to.
        QSettings().setValue("general/pathDoubleClick", "item");
        catcher.urls.clear();
        QTest::mouseDClick(m_view->viewport(), Qt::LeftButton, {}, inPath);
        QTRY_COMPARE(catcher.urls, QList<QUrl>{beta});
        QSettings().remove("general/pathDoubleClick");
        QVERIFY(OptionsDialog::pathDoubleClickOpensFolder());

        // Shift+Enter from the search box opens the top result and puts the window away.
        catcher.urls.clear();
        m_view->selectOnly(0);
        m_search->setFocus();
        QTest::keyClick(m_search, Qt::Key_Return, Qt::ShiftModifier);
        QTRY_COMPARE(catcher.urls, QList<QUrl>{beta});
        QTRY_VERIFY(!m_win->isVisible() || m_win->isMinimized());
        m_win->bringToFront();
        QTRY_VERIFY(m_win->isVisible() && !m_win->isMinimized());

        // Shift+double-click does the same.
        catcher.urls.clear();
        QTest::mouseDClick(m_view->viewport(), Qt::LeftButton, Qt::ShiftModifier, inName);
        QTRY_COMPARE(catcher.urls, QList<QUrl>{beta});
        QTRY_VERIFY(!m_win->isVisible() || m_win->isMinimized());
        m_win->bringToFront();
        QTRY_VERIFY(m_win->isVisible() && !m_win->isMinimized());
        QTRY_VERIFY(m_win->isActiveWindow());

        QDesktopServices::unsetUrlHandler("file");
        m_search->clear();
        QTRY_COMPARE(status(), QStringLiteral("8 items"));
    }

    void terminalPlans()
    {
        const Terminal::Plan open = Terminal::plan("konsole", {}, "/tmp/a b", {});
        QCOMPARE(open.program, QStringLiteral("konsole"));
        QCOMPARE(open.arguments, (QStringList{"--workdir", "/tmp/a b"}));
        QCOMPARE(open.workingDirectory, QStringLiteral("/tmp/a b"));
        const Terminal::Plan run = Terminal::plan("gnome-terminal", {}, "/d", {"x", "y z"});
        QCOMPARE(run.arguments, (QStringList{"--working-directory=/d", "--", "x", "y z"}));
        const Terminal::Plan custom = Terminal::plan("custom", "foot -D {dir} {cmd} --last", "/d", {"a", "b c"});
        QCOMPARE(custom.program, QStringLiteral("foot"));
        QCOMPARE(custom.arguments, (QStringList{"-D", "/d", "a", "b c", "--last"}));
        const Terminal::Plan appended = Terminal::plan("custom", "\"my term\" -e", "/d", {"a"});
        QCOMPARE(appended.program, QStringLiteral("my term"));
        QCOMPARE(appended.arguments, (QStringList{"-e", "a"}));
        QVERIFY(Terminal::plan("custom", "", "/d", {}).isNull());
        QCOMPARE(Terminal::choices().first().key, QStringLiteral("auto"));
        QCOMPARE(Terminal::choices().last().key, QStringLiteral("custom"));
    }

    void runInTerminalShowsOutputThenAShell()
    {
        // Not executable, run through its #! line; a quote and a space in the name.
        const QString script = m_dir.path() + "/it's a test.sh";
        QFile f(script);
        QVERIFY(f.open(QIODevice::WriteOnly));
        f.write("#!/bin/sh\necho \"hello from $0\"\nexit 3\n");
        f.close();
        const QStringList cmd = Terminal::runCommand(script);
        QCOMPARE(cmd.mid(0, 2), (QStringList{"sh", "-c"}));
        QCOMPARE(cmd.last(), script);
        QVERIFY(cmd.contains("/bin/sh"));
        QProcess p;
        QProcessEnvironment env = QProcessEnvironment::systemEnvironment();
        env.insert("SHELL", "/bin/echo"); // the shell left open afterwards; echo just says "-i"
        p.setProcessEnvironment(env);
        p.start(cmd.first(), cmd.mid(1));
        QVERIFY(p.waitForFinished(5000));
        const QString out = QString::fromUtf8(p.readAllStandardOutput());
        QVERIFY2(out.contains("hello from " + script), qPrintable(out));
        QVERIFY2(out.contains("[exited with code 3]"), qPrintable(out));
        QVERIFY2(out.trimmed().endsWith("-i"), qPrintable(out));
    }

    void aSecondLaunchRaisesTheOpenWindow()
    {
        QSettings().setValue("general/allowMultipleWindows", false);
        QCOMPARE(windowCount(), 1);
        LaunchRequest r;
        r.search = "song";
        MainWindow::handleLaunch(r);
        QCOMPARE(windowCount(), 1);
        QCOMPARE(m_search->text(), QStringLiteral("song"));
        QVERIFY(m_search->hasFocus());
        QVERIFY(m_search->hasSelectedText()); // typing replaces it
        QTRY_COMPARE(nameAt(0), QStringLiteral("song.mp3"));

        // With no search given, the old one is kept and selected.
        MainWindow::handleLaunch(LaunchRequest());
        QCOMPARE(windowCount(), 1);
        QCOMPARE(m_search->text(), QStringLiteral("song"));
        m_search->clear();
    }

    void multipleWindowsWhenAllowedOrAskedFor()
    {
        QSettings().setValue("general/allowMultipleWindows", true);
        MainWindow::handleLaunch(LaunchRequest());
        QCOMPARE(windowCount(), 2);
        QSettings().setValue("general/allowMultipleWindows", false);
        MainWindow::handleLaunch(LaunchRequest()); // raises, however many there are
        QCOMPARE(windowCount(), 2);
        LaunchRequest r;
        r.newWindow = true; // --new-window
        MainWindow::handleLaunch(r);
        QCOMPARE(windowCount(), 3);
        closeExtraWindows(m_win);
        QTRY_COMPARE(windowCount(), 1);
    }

    void instanceHandsTheLaunchOver()
    {
        const QString path = m_dir.path() + "/run/instance";
        QCOMPARE(int(Instance::forward(path, LaunchRequest())), int(Instance::NoInstance));
        // A socket file left behind by a crash is not an instance.
        QFile stale(path);
        QVERIFY(stale.open(QIODevice::WriteOnly));
        stale.close();
        QCOMPARE(int(Instance::forward(path, LaunchRequest())), int(Instance::NoInstance));

        Instance inst;
        QVERIFY(inst.listen(path));
        QList<LaunchRequest> got;
        connect(&inst, &Instance::requested, this, [&got](const LaunchRequest &r) { got << r; });
        LaunchRequest r;
        r.search = "two words";
        r.newWindow = true;
        r.activationToken = "tok-1";
        std::atomic<int> result{-1};
        std::thread t([&] { result = int(Instance::forward(path, r)); });
        QTRY_VERIFY(result >= 0);
        t.join();
        QCOMPARE(result.load(), int(Instance::Handled));
        QCOMPARE(got.size(), 1);
        QCOMPARE(got[0].search, QStringLiteral("two words"));
        QVERIFY(got[0].newWindow);
        QCOMPARE(got[0].activationToken, QStringLiteral("tok-1"));
    }

    void shortcutsCanBeChanged()
    {
        const QKeySequence orig(Qt::CTRL | Qt::Key_I), mine(Qt::CTRL | Qt::ALT | Qt::Key_M);
        QCOMPARE(action("Match Case")->shortcut(), orig);
        Shortcuts::save("search.matchCase", mine, orig);
        MainWindow::applyShortcutsToAll();
        QCOMPARE(action("Match Case")->shortcut(), mine);
        QVERIFY(!action("Match Case")->isChecked());
        m_win->activateWindow();
        QVERIFY(QTest::qWaitForWindowActive(m_win)); // shortcuts only fire in the active window
        QTest::keyClick(m_win, Qt::Key_M, Qt::ControlModifier | Qt::AltModifier);
        QVERIFY(action("Match Case")->isChecked());
        QTest::keyClick(m_win, Qt::Key_I, Qt::ControlModifier); // the old key does nothing now
        QVERIFY(action("Match Case")->isChecked());
        action("Match Case")->trigger();

        // A removed shortcut stays removed; going back to the default forgets the override.
        Shortcuts::save("search.matchCase", QKeySequence(), orig);
        MainWindow::applyShortcutsToAll();
        QVERIFY(action("Match Case")->shortcut().isEmpty());
        Shortcuts::save("search.matchCase", orig, orig);
        MainWindow::applyShortcutsToAll();
        QCOMPARE(action("Match Case")->shortcut(), orig);
        QVERIFY(!QSettings().contains("shortcuts/search.matchCase"));
    }

    void optionsDialogEditsShortcuts()
    {
        const QList<CommandInfo> cmds = {
            {"t.one", "Test > One", QKeySequence(Qt::CTRL | Qt::Key_1)},
            {"t.two", "Test > Two", QKeySequence()},
        };
        QSettings().remove("shortcuts/t.one");
        QSettings().remove("shortcuts/t.two");
        OptionsDialog dlg(cmds);
        auto *list = dlg.findChild<QTreeWidget *>("commandList");
        auto *keys = dlg.findChild<QListWidget *>("keyList");
        auto *multi = dlg.findChild<QCheckBox *>("allowMultiple");
        QVERIFY(list && keys && multi);
        QCOMPARE(list->topLevelItemCount(), 2);
        const QString one = QKeySequence(Qt::CTRL | Qt::Key_1).toString(QKeySequence::NativeText);
        QCOMPARE(list->topLevelItem(0)->text(1), one);

        // Give "Two" the key "One" has: it moves.
        list->setCurrentItem(list->topLevelItem(1));
        dlg.addShortcut("t.two", QKeySequence(Qt::CTRL | Qt::Key_1));
        QVERIFY(list->topLevelItem(0)->text(1).isEmpty());
        QCOMPARE(list->topLevelItem(1)->text(1), one);
        QCOMPARE(keys->count(), 1);

        // A command can have several keys.
        dlg.addShortcut("t.two", QKeySequence(Qt::CTRL | Qt::Key_2));
        QCOMPARE(keys->count(), 2);
        QVERIFY(list->topLevelItem(1)->font(1).bold()); // changed from its default

        multi->setChecked(true);
        dlg.accept();
        QCOMPARE(Shortcuts::effective("t.one", cmds[0].defaultKey), Shortcuts::Keys());
        const Shortcuts::Keys two{QKeySequence(Qt::CTRL | Qt::Key_1), QKeySequence(Qt::CTRL | Qt::Key_2)};
        QCOMPARE(Shortcuts::effective("t.two", cmds[1].defaultKey), two);
        QVERIFY(OptionsDialog::allowMultipleWindows());

        // Restore Defaults on the Keyboard page puts both back (once saved).
        OptionsDialog again(cmds);
        again.showPage("Keyboard");
        QCOMPARE(again.currentPage(), QStringLiteral("Keyboard"));
        again.restorePageDefaults();
        again.accept();
        QCOMPARE(Shortcuts::effective("t.one", cmds[0].defaultKey), Shortcuts::Keys{cmds[0].defaultKey});
        QVERIFY(!QSettings().contains("shortcuts/t.one"));
        QVERIFY(!QSettings().contains("shortcuts/t.two"));
        QSettings().setValue("general/allowMultipleWindows", false);
    }

    void severalKeysWorkInTheWindow()
    {
        const QKeySequence orig(Qt::CTRL | Qt::Key_I), extra(Qt::CTRL | Qt::ALT | Qt::Key_J);
        Shortcuts::save("search.matchCase", Shortcuts::Keys{orig, extra}, orig);
        MainWindow::applyShortcutsToAll();
        QCOMPARE(action("Match Case")->shortcuts().size(), 2);
        m_win->activateWindow();
        QVERIFY(QTest::qWaitForWindowActive(m_win));
        QTest::keyClick(m_win, Qt::Key_J, Qt::ControlModifier | Qt::AltModifier);
        QVERIFY(action("Match Case")->isChecked());
        QTest::keyClick(m_win, Qt::Key_I, Qt::ControlModifier);
        QVERIFY(!action("Match Case")->isChecked());
        Shortcuts::save("search.matchCase", Shortcuts::Keys{orig}, orig);
        MainWindow::applyShortcutsToAll();
        QVERIFY(!QSettings().contains("shortcuts/search.matchCase"));
    }

    void optionsOpenWithCtrlComma()
    {
        QCOMPARE(action("Options…")->shortcut(), QKeySequence(Qt::CTRL | Qt::Key_Comma));
        m_win->activateWindow();
        QVERIFY(QTest::qWaitForWindowActive(m_win));
        bool opened = false;
        QTimer::singleShot(50, [&opened] {
            if (auto *dlg = qobject_cast<OptionsDialog *>(QApplication::activeModalWidget())) {
                opened = true;
                dlg->reject();
            }
        });
        QTest::keyClick(m_win, Qt::Key_Comma, Qt::ControlModifier);
        QVERIFY(opened);
    }

    void optionsSearch()
    {
        OptionsDialog dlg(QList<CommandInfo>{
            {"file.copyPath", "File > Copy Full Path", QKeySequence(Qt::CTRL | Qt::SHIFT | Qt::Key_C)},
            {"search.regex", "Search > Enable Regex", QKeySequence(Qt::CTRL | Qt::Key_R)},
        });
        dlg.show();
        QVERIFY(QTest::qWaitForWindowActive(&dlg));
        auto *search = dlg.findChild<QLineEdit *>("settingsSearch");
        auto *pages = dlg.findChild<QTreeWidget *>("pageList");
        auto *list = dlg.findChild<QTreeWidget *>("commandList");
        auto *regex = dlg.findChild<QCheckBox *>("regex");
        QVERIFY(search && pages && list && regex);
        auto visiblePages = [pages] {
            QStringList out;
            for (int i = 0; i < pages->topLevelItemCount(); ++i)
                if (!pages->topLevelItem(i)->isHidden())
                    out << pages->topLevelItem(i)->text(0);
            return out;
        };
        QCOMPARE(visiblePages(), (QStringList{"General", "View", "Search", "Terminal", "Keyboard"}));

        // "/" goes to the search box from elsewhere in the dialog.
        pages->setFocus();
        QTest::keyClick(pages, Qt::Key_Slash);
        QVERIFY(search->hasFocus());

        QTest::keyClicks(search, "regular");
        QCOMPARE(visiblePages(), QStringList{"Search"}); // the regex switch's text and the page's keywords
        QCOMPARE(dlg.currentPage(), QStringLiteral("Search"));
        QVERIFY(!regex->styleSheet().isEmpty()); // highlighted

        search->clear();
        QTest::keyClicks(search, "copy path");
        QCOMPARE(dlg.currentPage(), QStringLiteral("Keyboard"));
        QVERIFY(!list->topLevelItem(0)->isHidden());
        QVERIFY(list->topLevelItem(1)->isHidden()); // only the matching command
        QVERIFY(regex->styleSheet().isEmpty());

        search->clear();
        QTest::keyClicks(search, "dark");
        QCOMPARE(dlg.currentPage(), QStringLiteral("View"));

        search->clear();
        QTest::keyClicks(search, "zzzz");
        QVERIFY(visiblePages().isEmpty());
        QVERIFY(dlg.findChild<QLabel *>("noMatch")->isVisible());

        // The first Esc clears the search, it does not close the dialog.
        QTest::keyClick(search, Qt::Key_Escape);
        QVERIFY(search->text().isEmpty());
        QVERIFY(dlg.isVisible());
        QCOMPARE(visiblePages().size(), 5);
        QVERIFY(!list->topLevelItem(1)->isHidden());
        dlg.reject();
    }

    void toggleHidesTheActiveWindow()
    {
        QSettings().setValue("general/clearSearchOnShow", false);
        m_search->setText("song");
        m_win->activateWindow();
        QVERIFY(QTest::qWaitForWindowActive(m_win));
        LaunchRequest r;
        r.toggle = true;
        r.activationToken = "tok-2";
        MainWindow::handleLaunch(r);
        // Hidden, not minimized: off the taskbar and Alt+Tab, but still there.
        QVERIFY(!m_win->isVisible());
        QCOMPARE(windowCount(), 0);
        MainWindow::handleLaunch(r);
        QTRY_VERIFY(m_win->isVisible());
        QVERIFY(!m_win->isMinimized());
        QCOMPARE(windowCount(), 1);
        QVERIFY(qEnvironmentVariableIsEmpty("XDG_ACTIVATION_TOKEN")); // never left behind
        QCOMPARE(m_search->text(), QStringLiteral("song"));

        // "Start with an empty search" when brought to the front.
        QSettings().setValue("general/clearSearchOnShow", true);
        MainWindow::handleLaunch(LaunchRequest());
        QVERIFY(m_search->text().isEmpty());
        QSettings().setValue("general/clearSearchOnShow", false);
    }

    void survivesADaemonRestart()
    {
        QTRY_COMPARE(m_store->total(), 8); // 5 files, 3 folders
        m_daemon.terminate();
        QVERIFY(m_daemon.waitForFinished());
        QTRY_VERIFY(status().startsWith("Not connected"));
        write("docs/made while down.txt", 1);
        m_daemon.start();
        QVERIFY(m_daemon.waitForStarted());
        // Reconnects by itself and shows the file created in the meantime.
        QTRY_COMPARE_WITH_TIMEOUT(status(), QStringLiteral("9 items"), 15000);
    }

    void cleanupTestCase()
    {
        delete m_win;
        m_daemon.terminate();
        m_daemon.waitForFinished();
    }
};

QTEST_MAIN(GuiTest)
#include "tst_gui.moc"
