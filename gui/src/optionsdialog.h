#pragma once
#include "globalhotkey.h"
#include "shortcuts.h"

#include <QDialog>
#include <QHash>
#include <QList>
#include <QPointer>
#include <functional>

class QCheckBox;
class QComboBox;
class QKeySequenceEdit;
class QLabel;
class QLineEdit;
class QListWidget;
class QPushButton;
class QRadioButton;
class QStackedWidget;
class QTreeWidget;
class QTreeWidgetItem;

// Tools > Options: a list of pages on the left,
// the page on the right, and OK / Cancel / Apply. Nothing is saved until OK or
// Apply. The box above the page list searches every setting ("/" goes there).
class OptionsDialog : public QDialog {
    Q_OBJECT
public:
    explicit OptionsDialog(const QList<CommandInfo> &commands, QWidget *parent = nullptr);
    ~OptionsDialog() override;

    // Saves everything. False (and the dialog stays as it is) if something
    // could not be saved, after saying why.
    bool apply();
    void accept() override;

    // Shows the page with this title ("General", "Keyboard", ...).
    void showPage(const QString &title);
    QString currentPage() const;

    // What the Add/Edit buttons do once the key is known. A key another
    // command has is taken away from it.
    void addShortcut(const QString &id, const QKeySequence &key);
    void removeShortcut(const QString &id, const QKeySequence &key);
    void restorePageDefaults();

    // Saved settings the rest of the program reads.
    static bool allowMultipleWindows();
    static bool clearSearchOnShow();
    static bool showTrayIcon();
    // Double-clicking in the Path column opens the containing folder (else the item).
    static bool pathDoubleClickOpensFolder();
    static QString theme(); // "system", "light" or "dark"
    // Result columns size themselves to their contents; Name takes the rest.
    static bool autoFitColumns();
    // Folders left out of every search's results (with what is in them).
    static QStringList excludedFolders();
    static void setExcludedFolders(const QStringList &folders);

signals:
    void applied();

protected:
    bool eventFilter(QObject *o, QEvent *e) override;
    void done(int r) override;

private:
    struct Page {
        QString title;
        QString keywords; // other words people may search for
        QWidget *widget = nullptr;
        QTreeWidgetItem *item = nullptr;
        std::function<void()> restoreDefaults;
    };
    struct Entry { // one searchable thing on a page
        int page;
        QPointer<QWidget> widget; // null for a command in the keyboard list
        QString text;
        QString commandId;
    };

    void addPage(const QString &title, const QString &keywords, QWidget *w, std::function<void()> restore);
    QWidget *buildGeneralPage();
    QWidget *buildViewPage();
    QWidget *buildSearchPage();
    QWidget *buildKeyboardPage();
    QWidget *buildTerminalPage();
    void indexPage(int page);
    void runSettingsSearch();
    void setHighlighted(QWidget *w, bool on);

    bool applyGlobalHotkeys();
    void blockGlobalShortcuts(bool on);
    bool m_blocking = false;
    const CommandInfo *command(const QString &id) const;
    QString currentCommand() const;
    QTreeWidgetItem *itemFor(const QString &id) const;
    void showRow(const QString &id);
    void filterCommands();
    void commandChanged();
    QKeySequence askForKey(const QString &forId, const QKeySequence &start);
    QString usedBy(const QKeySequence &key, const QString &exceptId) const;

    QList<CommandInfo> m_commands;
    QHash<QString, Shortcuts::Keys> m_keys; // pending values, by command id
    QList<Page> m_pages;
    QList<Entry> m_entries;
    QList<QPointer<QWidget>> m_highlighted;

    QLineEdit *m_settingsSearch = nullptr;
    QTreeWidget *m_pageList = nullptr;
    QLabel *m_noMatch = nullptr;
    QLabel *m_pageTitle = nullptr;
    QStackedWidget *m_stack = nullptr;

    QCheckBox *m_multiple = nullptr;
    QCheckBox *m_tray = nullptr;
    QCheckBox *m_autostart = nullptr;
    QComboBox *m_pathClick = nullptr;
    QComboBox *m_terminal = nullptr;
    QLineEdit *m_terminalCustom = nullptr;
    QComboBox *m_theme = nullptr;
    QComboBox *m_accent = nullptr;
    QComboBox *m_density = nullptr;
    QCheckBox *m_menuBar = nullptr;
    QCheckBox *m_autoFit = nullptr;
    QCheckBox *m_matchCase = nullptr, *m_wholeWord = nullptr, *m_matchPath = nullptr, *m_regex = nullptr;
    QRadioButton *m_keepSearch = nullptr, *m_clearSearch = nullptr;
    QListWidget *m_excluded = nullptr;

    QKeySequenceEdit *m_global[GlobalHotkey::ActionCount] = {};
    QKeySequence m_globalAtStart[GlobalHotkey::ActionCount];
    QLineEdit *m_filter = nullptr;
    QTreeWidget *m_list = nullptr;
    QListWidget *m_keyList = nullptr;
    QPushButton *m_add = nullptr, *m_edit = nullptr, *m_remove = nullptr, *m_default = nullptr;
    QStringList m_searchCommands; // commands the settings search matched, if any
};
