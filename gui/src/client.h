// Connection to trawlrd: one JSON object per line over a Unix socket (a
// named pipe on Windows).
#pragma once
#include "platform.h"

#include <QJsonObject>
#include <QList>
#include <QLocalSocket>
#include <QObject>
#include <QString>
#include <QTimer>

struct Row {
    QString name;
    QString dir;
    qint64 size = 0;
    qint64 mtime = 0;
    bool isDir = false;
    bool isLink = false;
    bool gone = false;

    // A folder at the top ("/", "C:\") already ends in the separator.
    QString path() const { return dir.endsWith(Platform::separator()) ? dir + name : dir + Platform::separator() + name; }
};

struct QueryParams {
    QString text;
    QString sort = QStringLiteral("name");
    bool desc = false;
    bool matchCase = false;
    bool matchPath = false;
    bool regex = false;
    bool wholeWord = false;
    QStringList exclude; // folders left out of the results, with their contents
};

class Client : public QObject {
    Q_OBJECT
public:
    explicit Client(QObject *parent = nullptr);
    ~Client() override;

    void start();
    bool isConnected() const { return m_socket.state() == QLocalSocket::ConnectedState; }
    QString socketPath() const { return m_path; }

    quint64 sendQuery(const QueryParams &p, qint64 offset, int limit);
    void requestRows(quint64 qid, qint64 offset, int limit);
    void requestStatus();
    void requestRescan(bool rebuild);

signals:
    void connectedChanged(bool connected);
    void queryReply(quint64 qid, qint64 total, qint64 offset, const QList<Row> &rows, double ms, const QString &error, const QString &state);
    void rowsReply(quint64 qid, qint64 offset, const QList<Row> &rows, bool stale);
    void statusReply(const QJsonObject &status);
    // The index changed in a way that affects the current query.
    void indexChanged();
    void refused(const QString &message);

private:
    void tryConnect();
    void onReadyRead();
    void send(const QJsonObject &o);

    QLocalSocket m_socket;
    QTimer m_retry;
    QString m_path;
    QByteArray m_buffer;
    quint64 m_nextQid = 1;
};
