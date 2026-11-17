#include "optionsdialog.h"
#include "platform.h"
#include "terminal.h"
#include "theme.h"

#include <QPainter>
#include <QPixmap>

#include <QCheckBox>
#include <QComboBox>
#include <QDialogButtonBox>
#include <QDir>
#include <QFileDialog>
#include <QFormLayout>
#include <QGroupBox>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QKeyEvent>
#include <QKeySequenceEdit>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QMessageBox>
#include <QPushButton>
#include <QRadioButton>
#include <QSettings>
#include <QShortcut>
#include <QStackedWidget>
#include <QTextDocumentFragment>
#include <QTreeWidget>
#include <QVBoxLayout>

static const QString kMultiple = QStringLiteral("general/allowMultipleWindows");
static const QString kTray = QStringLiteral("general/trayIcon");
static const QString kClearOnShow = QStringLiteral("general/clearSearchOnShow");
static const QString kTheme = QStringLiteral("view/theme");
static const QString kLastPage = QStringLiteral("options/page");
static const QString kGeometry = QStringLiteral("options/geometry");

static const char *const kSearchKeys[] = {"search/matchCase", "search/wholeWord", "search/matchPath", "search/regex"};
static const bool kSearchDefaults[] = {false, false, true, false};
static const QString kExcluded = QStringLiteral("search/excludedFolders");

// Explanatory text under a control: readable, but quieter than the controls.
static QLabel *hintLabel(const QString &text, QWidget *parent)
{
    auto *l = new QLabel(text, parent);
    l->setObjectName("hint");
    l->setWordWrap(true);
    return l;
}

static QKeySequenceEdit *keyEdit(QWidget *parent)
{
    auto *e = new QKeySequenceEdit(parent);
#if QT_VERSION >= QT_VERSION_CHECK(6, 5, 0)
    e->setMaximumSequenceLength(1);
#endif
#if QT_VERSION >= QT_VERSION_CHECK(6, 4, 0)
    e->setClearButtonEnabled(true);
#endif
    return e;
}

static QString plain(QString text)
{
    if (text.contains(QLatin1Char('<')))
        text = QTextDocumentFragment::fromHtml(text).toPlainText();
    return text.remove(QLatin1Char('&'));
}

bool OptionsDialog::allowMultipleWindows()
{
    QSettings s;
    s.sync(); // another window or process may have changed it
    return s.value(kMultiple, false).toBool();
}

bool OptionsDialog::clearSearchOnShow()
{
    QSettings s;
    s.sync();
    return s.value(kClearOnShow, false).toBool();
}

bool OptionsDialog::showTrayIcon()
{
    return QSettings().value(kTray, true).toBool();
}

bool OptionsDialog::pathDoubleClickOpensFolder()
{
    return QSettings().value(QStringLiteral("general/pathDoubleClick"), QStringLiteral("folder")).toString() != QLatin1String("item");
}

bool OptionsDialog::autoFitColumns()
{
    return QSettings().value(QStringLiteral("view/autoFitColumns"), true).toBool();
}

QStringList OptionsDialog::excludedFolders()
{
    return QSettings().value(kExcluded).toStringList();
}

void OptionsDialog::setExcludedFolders(const QStringList &folders)
{
    QStringList sorted = folders;
    sorted.removeDuplicates();
    sorted.sort(Qt::CaseInsensitive);
    QSettings().setValue(kExcluded, sorted);
}

QString OptionsDialog::theme()
{
    const QString t = QSettings().value(kTheme, QStringLiteral("system")).toString();
    return t == QLatin1String("light") || t == QLatin1String("dark") ? t : QStringLiteral("system");
}

