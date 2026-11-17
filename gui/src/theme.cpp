#include "theme.h"
#include "icons.h"

#include <QAbstractButton>
#include <QApplication>
#include <QComboBox>
#include <QMenu>
#include <QPainter>
#include <QPainterPath>
#include <QPalette>
#include <QProxyStyle>
#include <QSettings>
#include <QStyleFactory>
#include <QStyleHints>
#include <QStyleOption>

namespace Theme {

static const QString kTheme = QStringLiteral("view/theme");
static const QString kAccent = QStringLiteral("view/accent");
static const QString kDensity = QStringLiteral("view/density");
static const QString kMenuBar = QStringLiteral("view/menuBar");

static Colors g_colors;
static QPalette g_systemPalette;
static bool g_ready = false;

QObject *notifier()
{
    static ThemeNotifier n;
    return &n;
}

const Colors &colors()
{
    return g_colors;
}

QString themeKey()
{
    const QString t = QSettings().value(kTheme, QStringLiteral("system")).toString();
    return t == QLatin1String("light") || t == QLatin1String("dark") ? t : QStringLiteral("system");
}

QStringList accentKeys()
{
    return {QStringLiteral("indigo"), QStringLiteral("blue"),  QStringLiteral("teal"),   QStringLiteral("green"),
            QStringLiteral("amber"),  QStringLiteral("rose"),  QStringLiteral("violet"), QStringLiteral("graphite")};
}

QString accentName(const QString &key)
{
    if (key == QLatin1String("indigo")) return QObject::tr("Indigo");
    if (key == QLatin1String("blue")) return QObject::tr("Blue");
    if (key == QLatin1String("teal")) return QObject::tr("Teal");
    if (key == QLatin1String("green")) return QObject::tr("Green");
    if (key == QLatin1String("amber")) return QObject::tr("Amber");
    if (key == QLatin1String("rose")) return QObject::tr("Rose");
    if (key == QLatin1String("violet")) return QObject::tr("Violet");
    if (key == QLatin1String("graphite")) return QObject::tr("Graphite");
    return QObject::tr("System");
}

static QColor systemAccent()
{
#if QT_VERSION >= QT_VERSION_CHECK(6, 6, 0)
    const QColor a = g_systemPalette.color(QPalette::Active, QPalette::Accent);
    if (a.isValid())
        return a;
#endif
    return g_systemPalette.color(QPalette::Active, QPalette::Highlight);
}

QColor accentColor(const QString &key)
{
    if (key == QLatin1String("blue")) return QColor(0x2f, 0x80, 0xed);
    if (key == QLatin1String("teal")) return QColor(0x12, 0x9e, 0x9a);
    if (key == QLatin1String("green")) return QColor(0x2d, 0xa1, 0x64);
    if (key == QLatin1String("amber")) return QColor(0xe3, 0x8b, 0x0b);
    if (key == QLatin1String("rose")) return QColor(0xe1, 0x46, 0x6c);
    if (key == QLatin1String("violet")) return QColor(0x95, 0x5d, 0xe3);
    if (key == QLatin1String("graphite")) return QColor(0x76, 0x7a, 0x86);
    if (key == QLatin1String("system")) return systemAccent();
    return QColor(0x63, 0x66, 0xf1); // indigo, the program's own colour
}

QString accentKey()
{
    const QString k = QSettings().value(kAccent, QStringLiteral("indigo")).toString();
    return k == QLatin1String("system") || accentKeys().contains(k) ? k : QStringLiteral("indigo");
}

QString densityKey()
{
    return QSettings().value(kDensity).toString() == QLatin1String("compact") ? QStringLiteral("compact") : QStringLiteral("comfortable");
}

bool compact()
{
    return densityKey() == QLatin1String("compact");
}

bool showMenuBar()
{
#ifdef Q_OS_MACOS
    return true; // the global menu bar is always there
#else
    return QSettings().value(kMenuBar, false).toBool();
#endif
}

static QColor mix(const QColor &a, const QColor &b, qreal t)
{
    return QColor::fromRgbF(float(a.redF() + (b.redF() - a.redF()) * t), float(a.greenF() + (b.greenF() - a.greenF()) * t),
                            float(a.blueF() + (b.blueF() - a.blueF()) * t), float(a.alphaF() + (b.alphaF() - a.alphaF()) * t));
}

static QColor alpha(QColor c, qreal a)
{
    c.setAlphaF(float(a));
    return c;
}

static qreal luminance(const QColor &c)
{
    auto lin = [](qreal v) { return v <= 0.03928 ? v / 12.92 : std::pow((v + 0.055) / 1.055, 2.4); };
    return 0.2126 * lin(c.redF()) + 0.7152 * lin(c.greenF()) + 0.0722 * lin(c.blueF());
}

static Colors makeColors(bool dark, QColor accent)
{
    Colors c;
    c.dark = dark;
    if (!accent.isValid())
        accent = accentColor(QStringLiteral("indigo"));
    accent.setAlpha(255);
    c.accent = accent;
    c.onAccent = luminance(accent) > 0.45 ? QColor(0x14, 0x15, 0x18) : QColor(Qt::white);
    if (dark) {
        c.canvas = QColor(0x17, 0x18, 0x1c);
        c.surface = QColor(0x1c, 0x1d, 0x22);
        c.elevated = QColor(0x24, 0x25, 0x2b);
        c.field = QColor(0x22, 0x23, 0x29);
        c.fieldHover = QColor(0x29, 0x2a, 0x31);
        c.border = QColor(0x35, 0x36, 0x3f);
        c.divider = QColor(0x27, 0x28, 0x2e);
        c.text = QColor(0xec, 0xec, 0xf0);
        c.textDim = QColor(0xa2, 0xa3, 0xad);
        c.textFaint = QColor(0x71, 0x72, 0x7d);
        c.hover = alpha(Qt::white, 0.05);
        c.pressed = alpha(Qt::white, 0.09);
        c.accentText = mix(accent, Qt::white, luminance(accent) < 0.2 ? 0.38 : 0.18);
        c.accentSoft = alpha(accent, 0.22);
        c.selection = alpha(accent, 0.30);
        c.selectionInactive = alpha(Qt::white, 0.08);
        c.success = QColor(0x3f, 0xc2, 0x7a);
        c.warning = QColor(0xf0, 0xb4, 0x3c);
        c.danger = QColor(0xf2, 0x67, 0x6b);
    } else {
        c.canvas = QColor(0xff, 0xff, 0xff);
        c.surface = QColor(0xf7, 0xf7, 0xf9);
        c.elevated = QColor(0xff, 0xff, 0xff);
        c.field = QColor(0xf4, 0xf4, 0xf6);
        c.fieldHover = QColor(0xee, 0xee, 0xf1);
        c.border = QColor(0xd9, 0xda, 0xe0);
        c.divider = QColor(0xea, 0xea, 0xee);
        c.text = QColor(0x18, 0x19, 0x1d);
        c.textDim = QColor(0x5a, 0x5c, 0x67);
        c.textFaint = QColor(0x82, 0x84, 0x8f);
        c.hover = alpha(Qt::black, 0.045);
        c.pressed = alpha(Qt::black, 0.08);
        c.accentText = luminance(accent) > 0.3 ? mix(accent, Qt::black, 0.35) : accent;
        c.accentSoft = alpha(accent, 0.13);
        c.selection = alpha(accent, 0.17);
        c.selectionInactive = alpha(Qt::black, 0.07);
        c.success = QColor(0x1f, 0x9d, 0x58);
        c.warning = QColor(0xc7, 0x84, 0x0a);
        c.danger = QColor(0xd9, 0x3f, 0x47);
    }
    return c;
}

static QPalette makePalette(const Colors &c)
{
    QPalette p;
    auto set = [&p](QPalette::ColorRole role, const QColor &normal, const QColor &disabled) {
        p.setColor(QPalette::Active, role, normal);
        p.setColor(QPalette::Inactive, role, normal);
        p.setColor(QPalette::Disabled, role, disabled);
    };
    const QColor solidSel = mix(c.canvas, c.accent, c.dark ? 0.45 : 0.25);
    set(QPalette::Window, c.canvas, c.canvas);
    set(QPalette::WindowText, c.text, c.textFaint);
    set(QPalette::Base, c.canvas, c.canvas);
    set(QPalette::AlternateBase, c.surface, c.surface);
    set(QPalette::Text, c.text, c.textFaint);
    set(QPalette::Button, c.field, c.field);
    set(QPalette::ButtonText, c.text, c.textFaint);
    set(QPalette::ToolTipBase, c.elevated, c.elevated);
    set(QPalette::ToolTipText, c.text, c.text);
    set(QPalette::PlaceholderText, c.textFaint, c.textFaint);
    set(QPalette::BrightText, Qt::white, Qt::white);
    set(QPalette::Light, c.fieldHover, c.fieldHover);
    set(QPalette::Midlight, c.field, c.field);
    set(QPalette::Mid, c.border, c.border);
    set(QPalette::Dark, c.divider, c.divider);
    set(QPalette::Shadow, QColor(0, 0, 0, 120), QColor(0, 0, 0, 120));
    set(QPalette::Highlight, c.accent, c.selectionInactive);
    set(QPalette::HighlightedText, c.onAccent, c.textDim);
    p.setColor(QPalette::Inactive, QPalette::Highlight, solidSel);
    p.setColor(QPalette::Inactive, QPalette::HighlightedText, c.text);
    set(QPalette::Link, c.accentText, c.accentText);
    set(QPalette::LinkVisited, c.accentText, c.accentText);
#if QT_VERSION >= QT_VERSION_CHECK(6, 6, 0)
    set(QPalette::Accent, c.accent, c.textFaint);
#endif
    return p;
}

static QString css(const QColor &c)
{
    return QStringLiteral("rgba(%1,%2,%3,%4)").arg(c.red()).arg(c.green()).arg(c.blue()).arg(c.alpha());
}

static QString styleSheet(const Colors &c)
{
    QString s = QStringLiteral(R"(
QMainWindow, QDialog, QMessageBox { background: {canvas}; }

QToolTip {
    background: {elevated}; color: {text}; border: 1px solid {border};
    border-radius: 7px; padding: 5px 8px;
}

QMenuBar { background: {canvas}; color: {text}; border-bottom: 1px solid {divider}; padding: 2px 4px; }
QFrame#notice { background: {accentSoft}; border: none; border-radius: 10px; margin: 0 14px 8px 14px; }
QMenuBar::item { background: transparent; padding: 4px 9px; border-radius: 6px; }
QMenuBar::item:selected { background: {hover}; }
QMenuBar::item:pressed { background: {pressed}; }

QMenu {
    background: {elevated}; color: {text}; border: 1px solid {border};
    border-radius: 10px; padding: 5px;
}
QMenu::item { padding: 6px 28px 6px 10px; border-radius: 6px; background: transparent; }
QMenu::item:selected { background: {accent}; color: {onAccent}; }
QMenu::item:disabled { color: {textFaint}; }
QMenu::item:disabled:selected { background: {hover}; color: {textFaint}; }
QMenu::separator { height: 1px; background: {divider}; margin: 5px 6px; }
QMenu::icon { padding-left: 8px; }

QLineEdit, QAbstractSpinBox {
    background: {field}; color: {text}; border: 1px solid {border};
    border-radius: 8px; padding: 5px 8px;
    selection-background-color: {accent}; selection-color: {onAccent};
}
QLineEdit:hover, QAbstractSpinBox:hover { background: {fieldHover}; }
QLineEdit:focus, QAbstractSpinBox:focus { border: 1px solid {accent}; background: {field}; }
QLineEdit:disabled { color: {textFaint}; }
QLineEdit#searchEdit {
    background: transparent; border: none; padding: 0; margin: 0;
    selection-background-color: {accentSoftStrong}; selection-color: {text};
}

QPushButton {
    background: {field}; color: {text}; border: 1px solid {border};
    border-radius: 8px; padding: 6px 16px; min-height: 18px;
}
QPushButton:hover { background: {fieldHover}; }
QPushButton:pressed { background: {pressed}; }
QPushButton:focus { border: 1px solid {accent}; }
QPushButton:default { background: {accent}; color: {onAccent}; border: 1px solid {accent}; }
QPushButton:default:hover { background: {accentHover}; }
QPushButton:disabled { color: {textFaint}; background: {field}; border: 1px solid {divider}; }
QPushButton:checked { background: {accentSoft}; color: {accentText}; border: 1px solid {accentSoft}; }

QComboBox {
    background: {field}; color: {text}; border: 1px solid {border};
    border-radius: 8px; padding: 5px 10px; min-width: 120px;
}
QComboBox:hover { background: {fieldHover}; }
QComboBox:focus { border: 1px solid {accent}; }
QComboBox::drop-down { border: none; width: 26px; }
QComboBox QAbstractItemView {
    background: {elevated}; color: {text}; border: 1px solid {border};
    padding: 4px; outline: 0; selection-background-color: {accent}; selection-color: {onAccent};
}

QCheckBox, QRadioButton { spacing: 9px; color: {text}; padding: 2px 0; }
QCheckBox:disabled, QRadioButton:disabled { color: {textFaint}; }

QGroupBox {
    border: 1px solid {divider}; border-radius: 10px; background: {surface};
    margin-top: 26px; padding: 12px 12px 10px 12px;
}
QGroupBox::title {
    subcontrol-origin: margin; subcontrol-position: top left;
    left: 2px; top: 4px; padding: 0 2px; color: {textDim}; font-weight: 600;
}

QTreeView, QListView {
    background: {canvas}; color: {text}; border: 1px solid {divider};
    border-radius: 8px; outline: 0; padding: 3px;
}
QTreeView::item, QListView::item { padding: 5px 4px; border: none; }
QTreeView::item:hover, QListView::item:hover { background: {hover}; }
QTreeView::item:selected, QListView::item:selected { background: {selection}; color: {text}; }
QTreeView::item:selected:!active, QListView::item:selected:!active { background: {selectionInactive}; }
QTreeView#pageList { background: transparent; border: none; padding: 0; }
QTreeView#pageList::item { padding: 7px 10px; border-radius: 7px; margin: 1px 0; }
QTreeView#pageList::item:selected { background: {accentSoft}; color: {accentText}; }

QHeaderView { background: transparent; border: none; }
QHeaderView::section {
    background: transparent; color: {textFaint}; border: none;
    border-bottom: 1px solid {divider}; padding: 5px 6px; font-weight: 600;
}

QScrollBar:vertical { background: transparent; width: 12px; margin: 2px 1px 2px 1px; }
QScrollBar:horizontal { background: transparent; height: 12px; margin: 1px 2px 1px 2px; }
QScrollBar::handle:vertical { background: {scroll}; border-radius: 4px; min-height: 36px; margin: 0 2px; }
QScrollBar::handle:horizontal { background: {scroll}; border-radius: 4px; min-width: 36px; margin: 2px 0; }
QScrollBar::handle:hover { background: {scrollHover}; }
QScrollBar::add-line, QScrollBar::sub-line { width: 0; height: 0; border: none; background: none; }
QScrollBar::add-page, QScrollBar::sub-page { background: none; }
QAbstractScrollArea::corner { background: transparent; }

QStatusBar { background: {canvas}; color: {textDim}; border-top: 1px solid {divider}; min-height: 30px; }
QStatusBar::item { border: none; }
QStatusBar QLabel { color: {textDim}; padding: 0 6px; }
QLabel#queryTime { color: {textFaint}; }

QToolButton { background: transparent; border: none; border-radius: 7px; color: {textDim}; padding: 3px; }
QToolButton:hover { background: {hover}; color: {text}; }
QToolButton:pressed { background: {pressed}; }
QToolButton::menu-indicator { image: none; width: 0; }
QToolButton#searchToggle:checked { background: {accentSoft}; }
QToolButton#searchToggle:checked:hover { background: {accentSoftStrong}; }
QToolButton#chip {
    border: 1px solid {border}; border-radius: 14px; padding: 4px 12px; color: {textDim};
}
QToolButton#chip:hover { background: {hover}; color: {text}; }
QToolButton#chip:checked { background: {accent}; border: 1px solid {accent}; color: {onAccent}; }

QScrollArea, QScrollArea > QWidget > QWidget#syntaxBody { background: transparent; border: none; }
QLabel#hint { color: {textDim}; }
QLabel#sectionTitle { color: {textFaint}; font-weight: 600; }
QPushButton#example {
    background: {field}; border: 1px solid {divider}; border-radius: 6px; padding: 3px 8px;
    color: {accentText}; text-align: left; font-family: monospace;
}
QPushButton#example:hover { background: {accentSoft}; border: 1px solid {accentSoft}; }
)");
    const QList<QPair<QString, QColor>> vars = {
        {"canvas", c.canvas},
        {"surface", c.surface},
        {"elevated", c.elevated},
        {"fieldHover", c.fieldHover},
        {"field", c.field},
        {"border", c.border},
        {"divider", c.divider},
        {"textDim", c.textDim},
        {"textFaint", c.textFaint},
        {"text", c.text},
        {"hover", c.hover},
        {"pressed", c.pressed},
        {"accentHover", mix(c.accent, c.dark ? QColor(Qt::white) : QColor(Qt::black), 0.1)},
        {"accentSoftStrong", alpha(c.accent, c.accentSoft.alphaF() * 1.6)},
        {"accentSoft", c.accentSoft},
        {"accentText", c.accentText},
        {"accent", c.accent},
        {"onAccent", c.onAccent},
        {"selectionInactive", c.selectionInactive},
        {"selection", c.selection},
        {"scrollHover", alpha(c.text, 0.38)},
        {"scroll", alpha(c.text, 0.2)},
    };
    for (const auto &v : vars)
        s.replace(QLatin1Char('{') + v.first + QLatin1Char('}'), css(v.second));
    return s;
}

