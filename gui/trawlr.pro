# Graphical client for trawlrd.   qmake6 trawlr.pro && make
QT += widgets network
CONFIG += c++17 release warn_on
TEMPLATE = app
TARGET = trawlr-gui
VERSION = 0.1.0
DEFINES += APP_VERSION=\\\"$$VERSION\\\"

SOURCES += src/main.cpp src/client.cpp src/resultstore.cpp src/resultsview.cpp src/mainwindow.cpp \
    src/instance.cpp src/shortcuts.cpp src/optionsdialog.cpp \
    src/theme.cpp src/icons.cpp src/widgets.cpp \
    src/terminal.cpp
HEADERS += src/client.h src/resultstore.h src/resultsview.h src/mainwindow.h src/rangeset.h \
    src/instance.h src/shortcuts.h src/globalhotkey.h src/optionsdialog.h src/windowactivation.h \
    src/theme.h src/icons.h src/widgets.h \
    src/terminal.h
RESOURCES += resources.qrc

# The desktop integration differs per system: KDE over D-Bus on Linux,
# the Windows API on Windows, AppKit and Carbon on macOS.
macx {
    QT += svg
    SOURCES += src/globalhotkey_mac.cpp
    OBJECTIVE_SOURCES += src/windowactivation_mac.mm src/platform_mac.mm
    LIBS += -framework AppKit -framework Carbon
    QMAKE_INFO_PLIST = ../dist/macos/Info.plist
    ICON = ../assets/app-icon.icns
    QMAKE_MACOSX_DEPLOYMENT_TARGET = 12.0
} else:win32 {
    # Draws the SVG window and tray icon (Linux has the icon theme for that).
    QT += svg
    SOURCES += src/globalhotkey_win.cpp src/windowactivation_win.cpp src/platform_win.cpp
    LIBS += -luser32 -lshell32 -lole32 -ladvapi32
    RC_ICONS = ../assets/app-icon.ico
    QMAKE_TARGET_PRODUCT = Trawlr
    QMAKE_TARGET_DESCRIPTION = Trawlr
    QMAKE_TARGET_COPYRIGHT = Sam Gilbert, GPL-3.0-or-later
} else {
    QT += dbus
    DEFINES += TRAWLR_DBUS
    SOURCES += src/globalhotkey.cpp src/windowactivation.cpp src/platform_unix.cpp
}
HEADERS += src/platform.h

OBJECTS_DIR = obj
MOC_DIR = moc
RCC_DIR = rcc
