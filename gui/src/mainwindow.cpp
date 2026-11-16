#include "mainwindow.h"

#include "optionsdialog.h"
#include "resultstore.h"
#include "icons.h"
#include "resultsview.h"
#include "terminal.h"
#include "theme.h"
#include "widgets.h"
#include "windowactivation.h"

#include <QActionGroup>
#include <QApplication>
#include <QClipboard>
#include <QCloseEvent>
#include <QDesktopServices>
#include <QDialog>
#include <QDrag>
#include <QFile>
#include <QHeaderView>
#include <QJsonArray>
#include <QLabel>
#include <QLineEdit>
#include <QLocale>
#include <QMenu>
#include <QMenuBar>
#include <QMessageBox>
#include <QMimeData>
#include <QPointer>
#include <QPalette>
#include <QRegularExpression>
#include <QSettings>
#include <QStyleHints>
#include <QStatusBar>
#include <QSystemTrayIcon>
#include <QWindow>
#include <memory>
#include <QUrl>
#include <QVBoxLayout>
#include <QGridLayout>
#include <QHBoxLayout>
#include <QPushButton>
#include <QScrollArea>
#include <QToolButton>

static const char *const kColumnSort[] = {"name", "path", "size", "date"};

// Most recently used first. Entries disappear by themselves when a window is deleted.
static QList<QPointer<MainWindow>> &windows()
{
    static QList<QPointer<MainWindow>> list;
    return list;
}

static MainWindow *lastUsedWindow()
{
    for (const QPointer<MainWindow> &w : std::as_const(windows()))
        if (w)
            return w;
    return nullptr;
}

// The plain words of a search, for bolding in the Name column. Anything with
// an operator or a `name:` prefix is not a plain word and is left alone.
static QStringList highlightTerms(const QString &text, bool regex)
{
    QStringList terms;
    if (regex)
        return terms;
    static const QRegularExpression split(QStringLiteral("\"([^\"]*)\"|([^\\s|<>]+)"));
    auto it = split.globalMatch(text);
    while (it.hasNext()) {
        const auto m = it.next();
        QString t = m.captured(1).isEmpty() ? m.captured(2) : m.captured(1);
        if (t.isEmpty() || t.startsWith(QLatin1Char('!')) || t.contains(QLatin1Char(':')))
            continue;
        // For a path term, what can show up in a name is its last component.
        t = t.section(QRegularExpression(QStringLiteral("[/\\\\]")), -1);
        for (const QString &piece : t.split(QRegularExpression(QStringLiteral("[*?]")), Qt::SkipEmptyParts))
            terms.append(piece);
    }
    return terms;
}

MainWindow::MainWindow(const QString &initialSearch, QWidget *parent) : QMainWindow(parent)
{
    Theme::ensure();
    m_client = new Client(this);
    m_store = new ResultStore(m_client, this);

    auto *central = new QWidget(this);
    auto *layout = new QVBoxLayout(central);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->setSpacing(0);

    // Top: the search field and the menu button, then the kind filters.
    auto *header = new QWidget(central);
    auto *headerLayout = new QVBoxLayout(header);
    headerLayout->setContentsMargins(14, 12, 14, 10);
    headerLayout->setSpacing(10);
    auto *top = new QHBoxLayout;
    top->setSpacing(6);
    m_searchBox = new SearchBox(header);
    m_search = m_searchBox->edit();
    m_search->setPlaceholderText(tr("Search files and folders"));
    m_search->installEventFilter(this);
    top->addWidget(m_searchBox, 1);
    m_menuButton = new QToolButton(header);
    m_menuButton->setIcon(Icons::icon(Icons::Menu, Icons::Dim));
    m_menuButton->setIconSize(QSize(20, 20));
    m_menuButton->setFixedSize(42, 42);
    m_menuButton->setToolTip(tr("Menu"));
    m_menuButton->setPopupMode(QToolButton::InstantPopup);
    m_menuButton->setFocusPolicy(Qt::NoFocus);
    top->addWidget(m_menuButton);
    headerLayout->addLayout(top);
    m_chipRow = new QHBoxLayout;
    m_chipRow->setContentsMargins(2, 0, 0, 0);
    m_chipRow->setSpacing(6);
    headerLayout->addLayout(m_chipRow);

    m_busy = new BusyBar(central);
    m_view = new ResultsView(m_store, central);
    m_view->installEventFilter(this);
    layout->addWidget(header);
    layout->addWidget(m_busy);
    layout->addWidget(m_view, 1);
    setCentralWidget(central);

    // Footer: what is listed or selected, how long the search took, and the index.
    m_countLabel = new QLabel(this);
    m_countLabel->setTextFormat(Qt::PlainText);
    m_timeLabel = new QLabel(this);
    m_timeLabel->setObjectName("queryTime");
    m_timeLabel->setTextFormat(Qt::PlainText);
    m_pill = new StatusPill(this);
    statusBar()->setSizeGripEnabled(false);
    statusBar()->setContentsMargins(8, 0, 8, 0);
    statusBar()->addWidget(m_countLabel);
    statusBar()->addWidget(m_timeLabel, 1);
    statusBar()->addPermanentWidget(m_pill);
    connect(m_pill, &StatusPill::clicked, this, [this] {
        if (m_client->isConnected()) {
            m_wantStatusDialog = true;
            m_client->requestStatus();
        }
    });
    connect(m_store, &ResultStore::busyChanged, m_busy, &BusyBar::setBusy);

    buildMenus();

    QSettings s;
    m_matchCase->setChecked(s.value("search/matchCase", false).toBool());
    m_wholeWord->setChecked(s.value("search/wholeWord", false).toBool());
    m_matchPath->setChecked(s.value("search/matchPath", false).toBool());
    m_regex->setChecked(s.value("search/regex", false).toBool());
    setSort(s.value("view/sort", "name").toString(), s.value("view/descending", false).toBool());
    resize(1100, 700);
    setMinimumSize(560, 360);
    restoreGeometry(s.value("window/geometry").toByteArray());
    // Another window at the same place would hide the first completely.
    if (lastUsedWindow())
        move(pos() + QPoint(32, 32));
    windows().prepend(this);
    m_view->header()->restoreState(s.value("view/columns").toByteArray());
    m_view->setAutoFitColumns(OptionsDialog::autoFitColumns());
    m_view->header()->setSortIndicatorShown(true);
    m_view->header()->setSectionsClickable(true);

    connect(m_search, &QLineEdit::textChanged, this, &MainWindow::runSearch);
    for (QAction *a : {m_matchCase, m_wholeWord, m_matchPath, m_regex})
        connect(a, &QAction::toggled, this, &MainWindow::runSearch);

    connect(m_view, &ResultsView::activated, this, &MainWindow::activate);
    connect(m_view, &ResultsView::selectionChanged, this, &MainWindow::updateStatusBar);
    connect(m_view, &ResultsView::contextMenuRequested, this, &MainWindow::showContextMenu);
    connect(m_view, &ResultsView::dragRequested, this, &MainWindow::startDrag);
    connect(m_view, &ResultsView::textTyped, this, [this](const QString &t) {
        m_search->setFocus();
        m_search->insert(t);
    });
    connect(m_view, &ResultsView::sortRequested, this, [this](int column) {
        const QString key = QLatin1String(kColumnSort[column]);
        // Biggest and newest first is what one wants from those columns.
        const bool descFirst = key == QLatin1String("size") || key == QLatin1String("date");
        setSort(key, key == m_sortKey ? !m_desc : descFirst);
        runSearch();
    });

    connect(m_store, &ResultStore::statsChanged, this, &MainWindow::updateStatusBar);
    connect(m_store, &ResultStore::statsChanged, this, [this] {
        if (m_store->indexState() != QLatin1String("ready") && !m_statusTimer.isActive())
            m_statusTimer.start();
    });

    // The daemon says "your results changed" once; re-run the search a moment
    // later, less eagerly the more expensive the search is.
    m_refreshTimer.setSingleShot(true);
    connect(&m_refreshTimer, &QTimer::timeout, m_store, &ResultStore::refresh);
    connect(m_client, &Client::indexChanged, this, [this] {
        if (!m_refreshTimer.isActive())
            m_refreshTimer.start(qBound(120, int(m_store->lastMs() * 6), 3000));
    });

    m_statusTimer.setInterval(1000);
    connect(&m_statusTimer, &QTimer::timeout, m_client, &Client::requestStatus);
    connect(m_client, &Client::statusReply, this, [this](const QJsonObject &st) {
        m_indexState = st.value("state").toString();
        m_scanned = qint64(st.value("scanned").toDouble());
        if (m_indexState != QLatin1String("scanning") && m_indexState != QLatin1String("checking") && m_indexState != QLatin1String("starting"))
            m_statusTimer.stop();
        updateIndexPill();
        updateStatusBar();
        if (m_wantStatusDialog) {
            m_wantStatusDialog = false;
            showIndexStatus(st);
        }
    });
    connect(m_client, &Client::connectedChanged, this, [this](bool up) {
        m_refusal.clear();
        if (up) {
            m_client->requestStatus();
            m_statusTimer.start();
        } else {
            m_statusTimer.stop();
            m_indexState.clear();
        }
        updateIndexPill();
        updateStatusBar();
    });
    connect(m_client, &Client::refused, this, [this](const QString &msg) {
        m_refusal = msg;
        updateIndexPill();
        updateStatusBar();
    });

    updateTrayIcon();

    m_search->setText(initialSearch);
    runSearch();
    m_client->start();
    m_search->setFocus();
    updateIndexPill();
    updateStatusBar();
}