// Fusion, with our own check boxes, radio buttons, arrows and rounded popups.
class AppStyle : public QProxyStyle {
public:
    AppStyle() : QProxyStyle(QStyleFactory::create(QStringLiteral("Fusion"))) {}

    void polish(QWidget *w) override
    {
        QProxyStyle::polish(w);
        // Rounded corners on popups need a see-through window behind them.
        if (qobject_cast<QMenu *>(w) || w->inherits("QTipLabel")) {
            w->setAttribute(Qt::WA_TranslucentBackground);
            w->setAttribute(Qt::WA_NoSystemBackground, false);
        }
        if (qobject_cast<QAbstractButton *>(w) || qobject_cast<QComboBox *>(w))
            w->setAttribute(Qt::WA_Hover);
    }
    using QProxyStyle::polish;

    int styleHint(StyleHint hint, const QStyleOption *opt, const QWidget *w, QStyleHintReturn *ret) const override
    {
        switch (hint) {
        case SH_DialogButtonBox_ButtonsHaveIcons: return 0;
        case SH_ComboBox_Popup: return 0;
        case SH_UnderlineShortcut: return 0;
        case SH_ItemView_ShowDecorationSelected: return 1;
        case SH_ToolTipLabel_Opacity: return 255;
        case SH_Menu_SubMenuPopupDelay: return 120;
        default: return QProxyStyle::styleHint(hint, opt, w, ret);
        }
    }

