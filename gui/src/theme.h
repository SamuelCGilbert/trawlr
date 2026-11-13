// The look of the whole program: colours, the application style and style
// sheet, and the light/dark/accent/density settings.
//
// The program draws its own look on top of Qt's Fusion style instead of
// borrowing the desktop's, so it is the same on every desktop and system.
#pragma once
#include <QColor>
#include <QObject>
#include <QString>

namespace Theme {

struct Colors {
    bool dark = true;
    QColor canvas;       // window and list background
    QColor surface;      // dialogs, footer
    QColor elevated;     // menus, tooltips, popups
    QColor field;        // text fields, buttons
    QColor fieldHover;
    QColor border;       // control outlines
    QColor divider;      // hairlines between areas
    QColor text;
    QColor textDim;      // secondary text: paths, sizes, dates
    QColor textFaint;    // tertiary text: column headers, hints
    QColor hover;        // translucent wash under the pointer
    QColor pressed;
    QColor accent;       // solid accent: checked chips, default buttons
    QColor accentText;   // accent used as a text colour on the canvas
    QColor onAccent;     // text on a solid accent background
    QColor accentSoft;   // translucent accent: selection, checked toggles
    QColor selection;    // selected rows while the list has focus
    QColor selectionInactive;
    QColor success, warning, danger;
};

// Installs the style and applies the saved settings. Safe to call again.
void ensure();
// Re-reads the settings and restyles every window.
void apply();
const Colors &colors();
bool compact();

// Saved settings.
QString themeKey();   // "system", "light" or "dark"
QString accentKey();  // "system" or one of accentKeys()
QString densityKey(); // "comfortable" or "compact"
bool showMenuBar();
QStringList accentKeys();
QString accentName(const QString &key);
QColor accentColor(const QString &key);

// Emits changed() after every apply().
QObject *notifier();

} // namespace Theme

class ThemeNotifier : public QObject {
    Q_OBJECT
signals:
    void changed();
};
