#include "resultsview.h"
#include "icons.h"
#include "theme.h"

#include <QDir>
#include <algorithm>
#include <iterator>
#include <QMouseEvent>

#include <QApplication>
#include <QContextMenuEvent>
#include <QDateTime>
#include <QHeaderView>
#include <QHelpEvent>
#include <QLocale>
#include <QPainter>
#include <QScrollBar>
#include <QStandardItemModel>
#include <QStyle>
#include <QStyleOptionViewItem>
#include <QTextLayout>
#include <QToolTip>

// Column titles drawn quietly, with an accent chevron on the sorted column.
class ResultsHeader : public QHeaderView {
public:
    explicit ResultsHeader(QWidget *parent) : QHeaderView(Qt::Horizontal, parent) { setMouseTracking(true); }

    QSize sizeHint() const override
    {
        QSize s = QHeaderView::sizeHint();
        s.setHeight(fontMetrics().height() + (Theme::compact() ? 10 : 16));
        return s;
    }

protected:
    void paintEvent(QPaintEvent *e) override
    {
        {
            QPainter p(viewport());
            p.fillRect(viewport()->rect(), Theme::colors().canvas);
            p.setPen(Theme::colors().divider);
            p.drawLine(0, viewport()->height() - 1, viewport()->width(), viewport()->height() - 1);
        }
        // Sections only; the empty area right of them stays plain.
        QPainter p(viewport());
        for (int v = 0; v < count(); ++v) {
            const int logical = logicalIndex(v);
            if (isSectionHidden(logical))
                continue;
            const QRect r(sectionViewportPosition(logical), 0, sectionSize(logical), viewport()->height());
            if (r.intersects(e->rect()))
                paintSection(&p, r, logical);
        }
    }

    void paintSection(QPainter *p, const QRect &rect, int logical) const override
    {
        const Theme::Colors &c = Theme::colors();
        p->save();
        p->setRenderHint(QPainter::Antialiasing);
        if (logical == m_hover) {
            p->setPen(Qt::NoPen);
            p->setBrush(c.hover);
            p->drawRoundedRect(QRectF(rect).adjusted(3, 4, -3, -5), 6, 6);
        }
        p->setRenderHint(QPainter::Antialiasing, false);
        p->setPen(c.divider);
        if (visualIndex(logical) < count() - 1)
            p->drawLine(rect.right(), rect.top() + 9, rect.right(), rect.bottom() - 9);

        QFont f = font();
        f.setWeight(QFont::DemiBold);
        f.setPointSizeF(f.pointSizeF() * 0.92);
        p->setFont(f);
        const bool sorted = isSortIndicatorShown() && sortIndicatorSection() == logical;
        const QString label = model()->headerData(logical, Qt::Horizontal).toString();
        const bool right = logical == ResultsView::ColSize;
        const QRect tr = rect.adjusted(12, 0, -12, -1);
        const int textW = qMin(QFontMetrics(f).horizontalAdvance(label), tr.width());
        p->setPen(sorted ? c.textDim : c.textFaint);
        p->drawText(tr, Qt::AlignVCenter | (right ? Qt::AlignRight : Qt::AlignLeft),
                    QFontMetrics(f).elidedText(label, Qt::ElideRight, tr.width()));
        if (sorted) {
            const int x = right ? tr.right() - textW - 16 : tr.left() + textW + 4;
            Icons::paint(*p, sortIndicatorOrder() == Qt::DescendingOrder ? Icons::ChevronDown : Icons::ChevronUp,
                         QRectF(x, rect.center().y() - 7, 14, 14), c.accentText);
        }
        p->restore();
    }

    void mouseMoveEvent(QMouseEvent *e) override
    {
        const int h = e->buttons() ? -1 : logicalIndexAt(e->position().toPoint());
        if (h != m_hover) {
            m_hover = h;
            viewport()->update();
        }
        QHeaderView::mouseMoveEvent(e);
    }

    void leaveEvent(QEvent *e) override
    {
        m_hover = -1;
        viewport()->update();
        QHeaderView::leaveEvent(e);
    }

private:
    int m_hover = -1;
};