    int pixelMetric(PixelMetric m, const QStyleOption *opt, const QWidget *w) const override
    {
        switch (m) {
        case PM_IndicatorWidth:
        case PM_IndicatorHeight:
        case PM_ExclusiveIndicatorWidth:
        case PM_ExclusiveIndicatorHeight: return 18;
        case PM_SmallIconSize:
        case PM_ButtonIconSize: return 16;
        default: return QProxyStyle::pixelMetric(m, opt, w);
        }
    }

    QIcon standardIcon(StandardPixmap sp, const QStyleOption *opt, const QWidget *w) const override
    {
        if (sp == SP_LineEditClearButton)
            return Icons::icon(Icons::Close, Icons::Faint);
        return QProxyStyle::standardIcon(sp, opt, w);
    }

    void drawPrimitive(PrimitiveElement pe, const QStyleOption *opt, QPainter *p, const QWidget *w) const override
    {
        const Colors &c = g_colors;
        const bool enabled = opt->state & State_Enabled;
        const bool hover = opt->state & State_MouseOver;
        switch (pe) {
        case PE_FrameFocusRect:
            return; // focus is shown by the controls themselves
        case PE_IndicatorCheckBox:
        case PE_IndicatorItemViewItemCheck: {
            const bool on = opt->state & State_On;
            const bool partial = opt->state & State_NoChange;
            const qreal side = qMin(opt->rect.width(), opt->rect.height()) - 1;
            const QRectF r(opt->rect.x() + (opt->rect.width() - side) / 2.0, opt->rect.y() + (opt->rect.height() - side) / 2.0, side, side);
            p->save();
            p->setRenderHint(QPainter::Antialiasing);
            if (on || partial) {
                QColor fill = c.accent;
                if (!enabled)
                    fill.setAlphaF(0.4f);
                p->setPen(Qt::NoPen);
                p->setBrush(fill);
                p->drawRoundedRect(r, 4.5, 4.5);
                QPen pen(c.onAccent, 1.9, Qt::SolidLine, Qt::RoundCap, Qt::RoundJoin);
                p->setPen(pen);
                if (partial) {
                    p->drawLine(QPointF(r.left() + side * 0.28, r.center().y()), QPointF(r.right() - side * 0.28, r.center().y()));
                } else {
                    QPainterPath path;
                    path.moveTo(r.left() + side * 0.24, r.top() + side * 0.52);
                    path.lineTo(r.left() + side * 0.43, r.top() + side * 0.70);
                    path.lineTo(r.left() + side * 0.77, r.top() + side * 0.32);
                    p->setBrush(Qt::NoBrush);
                    p->drawPath(path);
                }
            } else {
                p->setPen(QPen(hover && enabled ? c.accent : c.border, 1.2));
                p->setBrush(c.field);
                p->drawRoundedRect(r.adjusted(0.5, 0.5, -0.5, -0.5), 4.5, 4.5);
            }
            p->restore();
            return;
        }
        case PE_IndicatorRadioButton: {
            const bool on = opt->state & State_On;
            const qreal side = qMin(opt->rect.width(), opt->rect.height()) - 1;
            const QRectF r(opt->rect.x() + (opt->rect.width() - side) / 2.0, opt->rect.y() + (opt->rect.height() - side) / 2.0, side, side);
            p->save();
            p->setRenderHint(QPainter::Antialiasing);
            if (on) {
                QColor fill = c.accent;
                if (!enabled)
                    fill.setAlphaF(0.4f);
                p->setPen(Qt::NoPen);
                p->setBrush(fill);
                p->drawEllipse(r);
                p->setBrush(c.onAccent);
                p->drawEllipse(r.center(), side * 0.2, side * 0.2);
            } else {
                p->setPen(QPen(hover && enabled ? c.accent : c.border, 1.2));
                p->setBrush(c.field);
                p->drawEllipse(r.adjusted(0.5, 0.5, -0.5, -0.5));
            }
            p->restore();
            return;
        }
        case PE_IndicatorMenuCheckMark: {
            if (!(opt->state & State_On))
                return;
            const QColor col = (opt->state & State_Selected) ? c.onAccent : c.text;
            Icons::paint(*p, Icons::Check, QRectF(opt->rect).adjusted(-1, -1, 1, 1), col);
            return;
        }
        case PE_IndicatorArrowDown:
        case PE_IndicatorArrowUp:
        case PE_IndicatorArrowRight:
        case PE_IndicatorArrowLeft: {
            const Icons::Glyph g = pe == PE_IndicatorArrowDown ? Icons::ChevronDown
                                   : pe == PE_IndicatorArrowUp ? Icons::ChevronUp
                                   : pe == PE_IndicatorArrowRight ? Icons::ChevronRight
                                                                  : Icons::ChevronLeft;
            QColor col = (opt->state & State_Selected) && qobject_cast<const QMenu *>(w) ? c.onAccent : c.textDim;
            if (!enabled)
                col = c.textFaint;
            const qreal side = qMin(opt->rect.width(), opt->rect.height()) + 4;
            Icons::paint(*p, g, QRectF(opt->rect.center().x() - side / 2 + 0.5, opt->rect.center().y() - side / 2 + 0.5, side, side), col);
            return;
        }
        default:
            QProxyStyle::drawPrimitive(pe, opt, p, w);
        }
    }
};

