#pragma once
#include <QLocalServer>
#include <QObject>

// What a second launch of the program says to the one that is already
// running: "show yourself" or "open another window", optionally with a search.
struct LaunchRequest {
    QString search;
    bool newWindow = false;
    // Minimize the window if it is the active one, else bring it forward.
    bool toggle = false;
    // Wayland and X11 only let the program being launched take focus; the
    // desktop hands it a token for that, which the running window needs to
    // be allowed to come to the front.
    QString activationToken;
};

// The running program listens on a per-user socket. Later launches find it,
// pass their request and exit.
class Instance : public QObject {
    Q_OBJECT
public:
    enum Result {
        Handled,      // a running instance took the request; the caller should exit
        NoInstance,   // nobody is listening; the caller is the first
        Unresponsive, // something is listening but did not answer; run on your own
    };

    static QString defaultPath();
    static Result forward(const QString &path, const LaunchRequest &r, int timeoutMs = 3000);

    explicit Instance(QObject *parent = nullptr) : QObject(parent) {}
    // False if another instance is already listening (it won a race to start first).
    bool listen(const QString &path);

signals:
    void requested(const LaunchRequest &r);

private:
    void accept();
    QLocalServer m_server;
};