ResultsView::ResultsView(ResultStore *store, QWidget *parent) : QAbstractScrollArea(parent), m_store(store)
{
    // The header only needs something to take its labels from.
    auto *labels = new QStandardItemModel(0, ColCount, this);
    labels->setHorizontalHeaderLabels({tr("Name"), tr("Path"), tr("Size"), tr("Modified")});
    m_header = new ResultsHeader(this);
    m_header->setModel(labels);
    m_header->setSectionsClickable(true);
    m_header->setSectionsMovable(true);
    m_header->setSortIndicatorShown(true);
    m_header->setHighlightSections(false);
    m_header->setDefaultAlignment(Qt::AlignLeft | Qt::AlignVCenter);
    m_header->setMinimumSectionSize(48);
    m_header->resizeSection(ColName, 340);
    m_header->resizeSection(ColPath, 420);
    m_header->resizeSection(ColSize, 96);
    m_header->resizeSection(ColDate, 180);
    connect(m_header, &QHeaderView::sectionResized, this, [this] { updateGeometries(); viewport()->update(); });
    connect(m_header, &QHeaderView::sectionMoved, this, [this] { viewport()->update(); });
    connect(m_header, &QHeaderView::sectionClicked, this, &ResultsView::sortRequested);
    // Only sent for sections that can be dragged (not the stretching Path).
    connect(m_header, &QHeaderView::sectionHandleDoubleClicked, this, &ResultsView::fitColumn);

    setFocusPolicy(Qt::StrongFocus);
    setFrameShape(QFrame::NoFrame);
    viewport()->setMouseTracking(true);
    viewport()->setBackgroundRole(QPalette::Base);
    viewport()->setAutoFillBackground(true);
    m_folderIcon = QIcon::fromTheme(QStringLiteral("folder"), QIcon::fromTheme(QStringLiteral("inode-directory"), Icons::icon(Icons::Folder, Icons::Accent)));
    m_fileIcon = QIcon::fromTheme(QStringLiteral("unknown"), QIcon::fromTheme(QStringLiteral("text-x-generic"), Icons::icon(Icons::File, Icons::Dim)));
    m_home = QDir::homePath();
    updateMetrics();
    connect(static_cast<ThemeNotifier *>(Theme::notifier()), &ThemeNotifier::changed, this, [this] {
        updateMetrics();
        updateGeometries();
        std::fill(std::begin(m_fitSeen), std::end(m_fitSeen), 0);
        scheduleAutoFit();
        m_header->viewport()->update();
        viewport()->update();
    });

    connect(store, &ResultStore::resultsReset, this, [this] {
        m_sel.clear();
        m_current = m_anchor = m_hover = -1;
        updateGeometries();
        verticalScrollBar()->setValue(0);
        // A new result set: fit to it afresh once its rows arrive.
        std::fill(std::begin(m_fitSeen), std::end(m_fitSeen), 0);
        scheduleAutoFit();
        viewport()->update();
        emit selectionChanged();
    });
    connect(store, &ResultStore::rowsLoaded, this, [this] {
        scheduleAutoFit();
        viewport()->update();
    });

    connect(store, &ResultStore::aboutToRefresh, this, [this] {
        m_snapSel.clear();
        m_snapExact = m_sel.count() <= 5000;
        if (m_snapExact) {
            for (qint64 r : m_sel.rows(5000)) {
                if (const Row *row = m_store->cached(r))
                    m_snapSel.append(row->path());
            }
        }
        const Row *cur = m_store->cached(m_current);
        m_snapCurrent = cur ? cur->path() : QString();
        m_snapTopRow = verticalScrollBar()->value();
        const Row *top = m_store->cached(m_snapTopRow);
        m_snapTop = top ? top->path() : QString();
        m_snapTotal = m_store->total();
    });
    connect(store, &ResultStore::refreshed, this, [this] {
        // Where did the rows we cared about end up? Look around the old spot.
        QHash<QString, qint64> where;
        const qint64 from = qMax<qint64>(0, m_snapTopRow - 3 * ResultStore::PageSize);
        const qint64 to = qMin(m_store->total(), m_snapTopRow + visibleRows() + 4 * ResultStore::PageSize);
        for (qint64 r = from; r < to; ++r) {
            if (const Row *row = m_store->cached(r))
                where.insert(row->path(), r);
        }
        if (m_snapExact) {
            m_sel.clear();
            for (const QString &p : std::as_const(m_snapSel)) {
                const auto it = where.constFind(p);
                if (it != where.constEnd())
                    m_sel.add(*it, *it);
            }
        } else if (m_store->total() != m_snapTotal) {
            // Too many to track one by one, and the rows have shifted: better
            // nothing selected than the wrong files.
            m_sel.clear();
        }
        const auto cur = where.constFind(m_snapCurrent);
        m_current = cur != where.constEnd() ? *cur : (m_sel.isEmpty() ? -1 : m_sel.ranges().first().first);
        m_anchor = m_current;
        updateGeometries();
        const auto top = where.constFind(m_snapTop);
        if (m_snapTopRow > 0 && top != where.constEnd())
            verticalScrollBar()->setValue(int(*top));
        m_hover = -1;
        scheduleAutoFit();
        viewport()->update();
        emit selectionChanged();
    });
}

