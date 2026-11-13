#include "resultstore.h"

ResultStore::ResultStore(Client *client, QObject *parent) : QObject(parent), m_client(client)
{
    connect(client, &Client::connectedChanged, this, [this](bool up) {
        // A new connection knows nothing of the old result set.
        m_inFlight = 0;
        m_queued = false;
        m_qid = 0;
        m_requested.clear();
        if (up) {
            m_inFlightRefresh = false;
            sendNow();
        } else {
            m_pages.clear();
            m_total = 0;
            emit resultsReset();
            emit statsChanged();
        }
        emit busyChanged(busy());
    });

    connect(client, &Client::queryReply, this,
            [this](quint64 qid, qint64 total, qint64 offset, const QList<Row> &rows, double ms, const QString &error, const QString &state) {
                if (qid != m_inFlight)
                    return;
                m_inFlight = 0;
                m_error = error;
                m_state = state;
                if (error.isEmpty()) {
                    const bool refresh = m_inFlightRefresh && m_qid != 0;
                    if (refresh)
                        emit aboutToRefresh();
                    m_qid = qid;
                    m_total = total;
                    m_ms = ms;
                    m_pages.clear();
                    m_requested.clear();
                    store(offset, rows);
                    if (refresh) {
                        emit refreshed();
                    } else {
                        m_collectRows.clear();
                        m_collectDone = nullptr;
                        emit resultsReset();
                    }
                    finishCollect();
                }
                emit statsChanged();
                if (m_queued) {
                    m_queued = false;
                    m_inFlightRefresh = m_queuedRefresh;
                    sendNow();
                }
                emit busyChanged(busy());
            });

    connect(client, &Client::rowsReply, this, [this](quint64 qid, qint64 offset, const QList<Row> &rows, bool stale) {
        if (qid != m_qid)
            return;
        if (stale) {
            // The daemon renumbered its index; our result set is void.
            refresh();
            return;
        }
        store(offset, rows);
        if (!rows.isEmpty())
            emit rowsLoaded(offset, offset + rows.size() - 1);
        finishCollect();
    });
}

void ResultStore::setQuery(const QueryParams &p)
{
    m_params = p;
    if (m_inFlight) {
        // Only the latest search matters; send it when the one in flight returns.
        m_queued = true;
        m_queuedRefresh = false;
        return;
    }
    m_inFlightRefresh = false;
    sendNow();
}

void ResultStore::refresh()
{
    if (m_inFlight) {
        if (!m_queued) {
            m_queued = true;
            m_queuedRefresh = true;
        }
        return;
    }
    m_inFlightRefresh = true;
    sendNow();
}

void ResultStore::sendNow()
{
    if (!m_client->isConnected())
        return;
    qint64 offset = 0;
    int limit = 2 * PageSize;
    if (m_inFlightRefresh) {
        // Fetch the part of the list that is on screen, plus a margin, so the
        // view can keep its place.
        offset = qMax<qint64>(0, m_top / PageSize - 1) * PageSize;
        limit = (m_visible / PageSize + 4) * PageSize;
    }
    m_inFlight = m_client->sendQuery(m_params, offset, limit);
    emit busyChanged(busy());
}

void ResultStore::store(qint64 offset, const QList<Row> &rows)
{
    for (int i = 0; i < rows.size(); i += PageSize) {
        const qint64 page = (offset + i) / PageSize;
        m_pages.insert(page, rows.mid(i, PageSize));
        m_requested.remove(page);
    }
    if (m_pages.size() > 400) {
        // Forget pages far from what is on screen.
        const qint64 here = m_top / PageSize;
        QSet<qint64> keep;
        for (qint64 r : std::as_const(m_collectRows))
            keep.insert(r / PageSize);
        for (auto it = m_pages.begin(); it != m_pages.end();) {
            if (qAbs(it.key() - here) > 100 && !keep.contains(it.key()))
                it = m_pages.erase(it);
            else
                ++it;
        }
    }
}

void ResultStore::requestPage(qint64 page)
{
    if (!m_qid || m_requested.contains(page) || m_pages.contains(page))
        return;
    m_requested.insert(page);
    m_client->requestRows(m_qid, page * PageSize, PageSize);
}

const Row *ResultStore::cached(qint64 r) const
{
    if (r < 0 || r >= m_total)
        return nullptr;
    const auto it = m_pages.constFind(r / PageSize);
    if (it == m_pages.constEnd())
        return nullptr;
    const int i = int(r % PageSize);
    return i < it->size() ? &it->at(i) : nullptr;
}

const Row *ResultStore::row(qint64 r)
{
    const Row *row = cached(r);
    if (!row && r >= 0 && r < m_total)
        requestPage(r / PageSize);
    return row;
}

void ResultStore::setVisibleRange(qint64 top, int count)
{
    m_top = top;
    m_visible = count;
}

void ResultStore::collect(const QList<qint64> &rows, std::function<void(const QList<Row> &)> done)
{
    m_collectRows = rows;
    m_collectDone = std::move(done);
    for (qint64 r : rows)
        if (!cached(r))
            requestPage(r / PageSize);
    finishCollect();
}

void ResultStore::finishCollect()
{
    if (!m_collectDone)
        return;
    QList<Row> out;
    out.reserve(m_collectRows.size());
    for (qint64 r : std::as_const(m_collectRows)) {
        const Row *row = cached(r);
        if (!row) {
            if (r < m_total)
                return; // still loading
            continue;
        }
        out.append(*row);
    }
    auto done = std::move(m_collectDone);
    m_collectDone = nullptr;
    m_collectRows.clear();
    done(out);
}
