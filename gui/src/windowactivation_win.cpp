#include "windowactivation.h"

// Windows has no KWin to ask. Bringing a window forward works there when the
// launching program allows it first (Platform::allowForeground) or the key
// press came to this program (a global hotkey).
bool WindowActivation::viaKWin(const QString &)
{
    return false;
}
