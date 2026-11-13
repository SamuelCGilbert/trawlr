// A small set of line icons, drawn with QPainter so they are sharp at any
// size and scale and need no image files or icon theme. They take their
// colour from the current theme when painted, so they follow theme changes.
#pragma once
#include <QColor>
#include <QIcon>

class QPainter;
class QRectF;

namespace Icons {

enum Glyph {
    Search,
    Close,
    Menu,
    Folder,
    File,
    Check,
    ChevronDown,
    ChevronUp,
    ChevronRight,
    ChevronLeft,
    MatchCase,
    WholeWord,
    MatchPath,
    Regex,
    Offline,
    Warning,
    NoResults,
    Settings,
    Plus,
};

// Which theme colour an icon is drawn in. Checked icons (State On) always use
// the accent; hovered ones (Active mode) the full text colour.
enum Tone { Dim, Strong, Faint, Accent, OnAccent };

QIcon icon(Glyph g, Tone tone = Dim);
// Draws the glyph centred in `r` (a 24-unit design scaled to fit).
void paint(QPainter &p, Glyph g, const QRectF &r, const QColor &color);

} // namespace Icons
