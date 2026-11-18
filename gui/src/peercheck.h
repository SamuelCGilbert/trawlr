// Who is on the other end of a local socket or pipe, so that the window only
// believes the real trawlrd (or another Trawlr window of the same user), not
// a program another user started at the same address first.
#pragma once
#include <QLocalSocket>
#include <QString>

#ifdef Q_OS_WIN
#include <windows.h>
#else
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>
#endif

namespace PeerCheck {

enum class Want {
    Daemon,     // root or this user (Unix); the services session (Windows)
    SameUser,   // this user, in this session
};

// Empty if the peer is acceptable, else why not.
inline QString check(const QLocalSocket &s, Want want)
{
#ifdef Q_OS_WIN
    const HANDLE pipe = reinterpret_cast<HANDLE>(s.socketDescriptor());
    ULONG session = ~0UL;
    if (!GetNamedPipeServerSessionId(pipe, &session))
        return QStringLiteral("cannot tell who is serving %1").arg(s.serverName());
    DWORD mine = ~0UL;
    ProcessIdToSessionId(GetCurrentProcessId(), &mine);
    const ULONG expected = want == Want::Daemon ? 0 : mine;
    if (session != expected)
        return QStringLiteral("%1 is served from session %2, not by %3; not trusting it")
            .arg(s.serverName()).arg(session).arg(want == Want::Daemon ? QStringLiteral("the Trawlr service") : QStringLiteral("you"));
    return QString();
#else
    uid_t uid = uid_t(-1);
#ifdef Q_OS_MACOS
    gid_t gid;
    if (getpeereid(int(s.socketDescriptor()), &uid, &gid) != 0)
        return QStringLiteral("cannot tell who is answering on %1").arg(s.serverName());
#else
    struct ucred cred = {};
    socklen_t len = sizeof cred;
    if (getsockopt(int(s.socketDescriptor()), SOL_SOCKET, SO_PEERCRED, &cred, &len) != 0)
        return QStringLiteral("cannot tell who is answering on %1").arg(s.serverName());
    uid = cred.uid;
#endif
    if (uid == geteuid() || (want == Want::Daemon && uid == 0))
        return QString();
    return QStringLiteral("the program answering on %1 runs as another user (%2); not trusting it").arg(s.fullServerName()).arg(uid);
#endif
}

} // namespace PeerCheck