void MainWindow::buildMenus()
{
    // Every action with an id can be given another key in Tools > Options.
    auto reg = [this](QMenu *menu, const QString &id, QAction *a, const QKeySequence &key) {
        m_commands.append({id, a, key, menu->title().remove(QLatin1Char('&')) + QStringLiteral(" > ") + a->text().remove(QLatin1Char('&'))});
    };
    auto add = [this, reg](QMenu *menu, const QString &id, const QString &text, const QKeySequence &key, std::function<void()> fn, bool needsSelection = false) {
        QAction *a = menu->addAction(text);
        connect(a, &QAction::triggered, this, fn);
        if (needsSelection)
            m_selectionActions.append(a);
        reg(menu, id, a, key);
        return a;
    };

    QMenu *file = menuBar()->addMenu(tr("&File"));
    // Return itself is handled by the list and the search box, so that it
    // keeps working as "accept" inside dialogs.
    add(file, "file.newWindow", tr("&New Window"), QKeySequence(Qt::CTRL | Qt::Key_N), [] { openNew(QString()); })->setIcon(QIcon::fromTheme("window-new"));
    file->addSeparator();
    add(file, "file.open", tr("&Open"), QKeySequence(), [this] { openSelected(); }, true)->setIcon(QIcon::fromTheme("document-open"));
    add(file, "file.openAndHide", tr("Open and &Hide Window"), QKeySequence(Qt::SHIFT | Qt::Key_Return), [this] { openAndHide(); });
    add(file, "file.openFolder", tr("Open Containing &Folder"), QKeySequence(Qt::CTRL | Qt::Key_Return), [this] { openContainingFolder(); }, true)->setIcon(QIcon::fromTheme("document-open-folder"));
    add(file, "file.openTerminal", tr("Open in &Terminal"), QKeySequence(Qt::CTRL | Qt::Key_T), [this] { openInTerminal(); }, true)->setIcon(QIcon::fromTheme("utilities-terminal"));
    add(file, "file.runTerminal", tr("R&un in Terminal"), QKeySequence(Qt::CTRL | Qt::SHIFT | Qt::Key_T), [this] { runInTerminal(); }, true)->setIcon(QIcon::fromTheme("system-run", QIcon::fromTheme("utilities-terminal")));
    file->addSeparator();
    add(file, "file.copy", tr("&Copy"), QKeySequence(QKeySequence::Copy), [this] { copyFiles(); }, true)->setIcon(QIcon::fromTheme("edit-copy"));
    add(file, "file.copyPath", tr("Copy Full &Path"), QKeySequence(Qt::CTRL | Qt::SHIFT | Qt::Key_C), [this] { copyPaths(false); }, true);
    add(file, "file.copyName", tr("Copy &Name"), QKeySequence(), [this] { copyPaths(true); }, true);
    file->addSeparator();
    m_trash = add(file, "file.trash", tr("Move to &Trash"), QKeySequence(Qt::Key_Delete), [this] { trashSelected(); }, true);
    m_trash->setIcon(QIcon::fromTheme("user-trash"));
    add(file, "file.properties", tr("P&roperties"), QKeySequence(Qt::ALT | Qt::Key_Return), [this] { showProperties(); }, true)->setIcon(QIcon::fromTheme("document-properties"));
    file->addSeparator();
    add(file, "file.quit", tr("&Quit"), QKeySequence(QKeySequence::Quit), [this] { close(); })->setIcon(QIcon::fromTheme("application-exit"));

    QMenu *edit = menuBar()->addMenu(tr("&Edit"));
    add(edit, "edit.selectAll", tr("Select &All"), QKeySequence(), [this] { m_view->selectAll(); m_view->setFocus(); });
    add(edit, "edit.focusSearch", tr("&Focus Search Box"), QKeySequence(Qt::CTRL | Qt::Key_L), [this] { m_search->setFocus(); m_search->selectAll(); });

    QMenu *search = menuBar()->addMenu(tr("&Search"));
    auto toggle = [search, reg](const QString &id, const QString &text, const QKeySequence &key) {
        QAction *a = search->addAction(text);
        a->setCheckable(true);
        reg(search, id, a, key);
        return a;
    };
    m_matchCase = toggle("search.matchCase", tr("Match &Case"), QKeySequence(Qt::CTRL | Qt::Key_I));
    m_wholeWord = toggle("search.wholeWord", tr("Match Whole &Word"), QKeySequence(Qt::CTRL | Qt::Key_B));
    m_matchPath = toggle("search.matchPath", tr("Match &Path"), QKeySequence(Qt::CTRL | Qt::Key_U));
    m_regex = toggle("search.regex", tr("Enable &Regex"), QKeySequence(Qt::CTRL | Qt::Key_R));
    search->addSeparator();
    m_filterGroup = new QActionGroup(this);
    const QList<QPair<QString, QString>> filters = {
        {tr("A&ll"), QString()}, {tr("&Audio"), "audio:"}, {tr("C&ompressed"), "zip:"}, {tr("&Document"), "doc:"},
        {tr("E&xecutable"), "exe:"},    {tr("&Folder"), "folder:"}, {tr("P&icture"), "pic:"},   {tr("&Video"), "video:"},
    };
    for (const auto &f : filters) {
        QAction *a = search->addAction(f.first);
        a->setCheckable(true);
        a->setData(f.second);
        m_filterGroup->addAction(a);
        reg(search, QStringLiteral("filter.") + (f.second.isEmpty() ? QStringLiteral("all") : f.second.chopped(1)), a, QKeySequence());
        connect(a, &QAction::triggered, this, &MainWindow::runSearch);
    }
    m_filterGroup->actions().first()->setChecked(true);

    QMenu *view = menuBar()->addMenu(tr("&View"));
    QMenu *sort = view->addMenu(tr("&Sort By"));
    m_sortGroup = new QActionGroup(this);
    const QList<QPair<QString, QString>> sorts = {
        {tr("&Name"), "name"}, {tr("&Path"), "path"}, {tr("&Size"), "size"}, {tr("&Date Modified"), "date"}, {tr("&Extension"), "ext"},
    };
    for (const auto &k : sorts) {
        QAction *a = sort->addAction(k.first);
        a->setCheckable(true);
        a->setData(k.second);
        m_sortGroup->addAction(a);
        reg(sort, QStringLiteral("sort.") + k.second, a, QKeySequence());
        connect(a, &QAction::triggered, this, [this, a] { setSort(a->data().toString(), m_desc); runSearch(); });
    }
    sort->addSeparator();
    m_sortDesc = sort->addAction(tr("D&escending"));
    m_sortDesc->setCheckable(true);
    reg(sort, "sort.descending", m_sortDesc, QKeySequence());
    connect(m_sortDesc, &QAction::triggered, this, [this](bool on) { setSort(m_sortKey, on); runSearch(); });

    // Light/dark: "system" follows the desktop and tracks changes to it.
    QMenu *theme = view->addMenu(tr("&Theme"));
    m_themeGroup = new QActionGroup(this);
    const QString saved = OptionsDialog::theme();
    const QList<QPair<QString, QString>> themes = {{tr("&System"), "system"}, {tr("&Light"), "light"}, {tr("&Dark"), "dark"}};
    for (const auto &t : themes) {
        QAction *a = theme->addAction(t.first);
        a->setCheckable(true);
        a->setData(t.second);
        a->setChecked(t.second == saved);
        m_themeGroup->addAction(a);
        connect(a, &QAction::triggered, this, [key = t.second] {
            applyTheme(key);
            applySettingsToAll(false); // the other windows' menus
        });
    }
    view->addSeparator();
    m_autoFitColumns = add(view, "view.autoFitColumns", tr("&Fit Columns to Contents"), QKeySequence(), [this] {
        QSettings().setValue("view/autoFitColumns", m_autoFitColumns->isChecked());
        applySettingsToAll(false);
    });
    m_autoFitColumns->setCheckable(true);
    m_autoFitColumns->setChecked(OptionsDialog::autoFitColumns());
#ifndef Q_OS_MACOS
    m_showMenuBar = add(view, "view.menuBar", tr("Show &Menu Bar"), QKeySequence(Qt::CTRL | Qt::Key_M), [this] {
        QSettings().setValue("view/menuBar", m_showMenuBar->isChecked());
        applySettingsToAll(false);
    });
    m_showMenuBar->setCheckable(true);
    m_showMenuBar->setChecked(Theme::showMenuBar());
#endif

    QMenu *tools = menuBar()->addMenu(tr("&Tools"));
    QAction *options = add(tools, "tools.options", tr("&Options…"), QKeySequence(Qt::CTRL | Qt::Key_Comma), [this] { showOptions(); });
    options->setIcon(QIcon::fromTheme("configure"));

    QMenu *index = menuBar()->addMenu(tr("&Index"));
    add(index, "index.status", tr("&Status…"), QKeySequence(), [this] { m_wantStatusDialog = true; m_client->requestStatus(); });
    add(index, "index.check", tr("&Check Against Disk Now"), QKeySequence(), [this] { m_client->requestRescan(false); m_statusTimer.start(); });
    add(index, "index.rebuild", tr("&Rebuild From Scratch"), QKeySequence(), [this] {
        if (QMessageBox::question(this, tr("Rebuild index"), tr("Throw the index away and scan every folder again?")) == QMessageBox::Yes) {
            m_client->requestRescan(true);
            m_statusTimer.start();
        }
    });

    QMenu *help = menuBar()->addMenu(tr("&Help"));
    QAction *syntax = add(help, "help.syntax", tr("Search &Syntax"), QKeySequence(Qt::Key_F1), [this] { showSyntaxHelp(); });
    QAction *about = add(help, "help.about", tr("&About"), QKeySequence(), [this] {
        QMessageBox box(this);
        box.setWindowTitle(tr("About"));
        box.setIconPixmap(QApplication::windowIcon().pixmap(QSize(72, 72), devicePixelRatioF()));
        box.setTextFormat(Qt::RichText);
        box.setText(tr("<h2 style='margin-bottom:2px'>Trawlr</h2><p>Version %1</p>"
                       "<p>Instant file search with a live, always-current index.</p>"
                       "<p style='color:%2'>Inspired by voidtools' Everything for Windows. Not affiliated with voidtools.</p>")
                        .arg(QApplication::applicationVersion(), Theme::colors().textDim.name()));
        box.exec();
    });

    // Every command also belongs to the window itself, so its key works
    // while the menu bar is hidden.
    for (const Command &c : std::as_const(m_commands))
        addAction(c.action);

    // The menu behind the button next to the search field: the commands one
    // looks for there, and the full menus one level down.
    auto *main = new QMenu(this);
    main->addAction(m_commands.first().action); // New Window
    main->addSeparator();
    main->addMenu(search);
    main->addMenu(sort);
    main->addMenu(theme);
    main->addAction(m_autoFitColumns);
    if (m_showMenuBar)
        main->addAction(m_showMenuBar);
    main->addSeparator();
    main->addMenu(index);
    main->addAction(options);
    main->addSeparator();
    main->addAction(syntax);
    main->addAction(about);
    main->addSeparator();
    for (const Command &c : std::as_const(m_commands))
        if (c.id == QLatin1String("file.quit"))
            main->addAction(c.action);
    m_menuButton->setMenu(main);
    menuBar()->setVisible(Theme::showMenuBar());

    m_searchBox->addToggle(m_matchCase, Icons::MatchCase, tr("Match case"));
    m_searchBox->addToggle(m_wholeWord, Icons::WholeWord, tr("Match whole words"));
    m_searchBox->addToggle(m_matchPath, Icons::MatchPath, tr("Match the full path, not just the name"));
    m_searchBox->addToggle(m_regex, Icons::Regex, tr("Regular expression"));
    buildChips();

    applyShortcuts();
}

