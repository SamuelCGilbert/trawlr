#include "widgets.h"
#include "icons.h"
#include "theme.h"

#include <QAction>
#include <QEnterEvent>
#include <QHBoxLayout>
#include <QLineEdit>
#include <QPainter>
#include <QPainterPath>
#include <QtMath>

// ---- SearchBox --------------------------------------------------------------

SearchBox::SearchBox(QWidget *parent) : QWidget(parent)
{
    setAttribute(Qt::WA_Hover);
    setCursor(Qt::IBeamCursor);
    auto *row = new QHBoxLayout(this);
    row->setContentsMargins(44, 0, 8, 0);
    row->setSpacing(2);

    m_edit = new QLineEdit(this);
    m_edit->setObjectName("searchEdit");
    QFont f = m_edit->font();
    f.setPointSizeF(f.pointSizeF() * 1.22);
    m_edit->setFont(f);
    m_edit->setAttribute(Qt::WA_MacShowFocusRect, false);
    m_edit->installEventFilter(this);
    row->addWidget(m_edit, 1);

    m_clear = new QToolButton(this);
    m_clear->setIcon(Icons::icon(Icons::Close, Icons::Faint));
    m_clear->setIconSize(QSize(16, 16));
    m_clear->setFixedSize(28, 28);
    m_clear->setCursor(Qt::ArrowCursor);
    m_clear->setToolTip(tr("Clear search (Esc)"));
    m_clear->setFocusPolicy(Qt::NoFocus);
    m_clear->hide();
    row->addWidget(m_clear);
    connect(m_clear, &QToolButton::clicked, this, [this] {
        m_edit->clear();
        m_edit->setFocus();
    });
    connect(m_edit, &QLineEdit::textChanged, this, [this](const QString &t) { m_clear->setVisible(!t.isEmpty()); });

    auto *sep = new QWidget(this);
    sep->setFixedSize(9, 20);
    row->addWidget(sep);

    m_toggles = new QHBoxLayout;
    m_toggles->setSpacing(2);
    row->addLayout(m_toggles);

    setFixedHeight(46);
    connect(Theme::notifier(), SIGNAL(changed()), this, SLOT(update()));
}

void SearchBox::addToggle(QAction *action, int glyph, const QString &tip)
{
    auto *b = new QToolButton(this);
    b->setObjectName("searchToggle");
    b->setCheckable(true);
    b->setIcon(Icons::icon(Icons::Glyph(glyph), Icons::Dim));
    b->setIconSize(QSize(20, 20));
    b->setFixedSize(32, 30);
    b->setCursor(Qt::ArrowCursor);
    b->setFocusPolicy(Qt::NoFocus);
    auto updateTip = [b, action, tip] {
        const QKeySequence k = action->shortcut();
        b->setToolTip(k.isEmpty() ? tip : QStringLiteral("%1  (%2)").arg(tip, k.toString(QKeySequence::NativeText)));
    };
    updateTip();
    b->setChecked(action->isChecked());
    connect(action, &QAction::changed, b, [b, action, updateTip] {
        b->setChecked(action->isChecked());
        updateTip();
    });
    connect(action, &QAction::toggled, b, &QToolButton::setChecked);
    connect(b, &QToolButton::clicked, action, [this, action](bool on) {
        action->setChecked(on);
        m_edit->setFocus();
    });
    m_toggles->addWidget(b);
}

void SearchBox::mousePressEvent(QMouseEvent *e)
{
    m_edit->setFocus();
    QWidget::mousePressEvent(e);
}

bool SearchBox::eventFilter(QObject *o, QEvent *e)
{
    if (o == m_edit && (e->type() == QEvent::FocusIn || e->type() == QEvent::FocusOut))
        update();
    return QWidget::eventFilter(o, e);
}

void SearchBox::paintEvent(QPaintEvent *)
{
    const Theme::Colors &c = Theme::colors();
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing);
    const bool focus = m_edit->hasFocus();
    const QRectF r = QRectF(rect()).adjusted(3.5, 3.5, -3.5, -3.5);
    if (focus) {
        // A soft ring outside the border, as browsers draw for focus.
        QColor ring = c.accent;
        ring.setAlphaF(c.dark ? 0.28f : 0.2f);
        p.setPen(QPen(ring, 4));
        p.setBrush(Qt::NoBrush);
        p.drawRoundedRect(r.adjusted(-1, -1, 1, 1), 12, 12);
    }
    p.setPen(QPen(focus ? c.accent : (underMouse() ? c.textFaint : c.border), 1));
    p.setBrush(underMouse() && !focus ? c.fieldHover : c.field);
    p.drawRoundedRect(r, 11, 11);
    Icons::paint(p, Icons::Search, QRectF(16, (height() - 20) / 2.0, 20, 20), focus ? c.accentText : c.textDim);
}