void ResultsView::updateMetrics()
{
    const bool compact = Theme::compact();
    m_iconSize = compact ? 16 : 20;
    m_rowH = qMax(fontMetrics().height() + (compact ? 8 : 14), compact ? 26 : 34);
}

void ResultsView::changeEvent(QEvent *e)
{
    QAbstractScrollArea::changeEvent(e);
    if (e->type() == QEvent::FontChange || e->type() == QEvent::StyleChange) {
        updateMetrics();
        m_icons.clear();
        updateGeometries();
        std::fill(std::begin(m_fitSeen), std::end(m_fitSeen), 0);
        scheduleAutoFit();
    }
}

int ResultsView::visibleRows() const
{
    return qMax(1, viewport()->height() / m_rowH);
}

void ResultsView::updateGeometries()
{
    const int hh = m_header->sizeHint().height();
    setViewportMargins(0, hh, 0, 0);
    const QRect vr = viewport()->geometry();
    m_header->setGeometry(vr.left(), vr.top() - hh, vr.width(), hh);

    const qint64 total = m_store->total();
    const int vis = visibleRows();
    verticalScrollBar()->setRange(0, int(qMax<qint64>(0, total - vis)));
    verticalScrollBar()->setPageStep(vis);
    verticalScrollBar()->setSingleStep(1);
    horizontalScrollBar()->setRange(0, qMax(0, m_header->length() - vr.width()));
    horizontalScrollBar()->setPageStep(vr.width());
    horizontalScrollBar()->setSingleStep(24);
    m_header->setOffset(horizontalScrollBar()->value());
}

void ResultsView::resizeEvent(QResizeEvent *e)
{
    QAbstractScrollArea::resizeEvent(e);
    autoFit(); // now, so the columns never lag a frame behind the window
    updateGeometries();
}

void ResultsView::scrollContentsBy(int, int dy)
{
    if (dy)
        scheduleAutoFit();
    m_header->setOffset(horizontalScrollBar()->value());
    m_hover = -1;
    viewport()->update();
}

qint64 ResultsView::rowAt(int y) const
{
    if (y < 0)
        return -1;
    const qint64 r = verticalScrollBar()->value() + y / m_rowH;
    return r < m_store->total() ? r : -1;
}

QIcon ResultsView::iconFor(const Row &row)
{
    if (row.isDir)
        return m_folderIcon;
    const int dot = row.name.lastIndexOf(QLatin1Char('.'));
    // Extension-less names are looked up whole (Makefile, README...).
    const QString key = dot > 0 ? row.name.mid(dot).toLower() : row.name;
    auto it = m_icons.constFind(key);
    if (it != m_icons.constEnd())
        return *it;
    const QMimeType mt = m_mime.mimeTypeForFile(row.name, QMimeDatabase::MatchExtension);
    QIcon icon = QIcon::fromTheme(mt.iconName());
    if (icon.isNull())
        icon = QIcon::fromTheme(mt.genericIconName(), m_fileIcon);
    if (m_icons.size() > 4000)
        m_icons.clear();
    m_icons.insert(key, icon);
    return icon;
}