void MainWindow::buildChips()
{
    // The kind filters of the Search menu, as a row of chips.
    const QList<QPair<QString, QString>> chips = {
        {tr("All"), QString()},          {tr("Folders"), "folder:"}, {tr("Documents"), "doc:"}, {tr("Pictures"), "pic:"},
        {tr("Audio"), "audio:"},         {tr("Video"), "video:"},    {tr("Archives"), "zip:"},  {tr("Programs"), "exe:"},
    };
    for (const auto &chip : chips) {
        QAction *action = nullptr;
        for (QAction *a : m_filterGroup->actions())
            if (a->data().toString() == chip.second)
                action = a;
        if (!action)
            continue;
        auto *b = new QToolButton(this);
        b->setObjectName("chip");
        b->setText(chip.first);
        b->setCheckable(true);
        b->setChecked(action->isChecked());
        b->setFocusPolicy(Qt::NoFocus);
        b->setCursor(Qt::PointingHandCursor);
        connect(b, &QToolButton::clicked, action, [b, action] {
            b->setChecked(true); // clicking the chosen chip again keeps it chosen
            action->trigger();
            if (!action->isChecked())
                action->setChecked(true);
        });
        connect(action, &QAction::toggled, b, &QToolButton::setChecked);
        m_chipRow->addWidget(b);
    }
    m_chipRow->addStretch(1);
}