static bool systemIsDark()
{
#if QT_VERSION >= QT_VERSION_CHECK(6, 5, 0)
    const Qt::ColorScheme s = QGuiApplication::styleHints()->colorScheme();
    if (s != Qt::ColorScheme::Unknown)
        return s == Qt::ColorScheme::Dark;
#endif
    return g_systemPalette.color(QPalette::Window).lightness() < 128;
}

void ensure()
{
    if (g_ready)
        return;
    g_ready = true;
    // The desktop's colours, taken before we replace them: they tell "System"
    // whether the desktop is dark, and which accent it uses.
    g_systemPalette = QApplication::palette();
    QApplication::setStyle(new AppStyle);
#if QT_VERSION >= QT_VERSION_CHECK(6, 5, 0)
    QObject::connect(QGuiApplication::styleHints(), &QStyleHints::colorSchemeChanged, notifier(), [] {
        if (themeKey() == QLatin1String("system"))
            apply();
    });
#endif
    apply();
}

void apply()
{
    static bool busy = false;
    if (busy)
        return;
    busy = true;
    const QString key = themeKey();
#if QT_VERSION >= QT_VERSION_CHECK(6, 8, 0)
    // Tells Qt (and the dialogs it borrows from the desktop) which way we went.
    QStyleHints *hints = QGuiApplication::styleHints();
    if (key == QLatin1String("system"))
        hints->unsetColorScheme();
    else
        hints->setColorScheme(key == QLatin1String("dark") ? Qt::ColorScheme::Dark : Qt::ColorScheme::Light);
#endif
    const bool dark = key == QLatin1String("dark") || (key == QLatin1String("system") && systemIsDark());
    g_colors = makeColors(dark, accentColor(accentKey()));
    QApplication::setPalette(makePalette(g_colors));
    static QString lastSheet;
    const QString sheet = styleSheet(g_colors);
    if (sheet != lastSheet) {
        lastSheet = sheet;
        qApp->setStyleSheet(sheet);
    }
    busy = false;
    emit static_cast<ThemeNotifier *>(notifier())->changed();
}

} // namespace Theme