OptionsDialog::OptionsDialog(const QList<CommandInfo> &commands, QWidget *parent) : QDialog(parent), m_commands(commands)
{
    setWindowTitle(tr("Options"));
    for (const CommandInfo &c : m_commands)
        m_keys.insert(c.id, Shortcuts::effective(c.id, c.defaultKey));

    // Left: the settings search and the page list.
    m_settingsSearch = new QLineEdit(this);
    m_settingsSearch->setObjectName("settingsSearch");
    m_settingsSearch->setPlaceholderText(tr("Search options  ( / )"));
    m_settingsSearch->setClearButtonEnabled(true);
    m_settingsSearch->installEventFilter(this);
    m_pageList = new QTreeWidget(this);
    m_pageList->setObjectName("pageList");
    m_pageList->setHeaderHidden(true);
    m_pageList->setRootIsDecorated(false);
    m_pageList->setUniformRowHeights(true);
    m_noMatch = hintLabel(tr("No options match."), this);
    m_noMatch->setObjectName("noMatch");
    m_noMatch->setAlignment(Qt::AlignHCenter);
    m_noMatch->hide();
    auto *left = new QVBoxLayout;
    left->setSpacing(10);
    left->addWidget(m_settingsSearch);
    left->addWidget(m_pageList, 1);
    left->addWidget(m_noMatch);
    auto *leftWidget = new QWidget(this);
    leftWidget->setLayout(left);
    left->setContentsMargins(0, 0, 0, 0);
    leftWidget->setFixedWidth(200);

    // Right: the page's title over the page.
    m_pageTitle = new QLabel(this);
    QFont tf = m_pageTitle->font();
    tf.setWeight(QFont::DemiBold);
    tf.setPointSizeF(tf.pointSizeF() * 1.4);
    m_pageTitle->setFont(tf);
    m_pageTitle->setContentsMargins(0, 0, 0, 8);
    m_stack = new QStackedWidget(this);
    auto *right = new QVBoxLayout;
    right->setContentsMargins(0, 0, 0, 0);
    right->addWidget(m_pageTitle);
    right->addWidget(m_stack, 1);

    auto *body = new QHBoxLayout;
    body->setSpacing(24);
    body->addWidget(leftWidget);
    body->addLayout(right, 1);

    addPage(tr("General"), tr("settings preferences startup launch instance single windows tray notification area background double click path folder"), buildGeneralPage(), [this] {
        m_multiple->setChecked(false);
        m_tray->setChecked(true);
        m_pathClick->setCurrentIndex(0);
    });
    addPage(tr("View"), tr("appearance theme colours colors dark mode light mode accent density compact rows menu bar columns column widths auto fit size contents"), buildViewPage(), [this] {
        m_theme->setCurrentIndex(0);
        m_autoFit->setChecked(true);
        m_accent->setCurrentIndex(qMax(0, m_accent->findData(QStringLiteral("indigo"))));
        m_density->setCurrentIndex(0);
        if (m_menuBar)
            m_menuBar->setChecked(false);
    });
    addPage(tr("Search"), tr("match case whole word path regex regular expression home exclude excluded hide hidden ignore folders"), buildSearchPage(), [this] {
        QCheckBox *boxes[] = {m_matchCase, m_wholeWord, m_matchPath, m_regex};
        for (int i = 0; i < 4; ++i)
            boxes[i]->setChecked(kSearchDefaults[i]);
        m_keepSearch->setChecked(true);
        m_excluded->clear();
    });
    addPage(tr("Terminal"), tr("terminal console shell konsole powershell command prompt cmd run script"), buildTerminalPage(), [this] {
        m_terminal->setCurrentIndex(0);
        m_terminalCustom->clear();
    });
    addPage(tr("Keyboard"), tr("keys keybindings key bindings shortcuts hotkeys accelerators"), buildKeyboardPage(), [this] {
        for (const CommandInfo &c : std::as_const(m_commands)) {
            m_keys[c.id] = Shortcuts::defaults(c.defaultKey);
            showRow(c.id);
        }
        commandChanged();
    });

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel | QDialogButtonBox::Apply | QDialogButtonBox::RestoreDefaults, this);
    buttons->button(QDialogButtonBox::RestoreDefaults)->setText(tr("Restore Defaults"));
    buttons->button(QDialogButtonBox::RestoreDefaults)->setToolTip(tr("Put the settings on this page back as they were when installed"));
    connect(buttons, &QDialogButtonBox::accepted, this, &OptionsDialog::accept);
    connect(buttons, &QDialogButtonBox::rejected, this, &QDialog::reject);
    connect(buttons->button(QDialogButtonBox::Apply), &QPushButton::clicked, this, [this] { apply(); });
    connect(buttons->button(QDialogButtonBox::RestoreDefaults), &QPushButton::clicked, this, &OptionsDialog::restorePageDefaults);

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(18, 18, 18, 14);
    layout->setSpacing(14);
    layout->addLayout(body, 1);
    layout->addWidget(buttons);

    connect(m_pageList, &QTreeWidget::currentItemChanged, this, [this](QTreeWidgetItem *it) {
        for (int i = 0; i < m_pages.size(); ++i) {
            if (m_pages[i].item == it) {
                m_stack->setCurrentIndex(i);
                m_pageTitle->setText(m_pages[i].title);
            }
        }
    });
    connect(m_settingsSearch, &QLineEdit::textChanged, this, &OptionsDialog::runSettingsSearch);

    // "/" jumps to the search from anywhere in the dialog. Text fields and
    // shortcut recorders take the key for themselves first, as they should.
    auto *slash = new QShortcut(QKeySequence(Qt::Key_Slash), this);
    connect(slash, &QShortcut::activated, this, [this] {
        m_settingsSearch->setFocus(Qt::ShortcutFocusReason);
        m_settingsSearch->selectAll();
    });

    for (int i = 0; i < m_pages.size(); ++i)
        indexPage(i);

    resize(820, 740);
    QSettings s;
    restoreGeometry(s.value(kGeometry).toByteArray());
    showPage(s.value(kLastPage, m_pages.first().title).toString());
    if (!m_pageList->currentItem())
        m_pageList->setCurrentItem(m_pages.first().item);
    m_settingsSearch->setFocus();
}

void OptionsDialog::addPage(const QString &title, const QString &keywords, QWidget *w, std::function<void()> restore)
{
    Page p;
    p.title = title;
    p.keywords = keywords;
    p.widget = w;
    p.item = new QTreeWidgetItem(m_pageList, {title});
    p.restoreDefaults = std::move(restore);
    m_stack->addWidget(w);
    m_pages.append(p);
}

void OptionsDialog::showPage(const QString &title)
{
    for (const Page &p : std::as_const(m_pages))
        if (p.title == title && !p.item->isHidden())
            m_pageList->setCurrentItem(p.item);
}

QString OptionsDialog::currentPage() const
{
    const int i = m_stack->currentIndex();
    return i >= 0 && i < m_pages.size() ? m_pages[i].title : QString();
}

void OptionsDialog::restorePageDefaults()
{
    const int i = m_stack->currentIndex();
    if (i >= 0 && i < m_pages.size() && m_pages[i].restoreDefaults)
        m_pages[i].restoreDefaults();
}

// ---- pages ----