QList<CommandInfo> MainWindow::commands() const
{
    QList<CommandInfo> out;
    for (const Command &c : m_commands)
        out.append({c.id, c.label, c.defaultKey});
    return out;
}

void MainWindow::applyShortcuts()
{
    for (const Command &c : std::as_const(m_commands))
        c.action->setShortcuts(Shortcuts::effective(c.id, c.defaultKey));
}

void MainWindow::applySettings(bool searchSwitches)
{
    applyShortcuts();
    const QString theme = OptionsDialog::theme();
    for (QAction *a : m_themeGroup->actions())
        a->setChecked(a->data().toString() == theme);
    menuBar()->setVisible(Theme::showMenuBar());
    if (m_showMenuBar)
        m_showMenuBar->setChecked(Theme::showMenuBar());
    const bool fit = OptionsDialog::autoFitColumns();
    if (m_autoFitColumns)
        m_autoFitColumns->setChecked(fit);
    if (fit != m_view->autoFitColumns())
        m_view->setAutoFitColumns(fit);
    if (!searchSwitches)
        return;
    QSettings s;
    m_matchCase->setChecked(s.value("search/matchCase", false).toBool());
    m_wholeWord->setChecked(s.value("search/wholeWord", false).toBool());
    m_matchPath->setChecked(s.value("search/matchPath", false).toBool());
    m_regex->setChecked(s.value("search/regex", false).toBool());
}

void MainWindow::showOptions()
{
    // The Search page shows this window's switches.
    QSettings s;
    s.setValue("search/matchCase", m_matchCase->isChecked());
    s.setValue("search/wholeWord", m_wholeWord->isChecked());
    s.setValue("search/matchPath", m_matchPath->isChecked());
    s.setValue("search/regex", m_regex->isChecked());

    OptionsDialog dlg(commands(), this);
    connect(&dlg, &OptionsDialog::applied, this, [] {
        applyTheme(OptionsDialog::theme());
        applySettingsToAll();
    });
    dlg.exec();
}

void MainWindow::applySettingsToAll(bool searchSwitches)
{
    updateTrayIcon();
    for (const QPointer<MainWindow> &w : std::as_const(windows()))
        if (w)
            w->applySettings(searchSwitches);
}

MainWindow *MainWindow::openNew(const QString &search)
{
    auto *w = new MainWindow(search);
    w->setAttribute(Qt::WA_DeleteOnClose);
    w->show();
    return w;
}

void MainWindow::handleLaunch(const LaunchRequest &r)
{
    // Toggle acts on the window you used last, whether or not more are allowed.
    MainWindow *w = r.newWindow || (!r.toggle && OptionsDialog::allowMultipleWindows()) ? nullptr : lastUsedWindow();
    if (!w) {
        openNew(r.search)->bringToFront(r.activationToken);
        return;
    }
    if (r.toggle && r.search.isEmpty() && w->isVisible() && !w->isMinimized() && w->isActiveWindow()) {
        w->hideAway(r.activationToken);
        return;
    }
    if (!r.search.isEmpty())
        w->m_search->setText(r.search);
    else if (OptionsDialog::clearSearchOnShow())
        w->m_search->clear();
    w->m_search->setFocus();
    w->m_search->selectAll(); // typing replaces the old search
    w->bringToFront(r.activationToken);
}