void ResultsView::drawCellText(QPainter &p, const QRect &rect, const QString &text, int align, Qt::TextElideMode elide, bool highlight)
{
    if (rect.width() <= 4)
        return;
    const QString shown = p.fontMetrics().elidedText(text, elide, rect.width());
    QList<QTextLayout::FormatRange> marks;
    if (highlight) {
        // Matched words in the accent colour (on a selected row, just heavier).
        const Theme::Colors &c = Theme::colors();
        for (const QString &t : std::as_const(m_terms)) {
            for (int at = shown.indexOf(t, 0, Qt::CaseInsensitive); at >= 0; at = shown.indexOf(t, at + t.size(), Qt::CaseInsensitive)) {
                QTextLayout::FormatRange fr;
                fr.start = at;
                fr.length = t.size();
                fr.format.setFontWeight(QFont::Bold);
                if (!m_paintingSelected)
                    fr.format.setForeground(c.accentText);
                marks.append(fr);
            }
        }
    }
    if (marks.isEmpty()) {
        p.drawText(rect, align | Qt::AlignVCenter | Qt::TextSingleLine, shown);
        return;
    }
    QTextLayout layout(shown, p.font());
    layout.setFormats(marks);
    layout.beginLayout();
    QTextLine line = layout.createLine();
    line.setLineWidth(100000);
    layout.endLayout();
    p.save();
    p.setClipRect(rect);
    layout.draw(&p, QPointF(rect.left(), rect.top() + (rect.height() - line.height()) / 2.0));
    p.restore();
}

QString ResultsView::displayDir(const QString &dir) const
{
    // Your own home folder as ~, the way a shell shows it (not a Windows habit).
    if (QDir::separator() == QLatin1Char('/') && !m_home.isEmpty() && m_home != QLatin1String("/") && dir.startsWith(m_home)
        && (dir.size() == m_home.size() || dir.at(m_home.size()) == QLatin1Char('/')))
        return QLatin1Char('~') + dir.mid(m_home.size());
    return dir;
}

QString ResultsView::displayDate(qint64 secs) const
{
    const QDateTime dt = QDateTime::fromSecsSinceEpoch(secs);
    const QDate d = dt.date();
    const QDate today = QDate::currentDate();
    const QLocale loc;
    const QString time = loc.toString(dt.time(), QLocale::ShortFormat);
    if (d == today)
        return tr("Today, %1").arg(time);
    if (d == today.addDays(-1))
        return tr("Yesterday, %1").arg(time);
    const QString fmt = loc.dateFormat(QLocale::ShortFormat);
    const bool dayFirst = fmt.indexOf(QLatin1Char('d')) < fmt.indexOf(QLatin1Char('M'));
    if (d.year() == today.year() && d < today)
        return loc.toString(d, dayFirst ? QStringLiteral("d MMM") : QStringLiteral("MMM d")) + QStringLiteral(", ") + time;
    return loc.toString(d, dayFirst ? QStringLiteral("d MMM yyyy") : QStringLiteral("MMM d, yyyy"));
}

// The widest cell of this column among the loaded rows on screen, padding
// included; -1 when none of them is loaded yet.
int ResultsView::contentWidth(int col) const
{
    const qint64 total = m_store->total();
    const qint64 top = verticalScrollBar()->value();
    const qint64 end = qMin(total, top + visibleRows() + 1);
    const QFontMetrics fm = fontMetrics();
    QFont bold = font();
    bold.setWeight(QFont::Bold);
    const QFontMetrics bfm(bold);
    int widest = -1;
    for (qint64 r = top; r < end; ++r) {
        const Row *row = m_store->cached(r);
        if (!row)
            continue;
        int w = 0;
        switch (col) {
        case ColName:
            w = fm.horizontalAdvance(row->name) + m_iconSize + 10;
            // Matched words are drawn bold, which is wider.
            for (const QString &t : m_terms) {
                const int n = row->name.count(t, Qt::CaseInsensitive);
                if (n)
                    w += n * qMax(0, bfm.horizontalAdvance(t) - fm.horizontalAdvance(t));
            }
            break;
        case ColPath:
            w = fm.horizontalAdvance(displayDir(row->dir));
            break;
        case ColSize:
            if (!row->isDir)
                w = fm.horizontalAdvance(QLocale().formattedDataSize(row->size, row->size < 1024 ? 0 : 1, QLocale::DataSizeTraditionalFormat));
            break;
        case ColDate:
            if (row->mtime > 0)
                w = fm.horizontalAdvance(displayDate(row->mtime));
            break;
        }
        widest = qMax(widest, w + 2 * 12 + 2);
    }
    return widest;
}

// Room for the column's title and the sort chevron beside it.
int ResultsView::headerWidth(int col) const
{
    QFont f = m_header->font();
    f.setWeight(QFont::DemiBold);
    f.setPointSizeF(f.pointSizeF() * 0.92);
    const QString label = m_header->model()->headerData(col, Qt::Horizontal).toString();
    return qMax(m_header->minimumSectionSize(), QFontMetrics(f).horizontalAdvance(label) + 2 * 12 + 18 + 2);
}

