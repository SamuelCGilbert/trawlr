// Changes to files on disk that the window makes itself.
#pragma once
#include <QString>

namespace FileOps {

// Why `name` cannot be the name of a file here, or an empty string if it can.
QString invalidNameReason(const QString &name);

// Renames the item at `from` to `to` in the same folder. Never replaces an
// item that is already at `to`; a change of case only (report.pdf to
// Report.pdf) works on file systems that ignore case too. On failure,
// `error` says why.
bool rename(const QString &from, const QString &to, QString *error);

} // namespace FileOps
