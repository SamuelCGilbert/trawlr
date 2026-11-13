#include "icons.h"
#include "theme.h"

#include <QApplication>
#include <QFont>
#include <QIconEngine>
#include <QPainter>
#include <QPainterPath>
#include <QPixmap>

namespace Icons {

// Strokes on a 24 x 24 grid, Lucide-like: 1.75 units wide, rounded ends.
static void strokeGlyph(QPainter &p, Glyph g)
{
    QPainterPath path;
    switch (g) {
    case Search:
        path.addEllipse(QPointF(10.5, 10.5), 6.5, 6.5);
        path.moveTo(15.5, 15.5);
        path.lineTo(20.5, 20.5);
        break;
    case Close:
        path.moveTo(6.5, 6.5);
        path.lineTo(17.5, 17.5);
        path.moveTo(17.5, 6.5);
        path.lineTo(6.5, 17.5);
        break;
    case Menu:
        for (qreal y : {6.5, 12.0, 17.5}) {
            path.moveTo(4.5, y);
            path.lineTo(19.5, y);
        }
        break;
    case Plus:
        path.moveTo(12, 5);
        path.lineTo(12, 19);
        path.moveTo(5, 12);
        path.lineTo(19, 12);
        break;
    case Folder:
        path.moveTo(3.5, 7);
        path.quadTo(3.5, 5, 5.5, 5);
        path.lineTo(9.2, 5);
        path.lineTo(11.2, 7.2);
        path.lineTo(18.5, 7.2);
        path.quadTo(20.5, 7.2, 20.5, 9.2);
        path.lineTo(20.5, 17.5);
        path.quadTo(20.5, 19.5, 18.5, 19.5);
        path.lineTo(5.5, 19.5);
        path.quadTo(3.5, 19.5, 3.5, 17.5);
        path.closeSubpath();
        break;
    case File:
        path.moveTo(14, 3.5);
        path.lineTo(7, 3.5);
        path.quadTo(5, 3.5, 5, 5.5);
        path.lineTo(5, 18.5);
        path.quadTo(5, 20.5, 7, 20.5);
        path.lineTo(17, 20.5);
        path.quadTo(19, 20.5, 19, 18.5);
        path.lineTo(19, 8.5);
        path.closeSubpath();
        path.moveTo(14, 3.5);
        path.lineTo(14, 8.5);
        path.lineTo(19, 8.5);
        break;
    case Check:
        path.moveTo(5, 12.5);
        path.lineTo(10, 17.5);
        path.lineTo(19.5, 7);
        break;
    case ChevronDown:
        path.moveTo(7, 10);
        path.lineTo(12, 15);
        path.lineTo(17, 10);
        break;
    case ChevronUp:
        path.moveTo(7, 14);
        path.lineTo(12, 9);
        path.lineTo(17, 14);
        break;
    case ChevronRight:
        path.moveTo(10, 7);
        path.lineTo(15, 12);
        path.lineTo(10, 17);
        break;
    case ChevronLeft:
        path.moveTo(14, 7);
        path.lineTo(9, 12);
        path.lineTo(14, 17);
        break;
    case MatchPath:
        // A folder with a slash through the middle: "the path".
        path.moveTo(3.5, 7);
        path.quadTo(3.5, 5, 5.5, 5);
        path.lineTo(9.2, 5);
        path.lineTo(11.2, 7.2);
        path.lineTo(18.5, 7.2);
        path.quadTo(20.5, 7.2, 20.5, 9.2);
        path.lineTo(20.5, 17.5);
        path.quadTo(20.5, 19.5, 18.5, 19.5);
        path.lineTo(5.5, 19.5);
        path.quadTo(3.5, 19.5, 3.5, 17.5);
        path.closeSubpath();
        path.moveTo(13.6, 10.2);
        path.lineTo(10.4, 16.6);
        break;
    case Offline:
        // A plug pulled out of its socket.
        path.moveTo(9, 3.5);
        path.lineTo(9, 7);
        path.moveTo(15, 3.5);
        path.lineTo(15, 7);
        path.moveTo(6.5, 7);
        path.lineTo(17.5, 7);
        path.lineTo(17.5, 10);
        path.quadTo(17.5, 15, 12, 15);
        path.quadTo(6.5, 15, 6.5, 10);
        path.closeSubpath();
        path.moveTo(12, 15);
        path.lineTo(12, 20.5);
        path.moveTo(3.5, 3.5);
        path.lineTo(20.5, 20.5);
        break;
    case Warning:
        path.moveTo(12, 3.8);
        path.lineTo(21, 19.5);
        path.lineTo(3, 19.5);
        path.closeSubpath();
        path.moveTo(12, 9.5);
        path.lineTo(12, 13.5);
        path.moveTo(12, 16.6);
        path.lineTo(12, 16.7);
        break;
    case NoResults:
        path.addEllipse(QPointF(10.5, 10.5), 6.5, 6.5);
        path.moveTo(15.5, 15.5);
        path.lineTo(20.5, 20.5);
        path.moveTo(8, 8);
        path.lineTo(13, 13);
        path.moveTo(13, 8);
        path.lineTo(8, 13);
        break;
    case Settings: {
        // Two sliders.
        path.moveTo(4, 8);
        path.lineTo(20, 8);
        path.moveTo(4, 16);
        path.lineTo(20, 16);
        path.addEllipse(QPointF(9, 8), 2.4, 2.4);
        path.addEllipse(QPointF(15, 16), 2.4, 2.4);
        break;
    }
    default:
        break;
    }
    p.drawPath(path);
}

void paint(QPainter &p, Glyph g, const QRectF &r, const QColor &color)
{
    const qreal side = qMin(r.width(), r.height());
    p.save();
    p.setRenderHint(QPainter::Antialiasing);
    p.translate(r.center().x() - side / 2, r.center().y() - side / 2);
    p.scale(side / 24.0, side / 24.0);

    if (g == MatchCase || g == WholeWord || g == Regex) {
        // Text glyphs, like the search toggles of code editors.
        QFont f = QApplication::font();
        f.setPixelSize(g == Regex ? 17 : 13);
        f.setWeight(QFont::DemiBold);
        p.setFont(f);
        p.setPen(color);
        const QString text = g == MatchCase ? QStringLiteral("Aa") : g == WholeWord ? QStringLiteral("ab") : QStringLiteral(".*");
        p.drawText(QRectF(0, g == Regex ? -1.5 : -1, 24, 24), Qt::AlignCenter, text);
        if (g == WholeWord) {
            QPen pen(color, 1.6, Qt::SolidLine, Qt::RoundCap, Qt::RoundJoin);
            p.setPen(pen);
            p.setBrush(Qt::NoBrush);
            QPainterPath bracket;
            bracket.moveTo(4.5, 15.5);
            bracket.lineTo(4.5, 18.5);
            bracket.lineTo(19.5, 18.5);
            bracket.lineTo(19.5, 15.5);
            p.drawPath(bracket);
        }
        p.restore();
        return;
    }
    QPen pen(color, 1.75, Qt::SolidLine, Qt::RoundCap, Qt::RoundJoin);
    p.setPen(pen);
    p.setBrush(Qt::NoBrush);
    strokeGlyph(p, g);
    p.restore();
}

static QColor toneColor(Tone t)
{
    const Theme::Colors &c = Theme::colors();
    switch (t) {
    case Strong: return c.text;
    case Faint: return c.textFaint;
    case Accent: return c.accentText;
    case OnAccent: return c.onAccent;
    case Dim:
    default: return c.textDim;
    }
}

class GlyphEngine : public QIconEngine {
public:
    GlyphEngine(Glyph g, Tone t) : m_glyph(g), m_tone(t) {}

