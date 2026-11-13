// The current search and a window onto its results.
//
// The daemon keeps the full result list; this holds only the pages of rows
// that have been looked at, fetching more as the view scrolls. That is what
// lets the list show tens of millions of results instantly.
#pragma once
#include "client.h"

#include <QHash>
#include <QSet>
#include <functional>

class ResultStore : public QObject {
    Q_OBJECT
public:
    enum { PageSize = 128 };

    explicit ResultStore(Client *client, QObject *parent = nullptr);

    // Start a new search. The view resets once its first results arrive.
    void setQuery(const QueryParams &p);
    // Run the current search again, keeping the scroll position and selection.
    void refresh();

    const QueryParams &params() const { return m_params; }
    qint64 total() const { return m_total; }
    double lastMs() const { return m_ms; }
    QString error() const { return m_error; }
    QString indexState() const { return m_state; }
    bool hasResults() const { return m_qid != 0; }
    // A search has been sent and its answer is not back yet.
    bool busy() const { return m_inFlight != 0; }

    // The row if it is loaded; otherwise null, and it is requested.
    const Row *row(qint64 r);
    const Row *cached(qint64 r) const;
    void setVisibleRange(qint64 top, int count);

    // Hand `done` the given rows once all of them are loaded.
    void collect(const QList<qint64> &rows, std::function<void(const QList<Row> &)> done);

signals:
    void resultsReset();
    void aboutToRefresh();
    void refreshed();
    void rowsLoaded(qint64 first, qint64 last);
    void statsChanged();
    void busyChanged(bool busy);

private:
    void sendNow();
    void store(qint64 offset, const QList<Row> &rows);
    void requestPage(qint64 page);
    void finishCollect();

    Client *m_client;
    QueryParams m_params;
    QHash<qint64, QList<Row>> m_pages;
    QSet<qint64> m_requested;
    quint64 m_qid = 0;        // result set the daemon currently holds for us
    quint64 m_inFlight = 0;   // query sent, reply not yet seen
    bool m_inFlightRefresh = false;
    bool m_queued = false;    // another query is wanted once the reply arrives
    bool m_queuedRefresh = false;
    qint64 m_total = 0;
    double m_ms = 0;
    QString m_error;
    QString m_state;
    qint64 m_top = 0;
    int m_visible = 50;
    QList<qint64> m_collectRows;
    std::function<void(const QList<Row> &)> m_collectDone;
};