QWidget *OptionsDialog::buildGeneralPage()
{
    auto *page = new QWidget;
    auto *layout = new QVBoxLayout(page);
    layout->setContentsMargins(0, 6, 0, 0);
    m_multiple = new QCheckBox(tr("Allow multiple windows"), page);
    m_multiple->setObjectName("allowMultiple");
    m_multiple->setChecked(allowMultipleWindows());
    auto *hint = hintLabel(tr("Off: starting Trawlr while a window is already open (from its hotkey, the launcher or a terminal) "
                              "brings that window to the front.\nOn: it opens another window."),
                           page);
    hint->setContentsMargins(22, 0, 0, 0);
    layout->addWidget(m_multiple);
    layout->addWidget(hint);
    layout->addSpacing(8);
    m_tray = new QCheckBox(tr("Show an icon in the system tray"), page);
    m_tray->setObjectName("trayIcon");
    m_tray->setChecked(showTrayIcon());
    auto *trayHint = hintLabel(tr("Click it to show or hide the window. The toggle window hotkey hides the window there, out of the taskbar "
                                  "and Alt+Tab, and Trawlr keeps running."),
                               page);
    trayHint->setContentsMargins(22, 0, 0, 0);
    layout->addWidget(m_tray);
    layout->addWidget(trayHint);
    if (Platform::hasStartAtLogin()) {
        layout->addSpacing(8);
        m_autostart = new QCheckBox(tr("Start Trawlr when I sign in"), page);
        m_autostart->setObjectName("startAtLogin");
        m_autostart->setChecked(Platform::startsAtLogin());
        auto *autoHint = hintLabel(tr("It starts in the tray, so the global hotkeys work from the start."), page);
        autoHint->setContentsMargins(22, 0, 0, 0);
        layout->addWidget(m_autostart);
        layout->addWidget(autoHint);
    }
    layout->addSpacing(14);

    auto *form = new QFormLayout;
    form->setHorizontalSpacing(14);
    m_pathClick = new QComboBox(page);
    m_pathClick->setObjectName("pathDoubleClick");
    m_pathClick->addItem(tr("Opens the containing folder"), QStringLiteral("folder"));
    m_pathClick->addItem(tr("Opens the item"), QStringLiteral("item"));
    m_pathClick->setCurrentIndex(pathDoubleClickOpensFolder() ? 0 : 1);
    auto *pathLabel = new QLabel(tr("Double-clicking a path:"), page);
    pathLabel->setBuddy(m_pathClick);
    form->addRow(pathLabel, m_pathClick);
    layout->addLayout(form);
    layout->addWidget(hintLabel(tr("Double-clicking anywhere else in a row opens the item. Hold Shift while double-clicking, "
                                   "or press Shift+Enter, to open it and then put this window out of the way."),
                                page));
    layout->addStretch(1);
    return page;
}

QWidget *OptionsDialog::buildTerminalPage()
{
    auto *page = new QWidget;
    auto *layout = new QVBoxLayout(page);
    layout->setContentsMargins(0, 6, 0, 0);
    auto *form = new QFormLayout;
    form->setHorizontalSpacing(14);
    form->setVerticalSpacing(10);
    m_terminal = new QComboBox(page);
    m_terminal->setObjectName("terminal");
    for (const Terminal::Choice &c : Terminal::choices())
        m_terminal->addItem(c.name, c.key);
    const int at = m_terminal->findData(Terminal::savedKey());
    m_terminal->setCurrentIndex(at >= 0 ? at : 0);
    auto *termLabel = new QLabel(tr("Terminal:"), page);
    termLabel->setBuddy(m_terminal);
    form->addRow(termLabel, m_terminal);
    m_terminalCustom = new QLineEdit(page);
    m_terminalCustom->setObjectName("terminalCustom");
    m_terminalCustom->setText(Terminal::savedCustom());
#ifdef Q_OS_WIN
    m_terminalCustom->setPlaceholderText(QStringLiteral("wt.exe -d {dir} {cmd}"));
#else
    m_terminalCustom->setPlaceholderText(QStringLiteral("alacritty --working-directory {dir} -e {cmd}"));
#endif
    auto *customLabel = new QLabel(tr("Custom command:"), page);
    customLabel->setBuddy(m_terminalCustom);
    form->addRow(customLabel, m_terminalCustom);
    layout->addLayout(form);
    auto syncCustom = [this] { m_terminalCustom->setEnabled(m_terminal->currentData().toString() == QLatin1String("custom")); };
    connect(m_terminal, &QComboBox::currentIndexChanged, this, syncCustom);
    syncCustom();
    layout->addSpacing(4);
#ifdef Q_OS_WIN
    const QString how = tr("Open in Terminal (Ctrl+T) starts the terminal in the item's folder. Run in Terminal (Ctrl+Shift+T) "
                           "runs the file there and keeps the window open, so its output can be read.");
#else
    const QString how = tr("Open in Terminal (Ctrl+T) starts the terminal in the item's folder. Run in Terminal (Ctrl+Shift+T) "
                           "runs the file there, using the interpreter its #! line names if it is not executable, then leaves "
                           "a shell open so the output can be read.");
#endif
    layout->addWidget(hintLabel(how, page));
    layout->addWidget(hintLabel(tr("In a custom command, {dir} is the folder and {cmd} the command to run; "
                                   "without {cmd}, the command goes at the end."),
                                page));
    layout->addStretch(1);
    return page;
}

// A round colour swatch for the accent list.
static QIcon swatch(const QColor &c)
{
    QPixmap pm(32, 32);
    pm.setDevicePixelRatio(2);
    pm.fill(Qt::transparent);
    QPainter p(&pm);
    p.setRenderHint(QPainter::Antialiasing);
    p.setPen(Qt::NoPen);
    p.setBrush(c);
    p.drawEllipse(QRectF(1.5, 1.5, 13, 13));
    return QIcon(pm);
}