static bool onWayland()
{
    return QGuiApplication::platformName().startsWith(QLatin1String("wayland"));
}

void MainWindow::bringToFront(const QString &activationToken)
{
    if (isMinimized())
        setWindowState(windowState() & ~Qt::WindowMinimized);
    show();
    raise();
    activateWithToken(activationToken);
    // From the background, the above may be refused (always, on Wayland). If
    // the window is still not the active one a moment later, ask KDE's window
    // manager directly. Twice, because a new window needs a moment to appear.
    for (int delay : {150, 600}) {
        QTimer::singleShot(delay, this, [this] {
            if (isVisible() && !isActiveWindow())
                WindowActivation::viaKWin(windowTitle());
        });
    }
}

void MainWindow::activateWithToken(const QString &token)
{
    if (token.isEmpty() || !onWayland()) {
        activateWindow();
        return;
    }
    // Qt hands XDG_ACTIVATION_TOKEN to the desktop with the next activation
    // request. A window that was hidden is not on screen for a moment after
    // show(), and the desktop cannot activate what it has not got yet.
    auto attempt = std::make_shared<int>(0);
    auto *timer = new QTimer(this);
    timer->setInterval(20);
    // True once done (activated, or given up after 1.5 s).
    auto step = [this, timer, token, attempt] {
        QWindow *win = windowHandle();
        if (!(win && win->isExposed()) && ++*attempt < 75)
            return false;
        timer->deleteLater();
        if (!win || !isVisible())
            return true;
        qputenv("XDG_ACTIVATION_TOKEN", token.toUtf8());
        win->requestActivate();
        qunsetenv("XDG_ACTIVATION_TOKEN"); // used or not, it is good for one request only
        return true;
    };
    if (!step()) {
        connect(timer, &QTimer::timeout, this, step);
        timer->start();
    }
}

void MainWindow::hideAway(const QString &activationToken)
{
    // Spend the launch token on the window before it goes, or KDE keeps
    // bouncing the launch icon until it gives up waiting.
    if (!activationToken.isEmpty() && onWayland() && windowHandle() && windowHandle()->isExposed()) {
        qputenv("XDG_ACTIVATION_TOKEN", activationToken.toUtf8());
        windowHandle()->requestActivate();
        qunsetenv("XDG_ACTIVATION_TOKEN");
    }
    hide();
}

void MainWindow::toggleLastWindow()
{
    MainWindow *w = lastUsedWindow();
    if (!w)
        openNew(QString())->bringToFront();
    else if (w->isVisible() && !w->isMinimized() && w->isActiveWindow())
        w->hideAway();
    else
        w->bringToFront();
}

void MainWindow::updateTrayIcon()
{
    static QPointer<QSystemTrayIcon> tray;
    const bool want = OptionsDialog::showTrayIcon() && QSystemTrayIcon::isSystemTrayAvailable();
#ifdef Q_OS_WIN
    // On Windows the global hotkeys belong to this program: with a tray icon
    // to come back from, closing the last window keeps it running for them.
    QApplication::setQuitOnLastWindowClosed(!want);
#endif
    if (!want) {
        delete tray;
        return;
    }
    if (tray)
        return;
    tray = new QSystemTrayIcon(QApplication::windowIcon(), qApp);
    tray->setToolTip(QApplication::applicationDisplayName());
    auto *menu = new QMenu;
    connect(tray, &QObject::destroyed, menu, &QObject::deleteLater);
    menu->addAction(tr("&Show Trawlr"), [] {
        if (MainWindow *w = lastUsedWindow())
            w->bringToFront();
        else
            openNew(QString())->bringToFront();
    });
    menu->addAction(QIcon::fromTheme("window-new"), tr("&New Window"), [] { openNew(QString())->bringToFront(); });
    menu->addAction(QIcon::fromTheme("configure"), tr("&Options…"), [] {
        MainWindow *w = lastUsedWindow();
        if (!w)
            w = openNew(QString());
        w->bringToFront();
        w->showOptions();
    });
    menu->addSeparator();
    menu->addAction(QIcon::fromTheme("application-exit"), tr("E&xit"), [] {
        for (const QPointer<MainWindow> &w : QList<QPointer<MainWindow>>(windows()))
            if (w)
                w->close(); // each saves its settings
        QApplication::quit();
    });
    tray->setContextMenu(menu);
    connect(tray, &QSystemTrayIcon::activated, qApp, [](QSystemTrayIcon::ActivationReason why) {
        if (why == QSystemTrayIcon::Trigger)
            toggleLastWindow();
    });
    tray->show();
}

bool MainWindow::event(QEvent *e)
{
    if (e->type() == QEvent::WindowActivate) {
        auto &list = windows();
        list.removeAll(QPointer<MainWindow>(this));
        list.prepend(this);
    }
    return QMainWindow::event(e);
}

void MainWindow::setSort(const QString &key, bool desc)
{
    m_sortKey = key;
    m_desc = desc;
    for (QAction *a : m_sortGroup->actions())
        a->setChecked(a->data().toString() == key);
    m_sortDesc->setChecked(desc);
    int column = -1;
    for (int i = 0; i < 4; ++i)
        if (key == QLatin1String(kColumnSort[i]))
            column = i;
    m_view->header()->setSortIndicator(column, desc ? Qt::DescendingOrder : Qt::AscendingOrder);
}

void MainWindow::runSearch()
{
    QueryParams p;
    const QString text = m_search->text();
    const QString filter = m_filterGroup->checkedAction() ? m_filterGroup->checkedAction()->data().toString() : QString();
    p.regex = m_regex->isChecked();
    if (!filter.isEmpty()) {
        // In regex mode the whole box is one pattern, so the filter is added
        // as its own term and the pattern as an explicit `regex:` term.
        if (p.regex && !text.trimmed().isEmpty()) {
            p.text = filter + QStringLiteral(" regex:\"") + text + QLatin1Char('"');
            p.regex = false;
        } else {
            p.text = filter + QLatin1Char(' ') + text;
        }
    } else {
        p.text = text;
    }
    p.sort = m_sortKey;
    p.desc = m_desc;
    p.matchCase = m_matchCase->isChecked();
    p.matchPath = m_matchPath->isChecked();
    p.wholeWord = m_wholeWord->isChecked();
    m_view->setHighlightTerms(highlightTerms(text, m_regex->isChecked()));
    m_store->setQuery(p);
    setWindowTitle(text.trimmed().isEmpty() ? tr("Trawlr") : tr("%1 - Trawlr").arg(text.trimmed()));
}

