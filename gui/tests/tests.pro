# Drives the real window, off screen, against a real trawlrd.
#   mkdir -p build && cd build && qmake6 ../tests.pro && make && QT_QPA_PLATFORM=offscreen ./tst_gui
QT += widgets network testlib
CONFIG += c++17 testcase no_testcase_installs
TARGET = tst_gui
DEFINES += APP_VERSION=\\\"test\\\"
INCLUDEPATH += ../src
SOURCES += tst_gui.cpp ../src/client.cpp ../src/resultstore.cpp ../src/resultsview.cpp ../src/mainwindow.cpp \
    ../src/instance.cpp ../src/shortcuts.cpp ../src/optionsdialog.cpp \
    ../src/theme.cpp ../src/icons.cpp ../src/widgets.cpp \
    ../src/terminal.cpp ../src/fileops.cpp
HEADERS += ../src/client.h ../src/resultstore.h ../src/resultsview.h ../src/mainwindow.h ../src/rangeset.h \
    ../src/instance.h ../src/shortcuts.h ../src/globalhotkey.h ../src/optionsdialog.h ../src/windowactivation.h \
    ../src/theme.h ../src/icons.h ../src/widgets.h \
    ../src/terminal.h ../src/fileops.h ../src/platform.h ../src/peercheck.h
macx {
    SOURCES += ../src/globalhotkey_mac.cpp
    OBJECTIVE_SOURCES += ../src/windowactivation_mac.mm ../src/platform_mac.mm
    LIBS += -framework AppKit -framework Carbon
    CONFIG -= app_bundle
} else:win32 {
    SOURCES += ../src/globalhotkey_win.cpp ../src/windowactivation_win.cpp ../src/platform_win.cpp
    LIBS += -luser32 -lshell32 -lole32 -ladvapi32
} else {
    QT += dbus
    DEFINES += TRAWLR_DBUS
    SOURCES += ../src/globalhotkey.cpp ../src/windowactivation.cpp ../src/platform_unix.cpp
}