QWidget *OptionsDialog::buildViewPage()
{
    auto *page = new QWidget;
    auto *layout = new QVBoxLayout(page);
    layout->setContentsMargins(0, 6, 0, 0);
    auto *form = new QFormLayout;
    form->setHorizontalSpacing(14);
    form->setVerticalSpacing(10);
    form->setLabelAlignment(Qt::AlignLeft | Qt::AlignVCenter);
    m_theme = new QComboBox(page);
    m_theme->setObjectName("theme");
    m_theme->addItem(tr("System"), QStringLiteral("system"));
    m_theme->addItem(tr("Light"), QStringLiteral("light"));
    m_theme->addItem(tr("Dark"), QStringLiteral("dark"));
    m_theme->setCurrentIndex(qMax(0, m_theme->findData(theme())));
    form->addRow(tr("Theme:"), m_theme);

    m_accent = new QComboBox(page);
    m_accent->setObjectName("accent");
    for (const QString &k : Theme::accentKeys())
        m_accent->addItem(swatch(Theme::accentColor(k)), Theme::accentName(k), k);
    m_accent->addItem(swatch(Theme::accentColor(QStringLiteral("system"))), tr("System accent"), QStringLiteral("system"));
    m_accent->setCurrentIndex(qMax(0, m_accent->findData(Theme::accentKey())));
    form->addRow(tr("Accent colour:"), m_accent);

    m_density = new QComboBox(page);
    m_density->setObjectName("density");
    m_density->addItem(tr("Comfortable"), QStringLiteral("comfortable"));
    m_density->addItem(tr("Compact"), QStringLiteral("compact"));
    m_density->setCurrentIndex(qMax(0, m_density->findData(Theme::densityKey())));
    form->addRow(tr("Rows:"), m_density);
    layout->addLayout(form);
    layout->addSpacing(4);
    layout->addWidget(hintLabel(tr("System follows the desktop's light or dark setting, and changes with it. "
                                   "Compact fits more results on screen."), page));
    layout->addSpacing(10);
    m_autoFit = new QCheckBox(tr("Fit column widths to their contents"), page);
    m_autoFit->setObjectName("autoFitColumns");
    m_autoFit->setChecked(autoFitColumns());
    layout->addWidget(m_autoFit);
    auto *fitHint = hintLabel(tr("Size, Modified and Path are kept as wide as the results on screen, and Name gets the rest. "
                                 "Turn this off to drag the columns yourself; double-clicking a column's edge then fits that column."), page);
    fitHint->setContentsMargins(27, 0, 0, 0);
    layout->addWidget(fitHint);
#ifndef Q_OS_MACOS
    layout->addSpacing(10);
    m_menuBar = new QCheckBox(tr("Show the menu bar"), page);
    m_menuBar->setObjectName("menuBar");
    m_menuBar->setChecked(Theme::showMenuBar());
    layout->addWidget(m_menuBar);
    auto *mbHint = hintLabel(tr("Every command is also in the menu next to the search field. Ctrl+M shows or hides the menu bar."), page);
    mbHint->setContentsMargins(27, 0, 0, 0);
    layout->addWidget(mbHint);
#endif
    layout->addStretch(1);
    return page;
}

QWidget *OptionsDialog::buildSearchPage()
{
    auto *page = new QWidget;
    auto *layout = new QVBoxLayout(page);
    layout->setContentsMargins(0, 6, 0, 0);

    auto *match = new QGroupBox(tr("Matching"), page);
    auto *ml = new QVBoxLayout(match);
    QSettings s;
    QCheckBox **boxes[] = {&m_matchCase, &m_wholeWord, &m_matchPath, &m_regex};
    const QString labels[] = {tr("Match case"), tr("Match whole words"), tr("Match path (search the full path, not just the name)"),
                              tr("Enable regex (the whole search is one regular expression)")};
    const char *const names[] = {"matchCase", "wholeWord", "matchPath", "regex"};
    for (int i = 0; i < 4; ++i) {
        *boxes[i] = new QCheckBox(labels[i], match);
        (*boxes[i])->setObjectName(QLatin1String(names[i]));
        (*boxes[i])->setChecked(s.value(QLatin1String(kSearchKeys[i]), kSearchDefaults[i]).toBool());
        ml->addWidget(*boxes[i]);
    }
    ml->addWidget(hintLabel(tr("The same switches as the Search menu. Changing them here changes every open window."), match));
    layout->addWidget(match);

    auto *front = new QGroupBox(tr("When a window is brought to the front"), page);
    auto *fl = new QVBoxLayout(front);
    m_keepSearch = new QRadioButton(tr("Keep the last search, selected so that typing replaces it"), front);
    m_clearSearch = new QRadioButton(tr("Start with an empty search"), front);
    m_clearSearch->setObjectName("clearSearchOnShow");
    (clearSearchOnShow() ? m_clearSearch : m_keepSearch)->setChecked(true);
    fl->addWidget(m_keepSearch);
    fl->addWidget(m_clearSearch);
    layout->addWidget(front);

    auto *ex = new QGroupBox(tr("Excluded folders"), page);
    auto *el = new QVBoxLayout(ex);
    el->addWidget(hintLabel(tr("These folders, and everything in them, are left out of the results. "
                               "Right-click a result and choose Exclude from Results to add one."), ex));
    auto *row = new QHBoxLayout;
    m_excluded = new QListWidget(ex);
    m_excluded->setObjectName("excludedFolders");
    m_excluded->setSelectionMode(QAbstractItemView::ExtendedSelection);
    m_excluded->setMinimumHeight(110);
    m_excluded->addItems(excludedFolders());
    row->addWidget(m_excluded, 1);
    auto *buttons = new QVBoxLayout;
    auto *add = new QPushButton(tr("Add…"), ex);
    auto *remove = new QPushButton(tr("Remove"), ex);
    add->setAutoDefault(false);
    remove->setAutoDefault(false);
    remove->setEnabled(false);
    buttons->addWidget(add);
    buttons->addWidget(remove);
    buttons->addStretch(1);
    row->addLayout(buttons);
    el->addLayout(row);
    layout->addWidget(ex, 1);
    connect(m_excluded, &QListWidget::itemSelectionChanged, remove, [this, remove] { remove->setEnabled(!m_excluded->selectedItems().isEmpty()); });
    connect(remove, &QPushButton::clicked, this, [this] { qDeleteAll(m_excluded->selectedItems()); });
    connect(add, &QPushButton::clicked, this, [this] {
        const QString dir = QFileDialog::getExistingDirectory(this, tr("Exclude Folder"));
        if (dir.isEmpty())
            return;
        const QString native = QDir::toNativeSeparators(dir);
        if (m_excluded->findItems(native, Qt::MatchFixedString).isEmpty())
            m_excluded->addItem(native);
    });
    return page;
}