// ---- BusyBar ----------------------------------------------------------------

BusyBar::BusyBar(QWidget *parent) : QWidget(parent)
{
    setFixedHeight(2);
    setAttribute(Qt::WA_TransparentForMouseEvents);
    m_delay.setSingleShot(true);
    m_delay.setInterval(180);
    connect(&m_delay, &QTimer::timeout, this, [this] {
        m_shown = true;
        m_clock.start();
        m_frame.start();
    });
    m_frame.setInterval(16);
    connect(&m_frame, &QTimer::timeout, this, qOverload<>(&QWidget::update));
}

void BusyBar::setBusy(bool busy)
{
    if (busy == m_busy)
        return;
    m_busy = busy;
    if (busy) {
        m_delay.start();
    } else {
        m_delay.stop();
        m_frame.stop();
        m_shown = false;
        update();
    }
}

void BusyBar::paintEvent(QPaintEvent *)
{
    if (!m_shown)
        return;
    const Theme::Colors &c = Theme::colors();
    QPainter p(this);
    const qreal t = std::fmod(m_clock.elapsed() / 1100.0, 1.0);
    const qreal w = width() * 0.3;
    const qreal x = -w + (width() + w) * (0.5 - 0.5 * std::cos(t * M_PI));
    QLinearGradient g(x, 0, x + w, 0);
    QColor edge = c.accent;
    edge.setAlpha(0);
    g.setColorAt(0, edge);
    g.setColorAt(0.5, c.accent);
    g.setColorAt(1, edge);
    p.fillRect(QRectF(x, 0, w, height()), g);
}

// ---- StatusPill -------------------------------------------------------------

StatusPill::StatusPill(QWidget *parent) : QWidget(parent)
{
    setCursor(Qt::PointingHandCursor);
    setAttribute(Qt::WA_Hover);
    m_pulse.setInterval(50);
    connect(&m_pulse, &QTimer::timeout, this, qOverload<>(&QWidget::update));
    connect(Theme::notifier(), SIGNAL(changed()), this, SLOT(update()));
}

void StatusPill::setState(State s, const QString &text, const QString &tip)
{
    m_state = s;
    if (m_text != text) {
        m_text = text;
        updateGeometry();
    }
    setToolTip(tip);
    if (s == Working && !m_pulse.isActive()) {
        m_clock.start();
        m_pulse.start();
    } else if (s != Working) {
        m_pulse.stop();
    }
    update();
}

QSize StatusPill::sizeHint() const
{
    return QSize(fontMetrics().horizontalAdvance(m_text) + 30, 22);
}

void StatusPill::paintEvent(QPaintEvent *)
{
    const Theme::Colors &c = Theme::colors();
    QPainter p(this);
    p.setRenderHint(QPainter::Antialiasing);
    const QRectF r = QRectF(rect()).adjusted(0.5, (height() - 22) / 2.0 + 0.5, -0.5, -(height() - 22) / 2.0 - 0.5);
    if (m_hover) {
        p.setPen(Qt::NoPen);
        p.setBrush(c.hover);
        p.drawRoundedRect(r, r.height() / 2, r.height() / 2);
    }
    QColor dot = m_state == Live ? c.success : m_state == Working ? c.warning : c.danger;
    const QPointF centre(r.left() + 12, r.center().y());
    if (m_state == Working) {
        const qreal phase = 0.5 + 0.5 * std::sin(m_clock.elapsed() / 260.0);
        QColor halo = dot;
        halo.setAlphaF(float(0.35 * phase));
        p.setPen(Qt::NoPen);
        p.setBrush(halo);
        p.drawEllipse(centre, 3.5 + 2.5 * phase, 3.5 + 2.5 * phase);
    } else if (m_state == Live) {
        QColor halo = dot;
        halo.setAlphaF(0.22f);
        p.setPen(Qt::NoPen);
        p.setBrush(halo);
        p.drawEllipse(centre, 6, 6);
    }
    p.setPen(Qt::NoPen);
    p.setBrush(dot);
    p.drawEllipse(centre, 3.5, 3.5);
    p.setPen(m_hover ? c.text : c.textDim);
    p.drawText(r.adjusted(22, 0, -8, 0), Qt::AlignVCenter | Qt::AlignLeft, m_text);
}

void StatusPill::mouseReleaseEvent(QMouseEvent *)
{
    emit clicked();
}

void StatusPill::enterEvent(QEnterEvent *)
{
    m_hover = true;
    update();
}

void StatusPill::leaveEvent(QEvent *)
{
    m_hover = false;
    update();
}
