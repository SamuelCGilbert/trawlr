#pragma once
#include "client.h"
#include "instance.h"
#include "shortcuts.h"

#include <QJsonObject>
#include <QMainWindow>
#include <QTimer>
#include <functional>

class QAction;
class QActionGroup;
class QLabel;
class QMenu;
class QLineEdit;
class QHBoxLayout;
class QToolButton;
class BusyBar;
class ResultStore;
class ResultsView;
class SearchBox;
class StatusPill;

class MainWindow : public QMainWindow {
    Q_OBJECT
public:
    explicit MainWindow(const QString &initialSearch, QWidget *parent = nullptr);

    // A new window that frees itself when closed.
    static MainWindow *openNew(const QString &search);
    // What to do when the program is started again while it is running: bring
    // the window you used last to the front, or open another if the user
    // allows multiple windows (or asked for one).
    static void handleLaunch(const LaunchRequest &r);
    // Re-read the settings (shortcuts, theme menu, and with `searchSwitches`
    // Match Case and the like) in every open window.
    static void applySettingsToAll(bool searchSwitches = true);
    static void applyShortcutsToAll() { applySettingsToAll(false); }
    // "system", "light" or "dark", for the whole program.
    static void applyTheme(const QString &key);

    // The notification-area icon, shared by every window; shown or removed
    // according to the setting.
    static void updateTrayIcon();

    void bringToFront(const QString &activationToken = QString());
    // Out of the way, as the toggle hotkey does: off the
    // screen, the taskbar and Alt+Tab, but still running (the tray icon,
    // hotkey or launcher bring it back).
    void hideAway(const QString &activationToken = QString());

protected:
    void closeEvent(QCloseEvent *e) override;
    bool eventFilter(QObject *o, QEvent *e) override;
    bool event(QEvent *e) override;

private:
    static void toggleLastWindow();
    // Activates the window using the desktop's launch token, once the window
    // is on screen. Using the token is what ends KDE's bouncing launch icon.
    void activateWithToken(const QString &token);

    struct Command {
        QString id;
        QAction *action;
        Shortcuts::Keys defaultKeys;
        QString label;
    };

    void buildMenus();
    void applyShortcuts();
    void applySettings(bool searchSwitches);
    QList<CommandInfo> commands() const;
    void showOptions(const QString &page = QString());
    void runSearch();
    void updateStatusBar();
    void updateIndexPill();
    void buildChips();
    void setSort(const QString &key, bool desc);
    void withSelection(int max, std::function<void(const QList<Row> &)> fn);

    // With `thenHide`, the window goes out of the way once the item is open.
    void openSelected(bool thenHide = false);
    void openContainingFolder(bool thenHide = false);
    void activate(int column, Qt::KeyboardModifiers mods);
    void openAndHide();
    void dismissAfterOpening();
    void openInTerminal();
    void runInTerminal();
    void copyFiles();
    void copyPaths(bool namesOnly);
    void trashSelected();
    void renameSelected();
    void renameItem(const Row &row, const QString &newName);
    void showProperties();
    void startDrag();
    QWidget *buildPrivacyNotice(QWidget *parent);
    static void updateDockIcon();
    void showContextMenu(const QPoint &globalPos);
    void addExcludeMenu(QMenu *menu);
    void showSyntaxHelp();
    void showIndexStatus(const QJsonObject &s);

    Client *m_client;
    ResultStore *m_store;
    ResultsView *m_view;
    SearchBox *m_searchBox;
    QLineEdit *m_search;
    QToolButton *m_menuButton;
    QHBoxLayout *m_chipRow;
    BusyBar *m_busy;
    QLabel *m_countLabel;
    QLabel *m_timeLabel;
    StatusPill *m_pill;
    QWidget *m_notice = nullptr;
    bool m_noticeDismissed = false;
    QAction *m_showMenuBar = nullptr;
    QAction *m_autoFitColumns = nullptr;

    QAction *m_matchCase, *m_wholeWord, *m_matchPath, *m_regex;
    QActionGroup *m_filterGroup, *m_sortGroup, *m_themeGroup;
    QAction *m_sortDesc;
    QList<QAction *> m_selectionActions;
    QList<Command> m_commands;
    QAction *m_trash = nullptr;

    QString m_sortKey = QStringLiteral("name");
    bool m_desc = false;
    QTimer m_refreshTimer;
    QTimer m_statusTimer;
    QString m_indexState;
    qint64 m_scanned = 0;
    QString m_refusal;
    bool m_wantStatusDialog = false;
};