void ResultsView::fitColumn(int col)
{
    if (col < 0 || col >= ColCount || m_header->sectionResizeMode(col) == QHeaderView::Stretch)
        return;
    const int hw = headerWidth(col);
    const int w = qMax(hw, contentWidth(col));
    // A very long name should not push everything else out of sight.
    m_header->resizeSection(col, qMin(w, qMax(hw, viewport()->width() * 3 / 4)));
}

void ResultsView::setAutoFitColumns(bool on)
{
    m_autoFit = on;
    for (int c = 0; c < ColCount; ++c)
        m_header->setSectionResizeMode(c, on ? QHeaderView::Fixed : QHeaderView::Interactive);
    if (!on) {
        // The path takes whatever width the other columns leave.
        m_header->setSectionResizeMode(ColPath, QHeaderView::Stretch);
        return;
    }
    std::fill(std::begin(m_fitSeen), std::end(m_fitSeen), 0);
    autoFit();
}

void ResultsView::scheduleAutoFit()
{
    if (!m_autoFit || m_fitPending)
        return;
    m_fitPending = true;
    QMetaObject::invokeMethod(this, [this] {
        m_fitPending = false;
        autoFit();
    }, Qt::QueuedConnection);
}

void ResultsView::autoFit()
{
    if (!m_autoFit)
        return;
    for (int c = 0; c < ColCount; ++c) {
        const int w = contentWidth(c);
        if (w >= 0) // else keep what was seen, until rows arrive
            m_fitSeen[c] = qMax(m_fitSeen[c], qMax(w, headerWidth(c)));
    }
    if (m_fitSeen[ColName] == 0)
        return; // nothing measured yet; leave the columns as they are

    const int avail = viewport()->width();
    const int size = m_fitSeen[ColSize];
    const int date = m_fitSeen[ColDate];
    // Name keeps at least its own width or two fifths of the window,
    // whichever is less; a long path gives way (shown elided) before it.
    const int nameMin = qMax(headerWidth(ColName), qMin(m_fitSeen[ColName], qMax(200, avail * 2 / 5)));
    const int pathMin = qMin(m_fitSeen[ColPath], 160);
    const int path = qMin(m_fitSeen[ColPath], qMax(pathMin, avail - size - date - nameMin));
    const int name = qMax(nameMin, avail - size - date - path);
    const int widths[ColCount] = {name, path, size, date};
    for (int c = 0; c < ColCount; ++c) {
        if (m_header->sectionSize(c) != widths[c])
            m_header->resizeSection(c, widths[c]);
    }
}

void ResultsView::paintRow(QPainter &p, qint64 r, const QRect &rect)
{
    const Theme::Colors &c = Theme::colors();
    const Row *row = m_store->row(r);
    const bool selected = m_sel.contains(r);
    m_paintingSelected = selected;

    // Rounded highlight, inset from the edges.
    const QRectF bg = QRectF(rect).adjusted(6, 1, -6, -1);
    if (selected || r == m_hover) {
        p.save();
        p.setRenderHint(QPainter::Antialiasing);
        p.setPen(Qt::NoPen);
        p.setBrush(selected ? (hasFocus() ? c.selection : c.selectionInactive) : c.hover);
        p.drawRoundedRect(bg, 7, 7);
        p.restore();
    }
    if (r == m_current && hasFocus() && !selected) {
        p.save();
        p.setRenderHint(QPainter::Antialiasing);
        p.setPen(QPen(c.accent, 1));
        p.setBrush(Qt::NoBrush);
        p.drawRoundedRect(bg.adjusted(0.5, 0.5, -0.5, -0.5), 7, 7);
        p.restore();
    }

    if (!row)
        return; // still loading; filled in when the page arrives

    const QColor main = row->gone ? c.textFaint : c.text;
    const QColor dim = row->gone ? c.textFaint : c.textDim;
    QFont nameFont = font();
    nameFont.setStrikeOut(row->gone);

    const int pad = 12;
    for (int v = 0; v < m_header->count(); ++v) {
        const int col = m_header->logicalIndex(v);
        if (m_header->isSectionHidden(col))
            continue;
        QRect cell(m_header->sectionViewportPosition(col), rect.top(), m_header->sectionSize(col), rect.height());
        if (!cell.intersects(rect))
            continue;
        cell.adjust(pad, 0, -pad, 0);
        switch (col) {
        case ColName: {
            const QRect iconRect(cell.left(), cell.top() + (cell.height() - m_iconSize) / 2, m_iconSize, m_iconSize);
            iconFor(*row).paint(&p, iconRect, Qt::AlignCenter, row->gone ? QIcon::Disabled : QIcon::Normal);
            cell.setLeft(cell.left() + m_iconSize + 10);
            p.setPen(main);
            p.setFont(nameFont);
            drawCellText(p, cell, row->name, Qt::AlignLeft, Qt::ElideRight, true);
            p.setFont(font());
            break;
        }
        case ColPath:
            p.setPen(dim);
            drawCellText(p, cell, displayDir(row->dir), Qt::AlignLeft, Qt::ElideMiddle, false);
            break;
        case ColSize:
            if (!row->isDir) {
                p.setPen(dim);
                drawCellText(p, cell, QLocale().formattedDataSize(row->size, row->size < 1024 ? 0 : 1, QLocale::DataSizeTraditionalFormat), Qt::AlignRight, Qt::ElideRight, false);
            }
            break;
        case ColDate:
            if (row->mtime > 0) {
                p.setPen(dim);
                drawCellText(p, cell, displayDate(row->mtime), Qt::AlignLeft, Qt::ElideRight, false);
            }
            break;
        }
    }
}

