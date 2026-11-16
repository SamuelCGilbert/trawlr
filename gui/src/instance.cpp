#include "instance.h"

#include <QDir>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLocalSocket>
#include <QStandardPaths>

#ifdef Q_OS_WIN
#include "platform.h"
#else
#include <unistd.h>
#endif

QString Instance::defaultPath()
{
    const QString over = qEnvironmentVariable("TRAWLR_INSTANCE_SOCKET");
    if (!over.isEmpty())
        return over;
#ifdef Q_OS_WIN
    // A pipe name, \\.\pipe\trawlr-gui-<user>; only that user may open it.
    return QStringLiteral("trawlr-gui-%1").arg(qEnvironmentVariable("USERNAME"));
#else
    QString dir = QStandardPaths::writableLocation(QStandardPaths::RuntimeLocation);
    if (dir.isEmpty())
        dir = QDir::tempPath();
    return dir + QStringLiteral("/trawlr-%1.sock").arg(getuid());
#endif
}

QString Instance::lockPath(const QString &path)
{
#ifdef Q_OS_WIN
    // A pipe has no place in the file system; the user's temp folder does.
    return QDir::temp().filePath(path + QStringLiteral(".lock"));
#else
    return path + QStringLiteral(".lock");
#endif
}

Instance::Result Instance::forward(const QString &path, const LaunchRequest &r, int timeoutMs)
{
    QLocalSocket sock;
    sock.connectToServer(path);
    if (!sock.waitForConnected(500))
        return NoInstance; // no socket, or a stale one nobody listens on
#ifdef Q_OS_WIN
    Platform::allowForeground();
#endif
    QJsonObject o;
    o.insert(QStringLiteral("search"), r.search);
    o.insert(QStringLiteral("new"), r.newWindow);
    o.insert(QStringLiteral("toggle"), r.toggle);
    o.insert(QStringLiteral("token"), r.activationToken);
    sock.write(QJsonDocument(o).toJson(QJsonDocument::Compact) + '\n');
    sock.flush();
    // The answer is one line; the other side closes after sending it.
    QByteArray reply;
    while (!reply.contains('\n') && sock.state() == QLocalSocket::ConnectedState) {
        if (!sock.waitForReadyRead(timeoutMs))
            break;
        reply += sock.readAll();
    }
    reply += sock.readAll();
    return reply.startsWith("ok") ? Handled : Unresponsive;
}

bool Instance::listen(const QString &path)
{
    m_server.setSocketOptions(QLocalServer::UserAccessOption);
    if (!m_server.listen(path)) {
        // Either a file left over from a crash, or another launch that won the
        // race to be the first. Only a file nobody answers on may be removed.
        QLocalSocket probe;
        probe.connectToServer(path);
        if (probe.waitForConnected(500))
            return false;
        QLocalServer::removeServer(path);
        if (!m_server.listen(path))
            return false;
    }
    connect(&m_server, &QLocalServer::newConnection, this, &Instance::accept);
    return true;
}

void Instance::accept()
{
    while (QLocalSocket *sock = m_server.nextPendingConnection()) {
        connect(sock, &QLocalSocket::disconnected, sock, &QObject::deleteLater);
        auto *buffer = new QByteArray;
        connect(sock, &QObject::destroyed, [buffer] { delete buffer; });
        connect(sock, &QLocalSocket::readyRead, this, [this, sock, buffer] {
            buffer->append(sock->readAll());
            const int nl = buffer->indexOf('\n');
            if (nl < 0 && buffer->size() < 65536)
                return; // wait for the rest of the line
            const QJsonObject o = QJsonDocument::fromJson(buffer->left(nl < 0 ? buffer->size() : nl)).object();
            buffer->clear();
            LaunchRequest r;
            r.search = o.value(QStringLiteral("search")).toString();
            r.newWindow = o.value(QStringLiteral("new")).toBool();
            r.toggle = o.value(QStringLiteral("toggle")).toBool();
            r.activationToken = o.value(QStringLiteral("token")).toString();
            sock->write("ok\n");
            sock->disconnectFromServer();
            emit requested(r);
        });
    }
}
