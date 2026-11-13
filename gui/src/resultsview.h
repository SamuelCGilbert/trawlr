// The result list.
//
// Painted by hand rather than through an item view: Qt's views keep per-row
// bookkeeping, which is fine for thousands of rows and not for the tens of
// millions an empty search returns. Here only the rows on screen exist.
#pragma once
#include "rangeset.h"
#include "resultstore.h"

#include <QAbstractScrollArea>
#include <QHash>
#include <QIcon>
#include <QMimeDatabase>

class QHeaderView;

class ResultsView : public QAbstractScrollArea {
    Q_OBJECT
public:
    enum Column { ColName, ColPath, ColSize, ColDate, ColCount };

    explicit ResultsView(ResultStore *store, QWidget *parent = nullptr);

    QHeaderView *header() const { return m_header; }
    const RangeSet &selection() const { return m_sel; }
    qint64 currentRow() const { return m_current; }
    int rowHeight() const { return m_rowH; }
    void selectAll();
    void selectOnly(qint64 row);
    // Plain words of the search, shown bold where they occur in a name.
    void setHighlightTerms(const QStringList &terms);
    // Selected rows that are already loaded, for actions that cannot wait.
    QList<Row> loadedSelection(int max) const;
    // What to show in place of the list when there are no rows.
    void setEmptyState(int glyph, const QString &title, const QString &body);
    // Auto-fit: Size, Modified and Path are kept as wide as their contents and
    // Name takes the rest. Off: columns are dragged by hand and Path stretches.
    void setAutoFitColumns(bool on);
    bool autoFitColumns() const { return m_autoFit; }
    // Sizes one column to the widest of its cells on screen (and its title).
    void fitColumn(int col);

signals:
    // Double-click (column under the pointer) or Return (column -1), with the modifiers held.
    void activated(int column, Qt::KeyboardModifiers modifiers);
    void selectionChanged();
    void contextMenuRequested(const QPoint &globalPos);
    void sortRequested(int column);
    void dragRequested();
    void textTyped(const QString &text);

protected:
    void paintEvent(QPaintEvent *e) override;
    void resizeEvent(QResizeEvent *e) override;
    void scrollContentsBy(int dx, int dy) override;
    void mousePressEvent(QMouseEvent *e) override;
    void mouseMoveEvent(QMouseEvent *e) override;
    void mouseReleaseEvent(QMouseEvent *e) override;
    void mouseDoubleClickEvent(QMouseEvent *e) override;
    void keyPressEvent(QKeyEvent *e) override;
    void contextMenuEvent(QContextMenuEvent *e) override;
    void focusInEvent(QFocusEvent *e) override;
    void focusOutEvent(QFocusEvent *e) override;
    bool viewportEvent(QEvent *e) override;
    void changeEvent(QEvent *e) override;

private:
    void updateGeometries();
    void updateMetrics();
    qint64 rowAt(int y) const;
    int visibleRows() const;
    void ensureVisible(qint64 row);
    void moveCurrent(qint64 row, Qt::KeyboardModifiers mods);
    void paintRow(QPainter &p, qint64 r, const QRect &rect);
    void drawCellText(QPainter &p, const QRect &rect, const QString &text, int align, Qt::TextElideMode elide, bool highlight);
    QIcon iconFor(const Row &row);
    void paintEmptyState(QPainter &p);
    QString displayDir(const QString &dir) const;
    QString displayDate(qint64 secs) const;
    int contentWidth(int col) const;
    int headerWidth(int col) const;
    void scheduleAutoFit();
    void autoFit();

    ResultStore *m_store;
    QHeaderView *m_header;
    RangeSet m_sel;
    qint64 m_current = -1;
    qint64 m_anchor = -1;
    qint64 m_hover = -1;
    int m_rowH = 24;
    int m_iconSize = 16;
    QStringList m_terms;
    bool m_paintingSelected = false;
    int m_emptyGlyph = -1;
    QString m_emptyTitle;
    QString m_emptyBody;
    QString m_home;

    QPoint m_pressPos;
    qint64 m_pressRow = -1;
    bool m_collapseOnRelease = false;

    // What was selected and on screen before a refresh, by path, so the same
    // files stay selected and in place when rows shift.
    QStringList m_snapSel;
    QString m_snapCurrent;
    QString m_snapTop;
    qint64 m_snapTopRow = 0;
    qint64 m_snapTotal = 0;
    bool m_snapExact = true;

    bool m_autoFit = false;
    bool m_fitPending = false;
    // Widest content seen per column since the last new search; columns only
    // grow while scrolling one result set, so they do not jitter.
    int m_fitSeen[ColCount] = {};

    QMimeDatabase m_mime;
    QHash<QString, QIcon> m_icons;
    QIcon m_folderIcon;
    QIcon m_fileIcon;
};
