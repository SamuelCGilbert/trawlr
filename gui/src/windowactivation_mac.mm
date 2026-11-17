#include "windowactivation.h"

#import <AppKit/AppKit.h>

// macOS has no KWin to ask: an application brings itself forward.
bool WindowActivation::viaKWin(const QString &)
{
    return false;
}

void WindowActivation::activateApp()
{
    if (@available(macOS 14.0, *))
        [NSApp activate];
    else
        [NSApp activateIgnoringOtherApps:YES];
}

void WindowActivation::setDockIcon(bool shown)
{
    const NSApplicationActivationPolicy want = shown ? NSApplicationActivationPolicyRegular : NSApplicationActivationPolicyAccessory;
    if (NSApp.activationPolicy != want)
        [NSApp setActivationPolicy:want];
}