    void paint(QPainter *p, const QRect &rect, QIcon::Mode mode, QIcon::State state) override
    {
        QColor c = toneColor(m_tone);
        if (state == QIcon::On && m_tone != OnAccent)
            c = toneColor(Accent);
        else if ((mode == QIcon::Active || mode == QIcon::Selected) && m_tone == Dim)
            c = toneColor(Strong);
        if (mode == QIcon::Disabled)
            c.setAlphaF(c.alphaF() * 0.4f);
        Icons::paint(*p, m_glyph, QRectF(rect), c);
    }

    QPixmap pixmap(const QSize &size, QIcon::Mode mode, QIcon::State state) override { return scaledPixmap(size, mode, state, 1.0); }

    QPixmap scaledPixmap(const QSize &size, QIcon::Mode mode, QIcon::State state, qreal scale) override
    {
        QPixmap pm(size * scale);
        pm.setDevicePixelRatio(scale);
        pm.fill(Qt::transparent);
        QPainter p(&pm);
        paint(&p, QRect(QPoint(), size), mode, state);
        return pm;
    }

    QSize actualSize(const QSize &size, QIcon::Mode, QIcon::State) override { return size; }
    QIconEngine *clone() const override { return new GlyphEngine(*this); }
    QString key() const override { return QStringLiteral("trawlr-glyph"); }

private:
    Glyph m_glyph;
    Tone m_tone;
};

QIcon icon(Glyph g, Tone tone)
{
    return QIcon(new GlyphEngine(g, tone));
}

} // namespace Icons
