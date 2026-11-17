// Global hotkeys on macOS: Carbon's RegisterEventHotKey, which needs no
// accessibility permission and is what most Mac launchers use. The keys
// belong to this program while it runs; they are kept in its settings and
// registered again at every start.
#include "globalhotkey.h"

#include <QCoreApplication>
#include <QSettings>

#include <Carbon/Carbon.h>

namespace GlobalHotkey {

static const char *const kKeys[ActionCount] = {"hotkeys/show", "hotkeys/new", "hotkeys/toggle"};
static const OSType kSignature = 'TrwL';
static std::function<void(Action)> s_handler;
static EventHotKeyRef s_refs[ActionCount] = {};
static bool s_blocked = false;

static OSStatus onHotKey(EventHandlerCallRef, EventRef event, void *)
{
    EventHotKeyID id = {};
    if (GetEventParameter(event, kEventParamDirectObject, typeEventHotKeyID, nullptr, sizeof id, nullptr, &id) != noErr)
        return eventNotHandledErr;
    if (id.signature != kSignature || id.id >= UInt32(ActionCount))
        return eventNotHandledErr;
    if (s_handler)
        s_handler(Action(id.id));
    return noErr;
}

static void installHandler()
{
    static bool done = false;
    if (done)
        return;
    done = true;
    EventTypeSpec spec = {kEventClassKeyboard, kEventHotKeyPressed};
    InstallApplicationEventHandler(NewEventHandlerUPP(onHotKey), 1, &spec, nullptr, nullptr);
}

// Qt's key and modifiers as Carbon wants them. In Qt on the Mac, Control
// means the Command key and Meta the Control key.
static bool toNative(const QKeySequence &seq, UInt32 *mods, UInt32 *code)
{
    if (seq.isEmpty())
        return false;
    const QKeyCombination c = seq[0];
    const Qt::KeyboardModifiers m = c.keyboardModifiers();
    *mods = 0;
    if (m & Qt::ControlModifier)
        *mods |= cmdKey;
    if (m & Qt::ShiftModifier)
        *mods |= shiftKey;
    if (m & Qt::AltModifier)
        *mods |= optionKey;
    if (m & Qt::MetaModifier)
        *mods |= controlKey;
    static const struct {
        int key;
        UInt32 code;
    } table[] = {
        {Qt::Key_A, kVK_ANSI_A}, {Qt::Key_B, kVK_ANSI_B}, {Qt::Key_C, kVK_ANSI_C}, {Qt::Key_D, kVK_ANSI_D}, {Qt::Key_E, kVK_ANSI_E},
        {Qt::Key_F, kVK_ANSI_F}, {Qt::Key_G, kVK_ANSI_G}, {Qt::Key_H, kVK_ANSI_H}, {Qt::Key_I, kVK_ANSI_I}, {Qt::Key_J, kVK_ANSI_J},
        {Qt::Key_K, kVK_ANSI_K}, {Qt::Key_L, kVK_ANSI_L}, {Qt::Key_M, kVK_ANSI_M}, {Qt::Key_N, kVK_ANSI_N}, {Qt::Key_O, kVK_ANSI_O},
        {Qt::Key_P, kVK_ANSI_P}, {Qt::Key_Q, kVK_ANSI_Q}, {Qt::Key_R, kVK_ANSI_R}, {Qt::Key_S, kVK_ANSI_S}, {Qt::Key_T, kVK_ANSI_T},
        {Qt::Key_U, kVK_ANSI_U}, {Qt::Key_V, kVK_ANSI_V}, {Qt::Key_W, kVK_ANSI_W}, {Qt::Key_X, kVK_ANSI_X}, {Qt::Key_Y, kVK_ANSI_Y},
        {Qt::Key_Z, kVK_ANSI_Z}, {Qt::Key_0, kVK_ANSI_0}, {Qt::Key_1, kVK_ANSI_1}, {Qt::Key_2, kVK_ANSI_2}, {Qt::Key_3, kVK_ANSI_3},
        {Qt::Key_4, kVK_ANSI_4}, {Qt::Key_5, kVK_ANSI_5}, {Qt::Key_6, kVK_ANSI_6}, {Qt::Key_7, kVK_ANSI_7}, {Qt::Key_8, kVK_ANSI_8},
        {Qt::Key_9, kVK_ANSI_9}, {Qt::Key_Space, kVK_Space}, {Qt::Key_Return, kVK_Return}, {Qt::Key_Enter, kVK_ANSI_KeypadEnter},
        {Qt::Key_Escape, kVK_Escape}, {Qt::Key_Tab, kVK_Tab}, {Qt::Key_Backspace, kVK_Delete}, {Qt::Key_Delete, kVK_ForwardDelete},
        {Qt::Key_Home, kVK_Home}, {Qt::Key_End, kVK_End}, {Qt::Key_PageUp, kVK_PageUp}, {Qt::Key_PageDown, kVK_PageDown},
        {Qt::Key_Left, kVK_LeftArrow}, {Qt::Key_Right, kVK_RightArrow}, {Qt::Key_Up, kVK_UpArrow}, {Qt::Key_Down, kVK_DownArrow},
        {Qt::Key_F1, kVK_F1}, {Qt::Key_F2, kVK_F2}, {Qt::Key_F3, kVK_F3}, {Qt::Key_F4, kVK_F4}, {Qt::Key_F5, kVK_F5},
        {Qt::Key_F6, kVK_F6}, {Qt::Key_F7, kVK_F7}, {Qt::Key_F8, kVK_F8}, {Qt::Key_F9, kVK_F9}, {Qt::Key_F10, kVK_F10},
        {Qt::Key_F11, kVK_F11}, {Qt::Key_F12, kVK_F12}, {Qt::Key_F13, kVK_F13}, {Qt::Key_F14, kVK_F14}, {Qt::Key_F15, kVK_F15},
        {Qt::Key_F16, kVK_F16}, {Qt::Key_F17, kVK_F17}, {Qt::Key_F18, kVK_F18}, {Qt::Key_F19, kVK_F19}, {Qt::Key_F20, kVK_F20},
        {Qt::Key_Minus, kVK_ANSI_Minus}, {Qt::Key_Equal, kVK_ANSI_Equal}, {Qt::Key_BracketLeft, kVK_ANSI_LeftBracket},
        {Qt::Key_BracketRight, kVK_ANSI_RightBracket}, {Qt::Key_Semicolon, kVK_ANSI_Semicolon}, {Qt::Key_Apostrophe, kVK_ANSI_Quote},
        {Qt::Key_Comma, kVK_ANSI_Comma}, {Qt::Key_Period, kVK_ANSI_Period}, {Qt::Key_Slash, kVK_ANSI_Slash},
        {Qt::Key_Backslash, kVK_ANSI_Backslash}, {Qt::Key_QuoteLeft, kVK_ANSI_Grave},
    };
    for (const auto &e : table) {
        if (e.key == c.key()) {
            *code = e.code;
            return true;
        }
    }
    return false;
}

static bool registerKey(Action a, const QKeySequence &key)
{
    UInt32 mods = 0, code = 0;
    if (!toNative(key, &mods, &code))
        return false;
    installHandler();
    EventHotKeyID id = {kSignature, UInt32(a)};
    EventHotKeyRef ref = nullptr;
    if (RegisterEventHotKey(code, mods, id, GetApplicationEventTarget(), 0, &ref) != noErr)
        return false;
    s_refs[a] = ref;
    return true;
}

static void unregisterKey(Action a)
{
    if (s_refs[a])
        UnregisterEventHotKey(s_refs[a]);
    s_refs[a] = nullptr;
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
    QSettings s;
    unregisterKey(a);
    if (key.isEmpty()) {
        s.remove(QLatin1String(kKeys[a]));
        return true;
    }
    UInt32 mods = 0, code = 0;
    if (!toNative(key, &mods, &code)) {
        if (error)
            *error = QCoreApplication::translate("GlobalHotkey", "%1 cannot be used as a global hotkey.").arg(key.toString(QKeySequence::NativeText));
        return false;
    }
    if (!s_blocked && !registerKey(a, key)) {
        if (error)
            *error = QCoreApplication::translate("GlobalHotkey", "%1 is already used by another program.").arg(key.toString(QKeySequence::NativeText));
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