void ResultsView::paintEmptyState(QPainter &p)
{
    if (m_emptyTitle.isEmpty())
        return;
    const Theme::Colors &c = Theme::colors();
    const QRect area = viewport()->rect();
    const int width = qMin(480, area.width() - 48);
    if (width < 80)
        return;

    QFont titleFont = font();
    titleFont.setPointSizeF(titleFont.pointSizeF() * 1.25);
    titleFont.setWeight(QFont::DemiBold);
    const QFontMetrics tfm(titleFont);
    const QRect bodyBounds = fontMetrics().boundingRect(QRect(0, 0, width, 10000), Qt::AlignHCenter | Qt::TextWordWrap, m_emptyBody);
    const int glyphBox = m_emptyGlyph >= 0 ? 64 : 0;
    const int total = glyphBox + (glyphBox ? 18 : 0) + tfm.height() + (m_emptyBody.isEmpty() ? 0 : 8 + bodyBounds.height());
    int y = qMax(24, area.height() * 2 / 5 - total / 2);
    const int cx = area.center().x();

    p.save();
    p.setRenderHint(QPainter::Antialiasing);
    if (glyphBox) {
        const QRectF circle(cx - glyphBox / 2.0, y, glyphBox, glyphBox);
        p.setPen(Qt::NoPen);
        p.setBrush(c.field);
        p.drawEllipse(circle);
        p.setPen(QPen(c.divider, 1));
        p.setBrush(Qt::NoBrush);
        p.drawEllipse(circle.adjusted(0.5, 0.5, -0.5, -0.5));
        const QColor glyphColor = m_emptyGlyph == Icons::Offline || m_emptyGlyph == Icons::Warning ? c.warning : c.textDim;
        Icons::paint(p, Icons::Glyph(m_emptyGlyph), circle.adjusted(18, 18, -18, -18), glyphColor);
        y += glyphBox + 18;
    }
    p.setFont(titleFont);
    p.setPen(c.text);
    p.drawText(QRect(cx - width / 2, y, width, tfm.height()), Qt::AlignHCenter | Qt::AlignVCenter, tfm.elidedText(m_emptyTitle, Qt::ElideMiddle, width));
    y += tfm.height() + 8;
    if (!m_emptyBody.isEmpty()) {
        p.setFont(font());
        p.setPen(c.textDim);
        p.drawText(QRect(cx - width / 2, y, width, bodyBounds.height()), Qt::AlignHCenter | Qt::TextWordWrap, m_emptyBody);
    }
    p.restore();
}

void ResultsView::setEmptyState(int glyph, const QString &title, const QString &body)
{
    if (glyph == m_emptyGlyph && title == m_emptyTitle && body == m_emptyBody)
        return;
    m_emptyGlyph = glyph;
    m_emptyTitle = title;
    m_emptyBody = body;
    if (m_store->total() == 0)
        viewport()->update();
}