QWidget *OptionsDialog::buildKeyboardPage()
{
    auto *page = new QWidget;
    auto *layout = new QVBoxLayout(page);
    layout->setContentsMargins(0, 6, 0, 0);

    // Global hotkeys: show, new and toggle.
    auto *global = new QGroupBox(tr("Global hotkeys"), page);
    auto *gform = new QFormLayout(global);
    const bool canSet = GlobalHotkey::available();
    const QString titles[] = {tr("Show window hotkey:"), tr("New window hotkey:"), tr("Toggle window hotkey:")};
    const QString tips[] = {tr("Brings the Trawlr window to the front, or opens one if none is open"),
                            tr("Always opens another Trawlr window"),
                            tr("Minimizes the Trawlr window if it is the active window, otherwise brings it to the front")};
    const char *const names[] = {"globalShow", "globalNew", "globalToggle"};
    bool needsInstall = false;
    for (int a = 0; a < GlobalHotkey::ActionCount; ++a) {
        auto *e = keyEdit(global);
        e->setObjectName(QLatin1String(names[a]));
        e->setToolTip(tips[a]);
        const bool supported = canSet && GlobalHotkey::supports(GlobalHotkey::Action(a));
        if (supported) {
            m_globalAtStart[a] = GlobalHotkey::current(GlobalHotkey::Action(a));
            e->setKeySequence(m_globalAtStart[a]);
        } else {
            e->setEnabled(false);
            needsInstall = needsInstall || canSet;
        }
        m_global[a] = e;
        e->installEventFilter(this);
        // One key, one hotkey: it leaves whichever other box had it.
        connect(e, &QKeySequenceEdit::keySequenceChanged, this, [this, a](const QKeySequence &k) {
            if (k.isEmpty())
                return;
            for (int b = 0; b < GlobalHotkey::ActionCount; ++b)
                if (b != a && m_global[b]->isEnabled() && m_global[b]->keySequence() == k)
                    m_global[b]->clear();
        });
        auto *label = new QLabel(titles[a], global);
        label->setToolTip(tips[a]);
        label->setBuddy(e);
        gform->addRow(label, e);
    }
    QString note;
    if (!canSet)
        note = tr("This desktop does not let programs set a global hotkey. Bind keys in your desktop's keyboard settings to the commands "
                  "<b>trawlr</b>, <b>trawlr --new-window</b> and <b>trawlr --toggle</b>.");
#if defined(Q_OS_WIN) || defined(Q_OS_MACOS)
    else
        note = tr("These work from any program while Trawlr is running, including from the tray with no window open.");
#else
    else
        note = tr("These work from any program, even when no Trawlr window is open.");
#endif
    if (needsInstall)
        note += QLatin1Char(' ') + tr("The greyed-out ones need the newer launcher entry: run <b>./install.sh</b> again.");
    gform->addRow(hintLabel(note, global));
    layout->addWidget(global);

    // In-window shortcuts: a command list, and the keys of the selected command.
    auto *keys = new QGroupBox(tr("Shortcuts in the window"), page);
    auto *kl = new QVBoxLayout(keys);
    auto *filterRow = new QHBoxLayout;
    auto *filterLabel = new QLabel(tr("Show commands containing:"), keys);
    filterRow->addWidget(filterLabel);
    m_filter = new QLineEdit(keys);
    m_filter->setObjectName("commandFilter");
    m_filter->setClearButtonEnabled(true);
    filterLabel->setBuddy(m_filter);
    filterRow->addWidget(m_filter, 1);
    kl->addLayout(filterRow);

    m_list = new QTreeWidget(keys);
    m_list->setObjectName("commandList");
    m_list->setColumnCount(2);
    m_list->setHeaderLabels({tr("Command"), tr("Shortcuts")});
    m_list->setRootIsDecorated(false);
    m_list->setUniformRowHeights(true);
    m_list->setMinimumHeight(m_list->fontMetrics().height() * 9);
    m_list->header()->setStretchLastSection(false);
    m_list->header()->setSectionResizeMode(0, QHeaderView::Stretch);
    m_list->header()->setSectionResizeMode(1, QHeaderView::ResizeToContents);
    for (const CommandInfo &c : std::as_const(m_commands)) {
        auto *item = new QTreeWidgetItem(m_list, {c.label});
        item->setData(0, Qt::UserRole, c.id);
        showRow(c.id);
    }
    kl->addWidget(m_list, 1);

    kl->addWidget(new QLabel(tr("Shortcuts for the selected command:"), keys));
    auto *selRow = new QHBoxLayout;
    m_keyList = new QListWidget(keys);
    m_keyList->setObjectName("keyList");
    // As tall as the buttons beside it, no taller.
    m_keyList->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Ignored);
    selRow->addWidget(m_keyList, 1);
    auto *selButtons = new QVBoxLayout;
    m_add = new QPushButton(tr("Add…"), keys);
    m_edit = new QPushButton(tr("Edit…"), keys);
    m_remove = new QPushButton(tr("Remove"), keys);
    m_default = new QPushButton(tr("Default"), keys);
    m_default->setToolTip(tr("Put back this command's original shortcut"));
    for (QPushButton *b : {m_add, m_edit, m_remove, m_default}) {
        b->setAutoDefault(false);
        selButtons->addWidget(b);
    }
    selRow->addLayout(selButtons);
    kl->addLayout(selRow);
    kl->addWidget(hintLabel(tr("Enter, Esc and the arrow keys have fixed meanings. Changed shortcuts are shown in bold."), keys));
    layout->addWidget(keys, 1);

    connect(m_filter, &QLineEdit::textChanged, this, &OptionsDialog::filterCommands);
    connect(m_list, &QTreeWidget::currentItemChanged, this, &OptionsDialog::commandChanged);
    connect(m_keyList, &QListWidget::currentRowChanged, this, &OptionsDialog::commandChanged);
    connect(m_keyList, &QListWidget::itemDoubleClicked, m_edit, &QPushButton::click);
    connect(m_list, &QTreeWidget::itemDoubleClicked, m_add, &QPushButton::click);
    connect(m_add, &QPushButton::clicked, this, [this] {
        const QString id = currentCommand();
        if (id.isEmpty())
            return;
        const QKeySequence k = askForKey(id, QKeySequence());
        if (!k.isEmpty())
            addShortcut(id, k);
    });
    connect(m_edit, &QPushButton::clicked, this, [this] {
        const QString id = currentCommand();
        QListWidgetItem *it = m_keyList->currentItem();
        if (id.isEmpty() || !it)
            return;
        const QKeySequence old = it->data(Qt::UserRole).value<QKeySequence>();
        const QKeySequence k = askForKey(id, old);
        if (k.isEmpty() || k == old)
            return;
        removeShortcut(id, old);
        addShortcut(id, k);
    });
    connect(m_remove, &QPushButton::clicked, this, [this] {
        const QString id = currentCommand();
        if (QListWidgetItem *it = m_keyList->currentItem())
            removeShortcut(id, it->data(Qt::UserRole).value<QKeySequence>());
    });
    connect(m_default, &QPushButton::clicked, this, [this] {
        const QString id = currentCommand();
        const CommandInfo *c = command(id);
        if (!c)
            return;
        m_keys[id].clear();
        if (!c->defaultKey.isEmpty())
            addShortcut(id, c->defaultKey);
        showRow(id);
        commandChanged();
    });
    commandChanged();
    return page;
}

