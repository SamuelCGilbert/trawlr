// The pieces of the window around the result list: the search field, the
// thin progress line under it, and the index state shown in the footer.
#pragma once
#include <QToolButton>
#include <QWidget>
#include <QElapsedTimer>
#include <QTimer>

class QAction;
class QHBoxLayout;
class QLineEdit;

// A large, rounded search field with an icon, toggle buttons for the search
// switches, and a clear button. The line edit inside is a plain QLineEdit.
class SearchBox : public QWidget {
    Q_OBJECT
public:
    explicit SearchBox(QWidget *parent = nullptr);
    QLineEdit *edit() const { return m_edit; }
    // A button that shows and flips a checkable action.
    void addToggle(QAction *action, int glyph, const QString &tip);

protected:
    void paintEvent(QPaintEvent *e) override;
    bool eventFilter(QObject *o, QEvent *e) override;
    void mousePressEvent(QMouseEvent *e) override;

private:
    QLineEdit *m_edit;
    QToolButton *m_clear;
    QHBoxLayout *m_toggles;
};

// A 2-pixel line that sweeps while a search is running. It only appears if
// the search takes long enough to notice, so fast searches never flash it.
class BusyBar : public QWidget {
    Q_OBJECT
public:
    explicit BusyBar(QWidget *parent = nullptr);
    void setBusy(bool busy);

protected:
    void paintEvent(QPaintEvent *e) override;

private:
    QTimer m_delay;
    QTimer m_frame;
    QElapsedTimer m_clock;
    bool m_busy = false;
    bool m_shown = false;
};

// "● Live" / "● Indexing… 1,234" / "● Offline" in the footer. Clickable.
class StatusPill : public QWidget {
    Q_OBJECT
public:
    enum State { Live, Working, Offline };
    explicit StatusPill(QWidget *parent = nullptr);
    void setState(State s, const QString &text, const QString &tip);
    QSize sizeHint() const override;

signals:
    void clicked();

protected:
    void paintEvent(QPaintEvent *e) override;
    void mouseReleaseEvent(QMouseEvent *e) override;
    void enterEvent(QEnterEvent *e) override;
    void leaveEvent(QEvent *e) override;

private:
    State m_state = Offline;
    QString m_text;
    bool m_hover = false;
    QTimer m_pulse;
    QElapsedTimer m_clock;
};
