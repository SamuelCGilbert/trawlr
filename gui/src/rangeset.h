// A set of row numbers stored as sorted, disjoint, inclusive ranges, so that
// "select all" over millions of results costs one entry.
#pragma once
#include <QList>
#include <QPair>
#include <QtGlobal>

class RangeSet {
public:
    using Range = QPair<qint64, qint64>;

    bool isEmpty() const { return m_ranges.isEmpty(); }
    void clear() { m_ranges.clear(); }
    const QList<Range> &ranges() const { return m_ranges; }

    qint64 count() const
    {
        qint64 n = 0;
        for (const Range &r : m_ranges)
            n += r.second - r.first + 1;
        return n;
    }

    bool contains(qint64 row) const
    {
        int lo = 0, hi = m_ranges.size();
        while (lo < hi) {
            const int mid = (lo + hi) / 2;
            if (m_ranges[mid].second < row)
                lo = mid + 1;
            else
                hi = mid;
        }
        return lo < m_ranges.size() && m_ranges[lo].first <= row;
    }

    void add(qint64 a, qint64 b)
    {
        if (a > b)
            qSwap(a, b);
        QList<Range> out;
        bool placed = false;
        for (const Range &r : m_ranges) {
            if (r.second + 1 < a) {
                out.append(r);
            } else if (b + 1 < r.first) {
                if (!placed) {
                    out.append({a, b});
                    placed = true;
                }
                out.append(r);
            } else {
                a = qMin(a, r.first);
                b = qMax(b, r.second);
            }
        }
        if (!placed)
            out.append({a, b});
        m_ranges = out;
    }

    void remove(qint64 row)
    {
        QList<Range> out;
        for (const Range &r : m_ranges) {
            if (row < r.first || row > r.second) {
                out.append(r);
                continue;
            }
            if (r.first < row)
                out.append({r.first, row - 1});
            if (row < r.second)
                out.append({row + 1, r.second});
        }
        m_ranges = out;
    }

    void toggle(qint64 row)
    {
        if (contains(row))
            remove(row);
        else
            add(row, row);
    }

    void set(qint64 a, qint64 b)
    {
        m_ranges.clear();
        add(a, b);
    }

    // The first `max` selected rows, in order.
    QList<qint64> rows(int max) const
    {
        QList<qint64> out;
        for (const Range &r : m_ranges)
            for (qint64 i = r.first; i <= r.second; ++i) {
                if (out.size() >= max)
                    return out;
                out.append(i);
            }
        return out;
    }

private:
    QList<Range> m_ranges;
};