// ---- in-window shortcuts ----

const CommandInfo *OptionsDialog::command(const QString &id) const
{
    for (const CommandInfo &c : m_commands)
        if (c.id == id)
            return &c;
    return nullptr;
}

QString OptionsDialog::currentCommand() const
{
    QTreeWidgetItem *it = m_list->currentItem();
    return it && !it->isHidden() ? it->data(0, Qt::UserRole).toString() : QString();
}

QTreeWidgetItem *OptionsDialog::itemFor(const QString &id) const
{
    for (int i = 0; i < m_list->topLevelItemCount(); ++i)
        if (m_list->topLevelItem(i)->data(0, Qt::UserRole).toString() == id)
            return m_list->topLevelItem(i);
    return nullptr;
}

void OptionsDialog::showRow(const QString &id)
{
    QTreeWidgetItem *it = itemFor(id);
    const CommandInfo *c = command(id);
    if (!it || !c)
        return;
    it->setText(1, Shortcuts::text(m_keys.value(id)));
    QFont f = it->font(1);
    f.setBold(m_keys.value(id) != Shortcuts::defaults(c->defaultKey)); // changed ones stand out
    it->setFont(1, f);
}

void OptionsDialog::commandChanged()
{
    const QString id = currentCommand();
    const int row = m_keyList->currentRow();
    {
        const QSignalBlocker block(m_keyList);
        m_keyList->clear();
        for (const QKeySequence &k : m_keys.value(id)) {
            auto *item = new QListWidgetItem(k.toString(QKeySequence::NativeText), m_keyList);
            item->setData(Qt::UserRole, QVariant::fromValue(k));
        }
        if (m_keyList->count())
            m_keyList->setCurrentRow(qBound(0, row, m_keyList->count() - 1));
    }
    const CommandInfo *c = command(id);
    m_add->setEnabled(c != nullptr);
    m_edit->setEnabled(m_keyList->currentItem() != nullptr);
    m_remove->setEnabled(m_keyList->currentItem() != nullptr);
    m_default->setEnabled(c && m_keys.value(id) != Shortcuts::defaults(c->defaultKey));
}

void OptionsDialog::filterCommands()
{
    const QString text = m_filter->text();
    for (int i = 0; i < m_list->topLevelItemCount(); ++i) {
        QTreeWidgetItem *it = m_list->topLevelItem(i);
        const bool byFilter = text.isEmpty() || it->text(0).contains(text, Qt::CaseInsensitive) || it->text(1).contains(text, Qt::CaseInsensitive);
        const bool bySearch = m_searchCommands.isEmpty() || m_searchCommands.contains(it->data(0, Qt::UserRole).toString());
        it->setHidden(!(byFilter && bySearch));
    }
    if (m_list->currentItem() && m_list->currentItem()->isHidden())
        m_list->setCurrentItem(nullptr);
    commandChanged();
}

QString OptionsDialog::usedBy(const QKeySequence &key, const QString &exceptId) const
{
    for (const CommandInfo &c : m_commands)
        if (c.id != exceptId && m_keys.value(c.id).contains(key))
            return c.label;
    return QString();
}

void OptionsDialog::addShortcut(const QString &id, const QKeySequence &key)
{
    if (key.isEmpty() || !command(id))
        return;
    // One key, one command: it leaves whichever command had it.
    for (const CommandInfo &c : std::as_const(m_commands)) {
        if (c.id != id && m_keys[c.id].removeAll(key))
            showRow(c.id);
    }
    if (!m_keys[id].contains(key))
        m_keys[id].append(key);
    showRow(id);
    commandChanged();
    for (int i = 0; i < m_keyList->count(); ++i)
        if (m_keyList->item(i)->data(Qt::UserRole).value<QKeySequence>() == key)
            m_keyList->setCurrentRow(i);
}

void OptionsDialog::removeShortcut(const QString &id, const QKeySequence &key)
{
    if (m_keys[id].removeAll(key)) {
        showRow(id);
        commandChanged();
    }
}

// "Add Keyboard Shortcut": press the keys, see what has them now.
QKeySequence OptionsDialog::askForKey(const QString &forId, const QKeySequence &start)
{
    const CommandInfo *c = command(forId);
    QDialog d(this);
    d.setWindowTitle(start.isEmpty() ? tr("Add Shortcut") : tr("Edit Shortcut"));
    auto *layout = new QVBoxLayout(&d);
    auto *what = new QLabel(tr("Press the new shortcut for <b>%1</b>:").arg(c ? c->label.toHtmlEscaped() : QString()), &d);
    auto *edit = keyEdit(&d);
    edit->setObjectName("captureEdit");
    edit->installEventFilter(this);
    edit->setKeySequence(start);
    auto *used = hintLabel(QString(), &d);
    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &d);
    layout->addWidget(what);
    layout->addWidget(edit);
    layout->addWidget(used);
    layout->addWidget(buttons);
    // A recorder takes Enter as a key to record, so OK is clicked, not pressed.
    for (QAbstractButton *b : buttons->buttons())
        if (auto *pb = qobject_cast<QPushButton *>(b))
            pb->setAutoDefault(false);
    connect(buttons, &QDialogButtonBox::accepted, &d, &QDialog::accept);
    connect(buttons, &QDialogButtonBox::rejected, &d, &QDialog::reject);
    auto update = [&] {
        const QKeySequence k = edit->keySequence();
        const QString other = k.isEmpty() ? QString() : usedBy(k, forId);
        used->setText(other.isEmpty() ? tr("Currently used by: none") : tr("Currently used by: %1 (it will lose this shortcut)").arg(other));
        buttons->button(QDialogButtonBox::Ok)->setEnabled(!k.isEmpty());
    };
    connect(edit, &QKeySequenceEdit::keySequenceChanged, &d, update);
    update();
    edit->setFocus();
    d.resize(qMax(d.sizeHint().width(), 380), d.sizeHint().height());
    return d.exec() == QDialog::Accepted ? edit->keySequence() : QKeySequence();
}

