// Global hotkeys on Windows: RegisterHotKey, owned by this program while it
// runs. The keys are kept in the program's settings and registered again
// at every start.
#include "globalhotkey.h"

#include <QAbstractEventDispatcher>
#include <QAbstractNativeEventFilter>
#include <QCoreApplication>
#include <QSettings>

#include <windows.h>

namespace GlobalHotkey {

static const char *const kKeys[ActionCount] = {"hotkeys/show", "hotkeys/new", "hotkeys/toggle"};
static std::function<void(Action)> s_handler;
static bool s_registered[ActionCount] = {};
static bool s_blocked = false;

// Hotkey ids are the action plus one (0 is not a valid id).
static int idOf(Action a)
{
    return int(a) + 1;
}

class Filter : public QAbstractNativeEventFilter {
public:
    bool nativeEventFilter(const QByteArray &type, void *message, qintptr *) override
    {
        // Keys registered without a window arrive as thread messages, which
        // Qt hands over as "windows_dispatcher_MSG".
        if (type != "windows_generic_MSG" && type != "windows_dispatcher_MSG")
            return false;
        const MSG *msg = static_cast<const MSG *>(message);
        if (msg->message != WM_HOTKEY)
            return false;
        const int a = int(msg->wParam) - 1;
        if (a >= 0 && a < ActionCount && s_handler)
            s_handler(Action(a));
        return true;
    }
};

static void installFilter()
{
    static Filter *filter = nullptr;
    if (!filter && QCoreApplication::instance()) {
        filter = new Filter;
        QCoreApplication::instance()->installNativeEventFilter(filter);
    }
}

// Qt's key and modifiers as RegisterHotKey wants them.
static bool toNative(const QKeySequence &seq, UINT *mods, UINT *vk)
{
    if (seq.isEmpty())
        return false;
    const QKeyCombination c = seq[0];
    const Qt::KeyboardModifiers m = c.keyboardModifiers();
    *mods = MOD_NOREPEAT;
    if (m & Qt::ControlModifier)
        *mods |= MOD_CONTROL;
    if (m & Qt::ShiftModifier)
        *mods |= MOD_SHIFT;
    if (m & Qt::AltModifier)
        *mods |= MOD_ALT;
    if (m & Qt::MetaModifier)
        *mods |= MOD_WIN;
    const int k = c.key();
    if ((k >= Qt::Key_A && k <= Qt::Key_Z) || (k >= Qt::Key_0 && k <= Qt::Key_9)) {
        *vk = UINT(k);
        return true;
    }
    if (k >= Qt::Key_F1 && k <= Qt::Key_F24) {
        *vk = VK_F1 + UINT(k - Qt::Key_F1);
        return true;
    }
    switch (k) {
    case Qt::Key_Space: *vk = VK_SPACE; return true;
    case Qt::Key_Return:
    case Qt::Key_Enter: *vk = VK_RETURN; return true;
    case Qt::Key_Escape: *vk = VK_ESCAPE; return true;
    case Qt::Key_Tab: *vk = VK_TAB; return true;
    case Qt::Key_Backspace: *vk = VK_BACK; return true;
    case Qt::Key_Insert: *vk = VK_INSERT; return true;
    case Qt::Key_Delete: *vk = VK_DELETE; return true;
    case Qt::Key_Home: *vk = VK_HOME; return true;
    case Qt::Key_End: *vk = VK_END; return true;
    case Qt::Key_PageUp: *vk = VK_PRIOR; return true;
    case Qt::Key_PageDown: *vk = VK_NEXT; return true;
    case Qt::Key_Left: *vk = VK_LEFT; return true;
    case Qt::Key_Right: *vk = VK_RIGHT; return true;
    case Qt::Key_Up: *vk = VK_UP; return true;
    case Qt::Key_Down: *vk = VK_DOWN; return true;
    case Qt::Key_Pause: *vk = VK_PAUSE; return true;
    case Qt::Key_Print: *vk = VK_SNAPSHOT; return true;
    default: break;
    }
    // Punctuation: whichever key types the character on this keyboard.
    if (k > 0x20 && k < 0x7f) {
        const SHORT scan = VkKeyScanW(WCHAR(QChar(k).toLower().unicode()));
        if (scan != -1) {
            *vk = UINT(scan & 0xff);
            return true;
        }
    }
    return false;
}

static bool registerKey(Action a, const QKeySequence &key)
{
    UINT mods = 0, vk = 0;
    if (!toNative(key, &mods, &vk))
        return false;
    s_registered[a] = RegisterHotKey(nullptr, idOf(a), mods, vk);
    return s_registered[a];
}

static void unregisterKey(Action a)
{
    if (s_registered[a])
        UnregisterHotKey(nullptr, idOf(a));
    s_registered[a] = false;
}

bool available()
{
    return true;
}

bool supports(Action)
{
    return true;
}

QKeySequence current(Action a)
{
    return QKeySequence(QSettings().value(QLatin1String(kKeys[a])).toString(), QKeySequence::PortableText);
}

void migrate()
{
    installFilter();
    for (int a = 0; a < ActionCount; ++a)
        if (!current(Action(a)).isEmpty())
            registerKey(Action(a), current(Action(a)));
}

void block(bool on)
{
    if (on == s_blocked)
        return;
    s_blocked = on;
    for (int a = 0; a < ActionCount; ++a) {
        if (on)
            unregisterKey(Action(a));
        else if (!current(Action(a)).isEmpty())
            registerKey(Action(a), current(Action(a)));
    }
}

bool set(Action a, const QKeySequence &key, QString *error)
{
    installFilter();
    QSettings s;
    unregisterKey(a);
    if (key.isEmpty()) {
        s.remove(QLatin1String(kKeys[a]));
        return true;
    }
    UINT mods = 0, vk = 0;
    if (!toNative(key, &mods, &vk)) {
        if (error)
            *error = QCoreApplication::translate("GlobalHotkey", "%1 cannot be used as a global hotkey.").arg(key.toString(QKeySequence::NativeText));
        return false;
    }
    if (!s_blocked && !registerKey(a, key)) {
        if (error)
            *error = QCoreApplication::translate("GlobalHotkey", "%1 is already used by another program.").arg(key.toString(QKeySequence::NativeText));
        // Put back the key that worked before.
        if (!current(a).isEmpty())
            registerKey(a, current(a));
        return false;
    }
    s.setValue(QLatin1String(kKeys[a]), key.toString(QKeySequence::PortableText));
    return true;
}

void setHandler(std::function<void(Action)> handler)
{
    s_handler = std::move(handler);
}

} // namespace GlobalHotkey