void ResultsView::paintEvent(QPaintEvent *e)
{
    QPainter p(viewport());
    const qint64 total = m_store->total();
    if (total == 0) {
        paintEmptyState(p);
        return;
    }
    const qint64 top = verticalScrollBar()->value();
    m_store->setVisibleRange(top, visibleRows() + 1);
    const qint64 first = top + e->rect().top() / m_rowH;
    const qint64 last = qMin(total - 1, top + e->rect().bottom() / m_rowH);
    for (qint64 r = first; r <= last; ++r)
        paintRow(p, r, QRect(0, int(r - top) * m_rowH, viewport()->width(), m_rowH));
}

void ResultsView::setHighlightTerms(const QStringList &terms)
{
    m_terms = terms;
    viewport()->update();
}

void ResultsView::selectAll()
{
    if (m_store->total() > 0) {
        m_sel.set(0, m_store->total() - 1);
        viewport()->update();
        emit selectionChanged();
    }
}

void ResultsView::selectOnly(qint64 row)
{
    if (row < 0 || row >= m_store->total())
        return;
    m_sel.set(row, row);
    m_current = m_anchor = row;
    ensureVisible(row);
    viewport()->update();
    emit selectionChanged();
}

QList<Row> ResultsView::loadedSelection(int max) const
{
    QList<Row> out;
    for (qint64 r : m_sel.rows(max)) {
        if (const Row *row = m_store->cached(r))
            out.append(*row);
    }
    return out;
}

void ResultsView::ensureVisible(qint64 row)
{
    const qint64 top = verticalScrollBar()->value();
    const int vis = visibleRows();
    if (row < top)
        verticalScrollBar()->setValue(int(row));
    else if (row >= top + vis)
        verticalScrollBar()->setValue(int(row - vis + 1));
}

void ResultsView::moveCurrent(qint64 row, Qt::KeyboardModifiers mods)
{
    const qint64 total = m_store->total();
    if (total == 0)
        return;
    row = qBound<qint64>(0, row, total - 1);
    if (mods & Qt::ShiftModifier) {
        if (m_anchor < 0)
            m_anchor = m_current < 0 ? row : m_current;
        m_sel.set(m_anchor, row);
    } else if (!(mods & Qt::ControlModifier)) {
        m_sel.set(row, row);
        m_anchor = row;
    }
    m_current = row;
    ensureVisible(row);
    viewport()->update();
    emit selectionChanged();
}

void ResultsView::mousePressEvent(QMouseEvent *e)
{
    setFocus(Qt::MouseFocusReason);
    const qint64 row = rowAt(e->pos().y());
    m_pressPos = e->pos();
    m_pressRow = row;
    m_collapseOnRelease = false;
    if (e->button() == Qt::RightButton) {
        if (row >= 0 && !m_sel.contains(row)) {
            m_sel.set(row, row);
            m_current = m_anchor = row;
        }
    } else if (e->button() == Qt::LeftButton) {
        if (row < 0) {
            if (!(e->modifiers() & (Qt::ControlModifier | Qt::ShiftModifier)))
                m_sel.clear();
        } else if (e->modifiers() & Qt::ShiftModifier) {
            const qint64 anchor = m_anchor < 0 ? row : m_anchor;
            if (e->modifiers() & Qt::ControlModifier)
                m_sel.add(anchor, row);
            else
                m_sel.set(anchor, row);
            m_anchor = anchor;
            m_current = row;
        } else if (e->modifiers() & Qt::ControlModifier) {
            m_sel.toggle(row);
            m_current = m_anchor = row;
        } else if (m_sel.contains(row)) {
            // Might be the start of dragging the whole selection; narrow it
            // to this row only if the button comes up without a drag.
            m_collapseOnRelease = true;
            m_current = row;
        } else {
            m_sel.set(row, row);
            m_current = m_anchor = row;
        }
    }
    viewport()->update();
    emit selectionChanged();
}

void ResultsView::mouseMoveEvent(QMouseEvent *e)
{
    if ((e->buttons() & Qt::LeftButton) && m_pressRow >= 0 && m_sel.contains(m_pressRow)
        && (e->pos() - m_pressPos).manhattanLength() >= QApplication::startDragDistance()) {
        m_collapseOnRelease = false;
        m_pressRow = -1;
        emit dragRequested();
        return;
    }
    const qint64 row = rowAt(e->pos().y());
    if (row != m_hover) {
        m_hover = row;
        viewport()->update();
    }
}