// ---- settings search ----

void OptionsDialog::indexPage(int page)
{
    QWidget *root = m_pages[page].widget;
    for (QWidget *w : root->findChildren<QWidget *>()) {
        QString text;
        if (auto *l = qobject_cast<QLabel *>(w)) {
            text = plain(l->text());
            if (auto *combo = qobject_cast<QComboBox *>(l->buddy()))
                for (int i = 0; i < combo->count(); ++i)
                    text += QLatin1Char(' ') + combo->itemText(i);
        } else if (auto *b = qobject_cast<QCheckBox *>(w)) {
            text = plain(b->text());
        } else if (auto *r = qobject_cast<QRadioButton *>(w)) {
            text = plain(r->text());
        } else if (auto *g = qobject_cast<QGroupBox *>(w)) {
            text = plain(g->title());
        }
        if (!w->toolTip().isEmpty())
            text += QLatin1Char(' ') + w->toolTip();
        if (!text.trimmed().isEmpty())
            m_entries.append({page, w, text, QString()});
    }
    if (m_list && root->isAncestorOf(m_list)) {
        for (const CommandInfo &c : std::as_const(m_commands))
            m_entries.append({page, nullptr, c.label, c.id});
    }
}

void OptionsDialog::setHighlighted(QWidget *w, bool on)
{
    if (!w)
        return;
    if (!on) {
        w->setStyleSheet(QString());
        return;
    }
    QColor c = palette().color(QPalette::Highlight);
    c.setAlpha(70);
    const QString bg = QStringLiteral("rgba(%1,%2,%3,%4)").arg(c.red()).arg(c.green()).arg(c.blue()).arg(c.alpha());
    w->setStyleSheet(QStringLiteral("background-color: %1; border-radius: 3px;").arg(bg));
    m_highlighted.append(w);
}

void OptionsDialog::runSettingsSearch()
{
    for (const QPointer<QWidget> &w : std::as_const(m_highlighted))
        setHighlighted(w, false);
    m_highlighted.clear();
    m_searchCommands.clear();

    const QStringList words = m_settingsSearch->text().split(QLatin1Char(' '), Qt::SkipEmptyParts);
    auto has = [](const QString &hay, const QString &w) { return hay.contains(w, Qt::CaseInsensitive); };

    QList<bool> pageHit(m_pages.size(), words.isEmpty());
    QList<bool> pageHasWidgetHit(m_pages.size(), false);
    if (!words.isEmpty()) {
        for (int p = 0; p < m_pages.size(); ++p) {
            const QString pageText = m_pages[p].title + QLatin1Char(' ') + m_pages[p].keywords;
            pageHit[p] = std::all_of(words.begin(), words.end(), [&](const QString &w) { return has(pageText, w); });
        }
        // A setting matches when it has at least one of the words itself and
        // every word is in it or its page ("keyboard copy" finds File > Copy).
        for (const Entry &e : std::as_const(m_entries)) {
            const QString pageText = m_pages[e.page].title + QLatin1Char(' ') + m_pages[e.page].keywords;
            const bool own = std::any_of(words.begin(), words.end(), [&](const QString &w) { return has(e.text, w); });
            const bool all = std::all_of(words.begin(), words.end(), [&](const QString &w) { return has(e.text, w) || has(pageText, w); });
            if (!own || !all)
                continue;
            pageHit[e.page] = true;
            pageHasWidgetHit[e.page] = true;
            // A group box styled by a style sheet loses its native frame, so
            // a matching group only counts for its page.
            if (e.widget && !qobject_cast<QGroupBox *>(e.widget.data()))
                setHighlighted(e.widget, true);
            else
                m_searchCommands.append(e.commandId);
        }
    }

    for (int p = 0; p < m_pages.size(); ++p)
        m_pages[p].item->setHidden(!pageHit[p]);
    const bool any = std::any_of(pageHit.begin(), pageHit.end(), [](bool b) { return b; });
    m_noMatch->setVisible(!any);
    m_stack->setEnabled(any);
    filterCommands();

    // Stay on the current page if it has something; else go to the first that does.
    const int cur = m_stack->currentIndex();
    if (cur >= 0 && pageHit[cur] && (pageHasWidgetHit[cur] || !std::any_of(pageHasWidgetHit.begin(), pageHasWidgetHit.end(), [](bool b) { return b; })))
        return;
    for (int p = 0; p < m_pages.size(); ++p) {
        if (pageHit[p] && (pageHasWidgetHit[p] || !std::any_of(pageHasWidgetHit.begin(), pageHasWidgetHit.end(), [](bool b) { return b; }))) {
            m_pageList->setCurrentItem(m_pages[p].item);
            return;
        }
    }
}

OptionsDialog::~OptionsDialog()
{
    blockGlobalShortcuts(false);
}

void OptionsDialog::blockGlobalShortcuts(bool on)
{
    if (on == m_blocking)
        return;
    m_blocking = on;
    GlobalHotkey::block(on);
}