void MainWindow::updateStatusBar()
{
    const QLocale loc;
    const qint64 selected = m_view->selection().count();
    const qint64 total = m_store->total();
    for (QAction *a : std::as_const(m_selectionActions))
        a->setEnabled(selected > 0);

    QString text;
    if (!m_refusal.isEmpty()) {
        text = m_refusal;
    } else if (!m_client->isConnected()) {
        text = tr("Not connected to the index service");
    } else if (!m_store->error().isEmpty()) {
        text = m_store->error();
    } else if (selected == 1) {
        const QList<Row> rows = m_view->loadedSelection(1);
        text = rows.isEmpty() ? tr("1 item selected") : rows.first().path();
    } else if (selected > 1) {
        text = tr("%1 of %2 selected").arg(loc.toString(selected), loc.toString(total));
    } else {
        text = total == 1 ? tr("1 item") : tr("%1 items").arg(loc.toString(total));
    }
    m_countLabel->setText(text);

    const bool searched = m_client->isConnected() && m_refusal.isEmpty() && m_store->error().isEmpty() && m_store->hasResults() && selected == 0;
    const double ms = m_store->lastMs();
    m_timeLabel->setText(!searched ? QString() : ms < 1 ? tr("in under 1 ms") : tr("in %1 ms").arg(loc.toString(qRound(ms))));

    // In place of an empty list, say why it is empty.
    const QString query = m_search->text().trimmed();
    const bool filtered = m_filterGroup->checkedAction() && !m_filterGroup->checkedAction()->data().toString().isEmpty();
    if (!m_refusal.isEmpty()) {
        m_view->setEmptyState(Icons::Warning, tr("No access to the index"), m_refusal);
    } else if (!m_client->isConnected()) {
        m_view->setEmptyState(Icons::Offline, tr("Can't reach the index service"),
                              tr("Searching needs the background service, trawlrd. Start it with\n"
                                 "%1\n"
                                 "and this window connects to it by itself.").arg(Platform::startServiceHint()));
    } else if (!m_store->error().isEmpty()) {
        m_view->setEmptyState(Icons::Warning, tr("That search didn't work"), m_store->error());
    } else if (m_indexState == QLatin1String("starting") || m_indexState == QLatin1String("scanning")) {
        m_view->setEmptyState(Icons::Search, tr("Building the index…"), tr("Results will show up here as soon as the first scan is done."));
    } else if (!query.isEmpty() || filtered) {
        m_view->setEmptyState(Icons::NoResults, query.isEmpty() ? tr("Nothing of this kind") : tr("No results for “%1”").arg(query),
                              tr("Check the spelling, try fewer words, or turn off some of the search switches. Press F1 for the search syntax."));
    } else {
        m_view->setEmptyState(Icons::Search, tr("Nothing indexed yet"), QString());
    }
}

void MainWindow::updateIndexPill()
{
    const QLocale loc;
    if (!m_refusal.isEmpty())
        m_pill->setState(StatusPill::Offline, tr("No access"), m_refusal);
    else if (!m_client->isConnected())
        m_pill->setState(StatusPill::Offline, tr("Offline"), tr("Not connected to the index service (trawlrd).\nIs it running?  %1").arg(Platform::startServiceHint()));
    else if (m_indexState == QLatin1String("scanning"))
        m_pill->setState(StatusPill::Working, tr("Indexing… %1").arg(loc.toString(m_scanned)), tr("Reading every folder for the first time"));
    else if (m_indexState == QLatin1String("checking"))
        m_pill->setState(StatusPill::Working, tr("Checking…"), tr("Comparing the index with the disk"));
    else if (m_indexState == QLatin1String("starting"))
        m_pill->setState(StatusPill::Working, tr("Loading…"), tr("Loading the saved index"));
    else
        m_pill->setState(StatusPill::Live, tr("Live"), tr("The index is up to date and follows changes as they happen.\nClick for details."));
}

void MainWindow::withSelection(int max, std::function<void(const QList<Row> &)> fn)
{
    const QList<qint64> rows = m_view->selection().rows(max);
    if (rows.isEmpty())
        return;
    m_store->collect(rows, [fn](const QList<Row> &r) {
        if (!r.isEmpty())
            fn(r);
    });
}

void MainWindow::openSelected(bool thenHide)
{
    withSelection(50, [this, thenHide](const QList<Row> &rows) {
        if (rows.size() > 8
            && QMessageBox::question(this, tr("Open"), tr("Open %1 items?").arg(rows.size())) != QMessageBox::Yes)
            return;
        for (const Row &r : rows)
            QDesktopServices::openUrl(QUrl::fromLocalFile(r.path()));
        if (thenHide)
            dismissAfterOpening();
    });
}

void MainWindow::activate(int column, Qt::KeyboardModifiers mods)
{
    const bool hide = mods & Qt::ShiftModifier;
    if (column == ResultsView::ColPath && OptionsDialog::pathDoubleClickOpensFolder())
        openContainingFolder(hide);
    else
        openSelected(hide);
}

void MainWindow::openAndHide()
{
    // From the search box with nothing chosen yet: the top result.
    if (m_view->selection().isEmpty())
        m_view->selectOnly(0);
    openSelected(true);
}

void MainWindow::dismissAfterOpening()
{
    // A moment's grace: the desktop hands focus to what was opened using this
    // window's launch token, which needs the window still to be there.
    QTimer::singleShot(250, this, [this] {
        if (OptionsDialog::showTrayIcon() && QSystemTrayIcon::isSystemTrayAvailable())
            hideAway();
        else
            showMinimized();
    });
}

void MainWindow::openInTerminal()
{
    withSelection(1, [this](const QList<Row> &rows) {
        const Row &r = rows.first();
        QString error;
        if (!Terminal::openIn(r.isDir ? r.path() : r.dir, &error))
            QMessageBox::warning(this, tr("Open in Terminal"), error);
    });
}

