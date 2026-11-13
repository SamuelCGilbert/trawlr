# Drives the real window, off screen, against a real trawlrd.
#   mkdir -p build && cd build && qmake6 ../tests.pro && make && QT_QPA_PLATFORM=offscreen ./tst_gui
QT += widgets network dbus testlib
CONFIG += c++17 testcase no_testcase_installs
TARGET = tst_gui
DEFINES += APP_VERSION=\\\"test\\\"
INCLUDEPATH += ../src
SOURCES += tst_gui.cpp ../src/client.cpp ../src/resultstore.cpp ../src/resultsview.cpp ../src/mainwindow.cpp \
    ../src/instance.cpp ../src/shortcuts.cpp ../src/globalhotkey.cpp ../src/optionsdialog.cpp ../src/windowactivation.cpp \
    ../src/theme.cpp ../src/icons.cpp ../src/widgets.cpp \
    ../src/terminal.cpp
HEADERS += ../src/client.h ../src/resultstore.h ../src/resultsview.h ../src/mainwindow.h ../src/rangeset.h \
    ../src/instance.h ../src/shortcuts.h ../src/globalhotkey.h ../src/optionsdialog.h ../src/windowactivation.h \
    ../src/theme.h ../src/icons.h ../src/widgets.h \
    ../src/terminal.h