void ResultsView::mouseReleaseEvent(QMouseEvent *e)
{
    if (e->button() == Qt::LeftButton && m_collapseOnRelease && m_pressRow >= 0) {
        m_sel.set(m_pressRow, m_pressRow);
        m_anchor = m_pressRow;
        viewport()->update();
        emit selectionChanged();
    }
    m_collapseOnRelease = false;
}

void ResultsView::mouseDoubleClickEvent(QMouseEvent *e)
{
    const qint64 row = rowAt(e->pos().y());
    if (e->button() == Qt::LeftButton && row >= 0) {
        m_collapseOnRelease = false;
        // Act on the row under the pointer. (Shift+double-click opens the item
        // clicked, not the range the first click of the pair selected.)
        if ((e->modifiers() & Qt::ShiftModifier) || !m_sel.contains(row))
            selectOnly(row);
        emit activated(m_header->logicalIndexAt(e->pos().x()), e->modifiers());
    }
}

void ResultsView::keyPressEvent(QKeyEvent *e)
{
    const qint64 cur = m_current < 0 ? verticalScrollBar()->value() - 1 : m_current;
    const int page = qMax(1, visibleRows() - 1);
    switch (e->key()) {
    case Qt::Key_Down: moveCurrent(cur + 1, e->modifiers()); return;
    case Qt::Key_Up: moveCurrent(qMax<qint64>(0, cur - 1), e->modifiers()); return;
    case Qt::Key_PageDown: moveCurrent(cur + page, e->modifiers()); return;
    case Qt::Key_PageUp: moveCurrent(cur - page, e->modifiers()); return;
    case Qt::Key_Home: moveCurrent(0, e->modifiers()); return;
    case Qt::Key_End: moveCurrent(m_store->total() - 1, e->modifiers()); return;
    case Qt::Key_Return:
    case Qt::Key_Enter:
        if (!(e->modifiers() & (Qt::ControlModifier | Qt::AltModifier)) && !m_sel.isEmpty()) {
            emit activated(-1, e->modifiers());
            return;
        }
        break;
    default:
        break;
    }
    // Typing while the list has focus continues the search.
    const QString text = e->text();
    if (!text.isEmpty() && text.at(0).isPrint() && !(e->modifiers() & (Qt::ControlModifier | Qt::AltModifier | Qt::MetaModifier))) {
        emit textTyped(text);
        return;
    }
    QAbstractScrollArea::keyPressEvent(e);
}

void ResultsView::contextMenuEvent(QContextMenuEvent *e)
{
    if (e->reason() == QContextMenuEvent::Keyboard && m_current >= 0) {
        const int y = int(m_current - verticalScrollBar()->value()) * m_rowH + m_rowH;
        emit contextMenuRequested(viewport()->mapToGlobal(QPoint(40, y)));
    } else if (!m_sel.isEmpty()) {
        emit contextMenuRequested(e->globalPos());
    }
}

void ResultsView::focusInEvent(QFocusEvent *e)
{
    QAbstractScrollArea::focusInEvent(e);
    viewport()->update();
}

void ResultsView::focusOutEvent(QFocusEvent *e)
{
    QAbstractScrollArea::focusOutEvent(e);
    viewport()->update();
}

bool ResultsView::viewportEvent(QEvent *e)
{
    if (e->type() == QEvent::ToolTip) {
        auto *he = static_cast<QHelpEvent *>(e);
        const Row *row = m_store->cached(rowAt(he->pos().y()));
        if (row) {
            const QLocale loc;
            QString tip = row->path();
            if (row->mtime > 0)
                tip += QLatin1Char('\n') + tr("Modified: %1").arg(loc.toString(QDateTime::fromSecsSinceEpoch(row->mtime), QLocale::LongFormat));
            if (!row->isDir)
                tip += QLatin1Char('\n') + tr("Size: %1 bytes").arg(loc.toString(row->size));
            QToolTip::showText(he->globalPos(), tip, viewport());
        }
        else
            QToolTip::hideText();
        return true;
    }
    if (e->type() == QEvent::Leave && m_hover != -1) {
        m_hover = -1;
        viewport()->update();
    }
    return QAbstractScrollArea::viewportEvent(e);
}