void MainWindow::runInTerminal()
{
    withSelection(1, [this](const QList<Row> &rows) {
        const Row &r = rows.first();
        QString error;
        const bool ok = r.isDir ? Terminal::openIn(r.path(), &error) : Terminal::run(r.path(), &error);
        if (!ok)
            QMessageBox::warning(this, tr("Run in Terminal"), error);
    });
}

static QStringList pathsOf(const QList<Row> &rows)
{
    QStringList paths;
    for (const Row &r : rows)
        paths << r.path();
    return paths;
}

void MainWindow::openContainingFolder(bool thenHide)
{
    withSelection(20, [this, thenHide](const QList<Row> &rows) {
        Platform::showInFileManager(this, pathsOf(rows), rows.first().dir);
        if (thenHide)
            dismissAfterOpening();
    });
}

void MainWindow::showProperties()
{
    withSelection(20, [this](const QList<Row> &rows) { Platform::showProperties(this, pathsOf(rows)); });
}

static QMimeData *mimeFor(const QList<Row> &rows)
{
    QList<QUrl> urls;
    QStringList paths;
    for (const Row &r : rows) {
        urls << QUrl::fromLocalFile(r.path());
        paths << r.path();
    }
    auto *mime = new QMimeData;
    mime->setUrls(urls);
    mime->setText(paths.join(QLatin1Char('\n')));
    return mime;
}

void MainWindow::copyFiles()
{
    // In the search box, Ctrl+C copies text as usual.
    if (m_search->hasFocus() && m_search->hasSelectedText()) {
        m_search->copy();
        return;
    }
    withSelection(10000, [](const QList<Row> &rows) { QApplication::clipboard()->setMimeData(mimeFor(rows)); });
}

void MainWindow::copyPaths(bool namesOnly)
{
    withSelection(100000, [namesOnly](const QList<Row> &rows) {
        QStringList out;
        for (const Row &r : rows)
            out << (namesOnly ? r.name : r.path());
        QApplication::clipboard()->setText(out.join(QLatin1Char('\n')));
    });
}

void MainWindow::trashSelected()
{
    // A bare key such as Delete is an editing key in the search box; with a
    // modifier it is still a command there.
    const QKeySequence k = m_trash->shortcut();
    if (m_search->hasFocus() && !k.isEmpty() && k[0].keyboardModifiers() == Qt::NoModifier)
        return;
    withSelection(10000, [this](const QList<Row> &rows) {
        const QString what = rows.size() == 1 ? tr("Move “%1” to the trash?").arg(rows.first().name) : tr("Move %1 items to the trash?").arg(rows.size());
        if (QMessageBox::question(this, tr("Move to Trash"), what, QMessageBox::Yes | QMessageBox::Cancel, QMessageBox::Yes) != QMessageBox::Yes)
            return;
        QStringList failed;
        for (const Row &r : rows)
            if (!QFile::moveToTrash(r.path()))
                failed << r.path();
        if (!failed.isEmpty())
            QMessageBox::warning(this, tr("Move to Trash"),
                                 tr("%1 of %2 items could not be moved to the trash, for example:\n%3").arg(failed.size()).arg(rows.size()).arg(failed.first()));
    });
}

void MainWindow::startDrag()
{
    const QList<Row> rows = m_view->loadedSelection(2000);
    if (rows.isEmpty())
        return;
    auto *drag = new QDrag(m_view);
    drag->setMimeData(mimeFor(rows));
    drag->exec(Qt::CopyAction | Qt::MoveAction | Qt::LinkAction, Qt::CopyAction);
}

void MainWindow::showContextMenu(const QPoint &globalPos)
{
    const QList<QStringList> groups = {
        {"file.open", "file.openFolder", "file.openTerminal", "file.runTerminal"},
        {"file.copy", "file.copyPath", "file.copyName"},
        {"file.trash", "file.properties"},
    };
    QMenu menu(this);
    for (const QStringList &group : groups) {
        if (!menu.isEmpty())
            menu.addSeparator();
        for (const QString &id : group)
            for (const Command &c : std::as_const(m_commands))
                if (c.id == id)
                    menu.addAction(c.action);
    }
    menu.exec(globalPos);
}

void MainWindow::showSyntaxHelp()
{
    struct Example {
        const char *code;
        QString what;
    };
    const QList<QPair<QString, QList<Example>>> sections = {
        {tr("Words"),
         {{"report 2024", tr("Names containing both words")},
          {"jpg|png", tr("Either word")},
          {"!tmp", tr("Leave out names containing the word")},
          {"\"annual report\"", tr("An exact phrase, spaces included")},
          {"<jpg|png> holiday", tr("Grouping")},
          {"*.pdf", tr("Wildcards * and ?; they must match the whole name")},
          {"docs/ report", tr("A term with a slash is matched against the full path")}}},
        {tr("Filters"),
         {{"ext:jpg;png", tr("By extension")},
          {"size:>10mb", tr("By size. Also size:1mb..2gb, and empty, tiny, small, medium, large, huge, gigantic")},
          {"dm:today", tr("By date modified. Also yesterday, thisweek, lastmonth, last2weeks, 2024-03, >2023, a..b")},
          {"folder:", tr("Only folders (file: for only files)")},
          {"pic:", tr("By kind: audio: video: pic: doc: zip: exe:")},
          {"parent:/etc", tr("Directly inside that folder")},
          {"empty:", tr("Empty folders")}}},
        {tr("For one term"),
         {{"case:README", tr("Match case")},
          {"path:src", tr("Match against the full path")},
          {"regex:^IMG_\\d+", tr("A regular expression")},
          {"ww:test", tr("Whole words only")},
          {"wfn:Makefile", tr("The whole file name")},
          {"startwith:IMG_", tr("Name starts with the text (endwith: for the end)")}}},
    };

    QDialog dlg(this);
    dlg.setWindowTitle(tr("Search Syntax"));
    dlg.resize(640, 660);
    auto *outer = new QVBoxLayout(&dlg);
    outer->setContentsMargins(22, 20, 22, 16);
    outer->setSpacing(4);
    auto *title = new QLabel(tr("Search syntax"), &dlg);
    QFont tf = title->font();
    tf.setPointSizeF(tf.pointSizeF() * 1.45);
    tf.setWeight(QFont::DemiBold);
    title->setFont(tf);
    outer->addWidget(title);
    auto *sub = new QLabel(tr("Combine any of these. Click an example to try it."), &dlg);
    sub->setObjectName("hint");
    outer->addWidget(sub);
    outer->addSpacing(10);

    auto *scroll = new QScrollArea(&dlg);
    scroll->setWidgetResizable(true);
    scroll->setFrameShape(QFrame::NoFrame);
    auto *body = new QWidget;
    body->setObjectName("syntaxBody");
    auto *grid = new QGridLayout(body);
    grid->setContentsMargins(0, 0, 8, 0);
    grid->setHorizontalSpacing(16);
    grid->setVerticalSpacing(7);
    grid->setColumnStretch(1, 1);
    int row = 0;
    for (const auto &section : sections) {
        auto *head = new QLabel(section.first.toUpper(), body);
        head->setObjectName("sectionTitle");
        grid->addWidget(head, row++, 0, 1, 2);
        for (const Example &ex : section.second) {
            const QString code = QString::fromUtf8(ex.code);
            auto *b = new QPushButton(code, body);
            b->setObjectName("example");
            b->setCursor(Qt::PointingHandCursor);
            b->setAutoDefault(false);
            b->setToolTip(tr("Search for %1").arg(code));
            connect(b, &QPushButton::clicked, &dlg, [this, &dlg, code] {
                m_search->setText(code);
                dlg.accept();
            });
            auto *what = new QLabel(ex.what, body);
            what->setWordWrap(true);
            what->setObjectName("hint");
            grid->addWidget(b, row, 0, Qt::AlignLeft | Qt::AlignVCenter);
            grid->addWidget(what, row, 1);
            ++row;
        }
        grid->setRowMinimumHeight(row++, 14);
    }
    grid->setRowStretch(row, 1);
    scroll->setWidget(body);
    outer->addWidget(scroll, 1);
    auto *close = new QPushButton(tr("Close"), &dlg);
    close->setDefault(true);
    connect(close, &QPushButton::clicked, &dlg, &QDialog::accept);
    auto *buttons = new QHBoxLayout;
    buttons->addStretch(1);
    buttons->addWidget(close);
    outer->addSpacing(8);
    outer->addLayout(buttons);
    dlg.exec();
    m_search->setFocus();
}