bool OptionsDialog::eventFilter(QObject *o, QEvent *e)
{
    // A key that is bound globally (Ctrl+Shift+F for Show window, say) never
    // reaches a window while the desktop holds it, even after it is cleared
    // here, because that only happens on Apply. So while a recorder has the
    // focus, the desktop's global shortcuts are paused.
    if (qobject_cast<QKeySequenceEdit *>(o)) {
        if (e->type() == QEvent::FocusIn)
            blockGlobalShortcuts(true);
        else if (e->type() == QEvent::FocusOut || e->type() == QEvent::Hide)
            blockGlobalShortcuts(false);
    }
    if (o == m_settingsSearch && e->type() == QEvent::KeyPress) {
        auto *ke = static_cast<QKeyEvent *>(e);
        switch (ke->key()) {
        case Qt::Key_Escape:
            if (!m_settingsSearch->text().isEmpty()) { // the first Esc clears, the next closes
                m_settingsSearch->clear();
                return true;
            }
            break;
        case Qt::Key_Down:
            m_pageList->setFocus();
            return true;
        case Qt::Key_Return:
        case Qt::Key_Enter: {
            // Go to the first matching setting rather than pressing OK.
            QWidget *page = m_stack->currentWidget();
            for (const QPointer<QWidget> &w : std::as_const(m_highlighted)) {
                if (w && page && page->isAncestorOf(w)) {
                    QWidget *target = w;
                    if (auto *l = qobject_cast<QLabel *>(w); l && l->buddy())
                        target = l->buddy();
                    if (target->focusPolicy() != Qt::NoFocus) {
                        target->setFocus(Qt::TabFocusReason);
                        return true;
                    }
                }
            }
            if (!m_searchCommands.isEmpty() && page && page->isAncestorOf(m_list)) {
                m_list->setFocus();
                if (!m_list->currentItem())
                    for (int i = 0; i < m_list->topLevelItemCount(); ++i)
                        if (!m_list->topLevelItem(i)->isHidden()) {
                            m_list->setCurrentItem(m_list->topLevelItem(i));
                            break;
                        }
                return true;
            }
            m_pageList->setFocus();
            return true;
        }
        default:
            break;
        }
    }
    return QDialog::eventFilter(o, e);
}

// ---- saving ----

bool OptionsDialog::applyGlobalHotkeys()
{
    QList<int> changed;
    for (int a = 0; a < GlobalHotkey::ActionCount; ++a)
        if (m_global[a]->isEnabled() && m_global[a]->keySequence() != m_globalAtStart[a])
            changed << a;
    if (changed.isEmpty())
        return true;
    for (int a = 0; a < GlobalHotkey::ActionCount; ++a) {
        for (int b = a + 1; b < GlobalHotkey::ActionCount; ++b) {
            const QKeySequence k = m_global[a]->keySequence();
            if (!k.isEmpty() && k == m_global[b]->keySequence()) {
                QMessageBox::warning(this, tr("Global hotkeys"), tr("%1 is set for two global hotkeys. Give each its own key.").arg(k.toString(QKeySequence::NativeText)));
                showPage(m_pages.last().title);
                return false;
            }
        }
    }
    // Clear first, so a key can move from one of them to another.
    QString why;
    for (int a : std::as_const(changed))
        if (!m_globalAtStart[a].isEmpty())
            GlobalHotkey::set(GlobalHotkey::Action(a), QKeySequence(), nullptr);
    for (int a : std::as_const(changed)) {
        const QKeySequence k = m_global[a]->keySequence();
        if (!k.isEmpty() && !GlobalHotkey::set(GlobalHotkey::Action(a), k, &why)) {
            for (int b : std::as_const(changed)) // put back what was there
                GlobalHotkey::set(GlobalHotkey::Action(b), m_globalAtStart[b], nullptr);
            QMessageBox::warning(this, tr("Global hotkeys"), why);
            showPage(m_pages.last().title);
            m_global[a]->setFocus();
            return false;
        }
    }
    for (int a : std::as_const(changed))
        m_globalAtStart[a] = m_global[a]->keySequence();
    return true;
}

bool OptionsDialog::apply()
{
    if (!applyGlobalHotkeys())
        return false; // stay open so it can be changed
    for (const CommandInfo &c : std::as_const(m_commands))
        Shortcuts::save(c.id, m_keys.value(c.id), c.defaultKey);
    QSettings s;
    s.setValue(kMultiple, m_multiple->isChecked());
    s.setValue(kTray, m_tray->isChecked());
    if (m_autostart && m_autostart->isChecked() != Platform::startsAtLogin())
        Platform::setStartAtLogin(m_autostart->isChecked());
    s.setValue(QStringLiteral("general/pathDoubleClick"), m_pathClick->currentData().toString());
    s.setValue(QStringLiteral("terminal/program"), m_terminal->currentData().toString());
    s.setValue(QStringLiteral("terminal/custom"), m_terminalCustom->text().trimmed());
    s.setValue(kTheme, m_theme->currentData().toString());
    s.setValue(QStringLiteral("view/accent"), m_accent->currentData().toString());
    s.setValue(QStringLiteral("view/density"), m_density->currentData().toString());
    if (m_menuBar)
        s.setValue(QStringLiteral("view/menuBar"), m_menuBar->isChecked());
    s.setValue(QStringLiteral("view/autoFitColumns"), m_autoFit->isChecked());
    QCheckBox *boxes[] = {m_matchCase, m_wholeWord, m_matchPath, m_regex};
    for (int i = 0; i < 4; ++i)
        s.setValue(QLatin1String(kSearchKeys[i]), boxes[i]->isChecked());
    s.setValue(kClearOnShow, m_clearSearch->isChecked());
    QStringList excluded;
    for (int i = 0; i < m_excluded->count(); ++i)
        excluded << m_excluded->item(i)->text();
    setExcludedFolders(excluded);
    emit applied();
    return true;
}

void OptionsDialog::accept()
{
    if (apply())
        QDialog::accept();
}

void OptionsDialog::done(int r)
{
    blockGlobalShortcuts(false);
    QSettings s;
    s.setValue(kGeometry, saveGeometry());
    s.setValue(kLastPage, currentPage());
    QDialog::done(r);
}
