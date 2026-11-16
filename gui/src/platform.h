// What differs between Linux and Windows outside the window itself: where
// the index service listens, how paths are written, the file manager.
#pragma once
#include <QChar>
#include <QObject>
#include <QString>
#include <QStringList>

namespace Platform {

// Where trawlrd listens: a socket path, or a pipe name on Windows.
QString daemonSocket();

// What joins the parts of a path the service sends: '/', or '\' on Windows.
QChar separator();

// The command that starts the service, for messages.
QString startServiceHint();

// Show the items selected in their folders in the file manager. Opens
// `fallbackDir` if the file manager cannot be asked.
void showInFileManager(QObject *ctx, const QStringList &paths, const QString &fallbackDir);

// Open the file manager's properties dialog for the items.
void showProperties(QObject *ctx, const QStringList &paths);

// Let the program this launch is handing over to take the foreground.
// Windows only lets the program the user is working with bring a window to
// the front, and passes that right on only when asked to.
void allowForeground();

// Whether Trawlr starts (in the tray) when the user signs in. Only Windows
// has this here; elsewhere the desktop's own autostart settings do it.
bool hasStartAtLogin();
bool startsAtLogin();
void setStartAtLogin(bool on);

} // namespace Platform