void MainWindow::showIndexStatus(const QJsonObject &s)
{
    const QLocale loc;
    QString html = tr("<p><b>%1</b> files and <b>%2</b> folders indexed, using %3 MB of memory.</p>")
                       .arg(loc.toString(qint64(s.value("files").toDouble())), loc.toString(qint64(s.value("folders").toDouble())))
                       .arg(qint64(s.value("memory_mb").toDouble()));
    html += QStringLiteral("<table cellspacing='4'>");
    for (const QJsonValue &v : s.value("filesystems").toArray()) {
        const QJsonObject f = v.toObject();
        const QString watch = f.value("watch").toString();
        const QString how = watch == QLatin1String("filesystem") ? tr("live, whole filesystem")
                            : watch == QLatin1String("per-folder") ? tr("live, folder by folder") : tr("not watched");
        html += QStringLiteral("<tr><td><b>%1</b></td><td>%2</td><td>%3</td></tr>").arg(f.value("mount").toString().toHtmlEscaped(), f.value("type").toString(), how);
    }
    html += QStringLiteral("</table>");
    html += tr("<p>%1 change events handled since the service started.</p>").arg(loc.toString(qint64(s.value("events").toDouble())));
    const qint64 unwatched = qint64(s.value("unwatched_folders").toDouble());
    if (unwatched > 0)
        html += tr("<p><b>%1 folders are not being watched</b> because the per-user limit on folder watches was reached. "
                   "Install trawlrd as a system service to watch whole filesystems instead.</p>").arg(loc.toString(unwatched));
    html += tr("<p>Service: trawlrd %1, %2 mode, %3.<br>Socket: %4</p>")
                .arg(s.value("version").toString(), s.value("mode").toString(), s.value("state").toString(), m_client->socketPath().toHtmlEscaped());
    QMessageBox box(this);
    box.setWindowTitle(tr("Index Status"));
    box.setTextFormat(Qt::RichText);
    box.setText(html);
    box.exec();
}

bool MainWindow::eventFilter(QObject *o, QEvent *e)
{
    if (o == m_search && e->type() == QEvent::KeyPress) {
        auto *ke = static_cast<QKeyEvent *>(e);
        switch (ke->key()) {
        case Qt::Key_Down:
        case Qt::Key_Up:
        case Qt::Key_PageDown:
        case Qt::Key_PageUp:
            // Arrow keys in the search box drive the list.
            m_view->setFocus();
            QApplication::sendEvent(m_view, e);
            return true;
        case Qt::Key_Return:
        case Qt::Key_Enter:
            if (ke->modifiers() & (Qt::ControlModifier | Qt::AltModifier))
                break;
            if (m_view->selection().isEmpty())
                m_view->selectOnly(0);
            else
                openSelected();
            m_view->setFocus();
            return true;
        case Qt::Key_Escape:
            if (m_search->text().isEmpty())
                close();
            else
                m_search->clear();
            return true;
        case Qt::Key_A:
            if (ke->modifiers() == Qt::ControlModifier && m_search->text().isEmpty()) {
                m_view->selectAll();
                return true;
            }
            break;
        default:
            break;
        }
    }
    if (o == m_view && e->type() == QEvent::KeyPress) {
        auto *ke = static_cast<QKeyEvent *>(e);
        if (ke->key() == Qt::Key_A && ke->modifiers() == Qt::ControlModifier) {
            m_view->selectAll();
            return true;
        }
        if (ke->key() == Qt::Key_Escape) {
            m_search->setFocus();
            m_search->selectAll();
            return true;
        }
    }
    return QMainWindow::eventFilter(o, e);
}

void MainWindow::applyTheme(const QString &key)
{
    QSettings().setValue("view/theme", key == QLatin1String("light") || key == QLatin1String("dark") ? key : QStringLiteral("system"));
    Theme::ensure();
    Theme::apply();
}

void MainWindow::closeEvent(QCloseEvent *e)
{
    QSettings s;
    s.setValue("window/geometry", saveGeometry());
    s.setValue("view/columns", m_view->header()->saveState());
    s.setValue("view/sort", m_sortKey);
    s.setValue("view/descending", m_desc);
    s.setValue("search/matchCase", m_matchCase->isChecked());
    s.setValue("search/wholeWord", m_wholeWord->isChecked());
    s.setValue("search/matchPath", m_matchPath->isChecked());
    s.setValue("search/regex", m_regex->isChecked());
    QMainWindow::closeEvent(e);
}
