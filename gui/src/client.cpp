#include "client.h"

#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>

static QString defaultSocket()
{
    const QByteArray env = qgetenv("TRAWLRD_SOCKET");
    if (!env.isEmpty())
        return QString::fromLocal8Bit(env);
    const QString system = QStringLiteral("/run/trawlrd/trawlrd.sock");
    if (QFileInfo::exists(system))
        return system;
    const QByteArray runtime = qgetenv("XDG_RUNTIME_DIR");
    if (!runtime.isEmpty())
        return QString::fromLocal8Bit(runtime) + QStringLiteral("/trawlrd/trawlrd.sock");
    return system;
}

Client::Client(QObject *parent) : QObject(parent)
{
    connect(&m_socket, &QLocalSocket::readyRead, this, &Client::onReadyRead);
    connect(&m_socket, &QLocalSocket::connected, this, [this] {
        m_retry.stop();
        m_buffer.clear();
        emit connectedChanged(true);
    });
    connect(&m_socket, &QLocalSocket::disconnected, this, [this] {
        emit connectedChanged(false);
        m_retry.start();
    });
    connect(&m_socket, &QLocalSocket::errorOccurred, this, [this] {
        if (m_socket.state() == QLocalSocket::UnconnectedState)
            m_retry.start();
    });
    m_retry.setInterval(1000);
    connect(&m_retry, &QTimer::timeout, this, &Client::tryConnect);
}

Client::~Client()
{
    // The socket reports "disconnected" while it is being torn down. By then
    // the retry timer, a member destroyed before it, no longer exists, so
    // stop listening to the socket first.
    m_socket.disconnect(this);
    m_socket.abort();
}

void Client::start()
{
    tryConnect();
}

void Client::tryConnect()
{
    if (m_socket.state() != QLocalSocket::UnconnectedState)
        return;
    // Looked up on every attempt: the service may be installed, or started
    // for the first time, while this window is already open.
    m_path = defaultSocket();
    m_socket.connectToServer(m_path);
}

void Client::send(const QJsonObject &o)
{
    if (!isConnected())
        return;
    m_socket.write(QJsonDocument(o).toJson(QJsonDocument::Compact) + '\n');
}

quint64 Client::sendQuery(const QueryParams &p, qint64 offset, int limit)
{
    const quint64 qid = m_nextQid++;
    send({{"op", "query"}, {"qid", double(qid)}, {"q", p.text}, {"sort", p.sort}, {"desc", p.desc},
          {"case", p.matchCase}, {"path", p.matchPath}, {"regex", p.regex}, {"word", p.wholeWord},
          {"offset", double(offset)}, {"limit", limit}});
    return qid;
}

void Client::requestRows(quint64 qid, qint64 offset, int limit)
{
    send({{"op", "rows"}, {"qid", double(qid)}, {"offset", double(offset)}, {"limit", limit}});
}

void Client::requestStatus()
{
    send({{"op", "status"}});
}

void Client::requestRescan(bool rebuild)
{
    send({{"op", "rescan"}, {"deep", true}, {"rebuild", rebuild}});
}

static QList<Row> parseRows(const QJsonArray &a)
{
    QList<Row> rows;
    rows.reserve(a.size());
    for (const QJsonValue &v : a) {
        const QJsonObject o = v.toObject();
        Row r;
        r.name = o.value("n").toString();
        r.dir = o.value("p").toString();
        r.size = qint64(o.value("s").toDouble());
        r.mtime = qint64(o.value("m").toDouble());
        r.isDir = o.value("d").toBool();
        r.isLink = o.value("l").toBool();
        r.gone = o.value("gone").toBool();
        rows.append(r);
    }
    return rows;
}

void Client::onReadyRead()
{
    m_buffer += m_socket.readAll();
    int nl;
    while ((nl = m_buffer.indexOf('\n')) >= 0) {
        const QJsonObject o = QJsonDocument::fromJson(m_buffer.left(nl)).object();
        m_buffer.remove(0, nl + 1);
        if (o.contains("event")) {
            emit indexChanged();
        } else if (o.contains("total") || (o.contains("qid") && o.contains("error"))) {
            emit queryReply(quint64(o.value("qid").toDouble()), qint64(o.value("total").toDouble()), qint64(o.value("offset").toDouble()),
                            parseRows(o.value("rows").toArray()), o.value("ms").toDouble(), o.value("error").toString(), o.value("state").toString());
        } else if (o.contains("qid")) {
            emit rowsReply(quint64(o.value("qid").toDouble()), qint64(o.value("offset").toDouble()), parseRows(o.value("rows").toArray()),
                           o.value("stale").toBool());
        } else if (o.contains("files")) {
            emit statusReply(o);
        } else if (o.value("ok").toBool(true) == false) {
            emit refused(o.value("error").toString());
        }
    }
}
