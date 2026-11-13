# Graphical client for trawlrd.   qmake6 trawlr.pro && make
QT += widgets network dbus
CONFIG += c++17 release warn_on
TEMPLATE = app
TARGET = trawlr-gui
VERSION = 0.1.0
DEFINES += APP_VERSION=\\\"$$VERSION\\\"

SOURCES += src/main.cpp src/client.cpp src/resultstore.cpp src/resultsview.cpp src/mainwindow.cpp \
    src/instance.cpp src/shortcuts.cpp src/globalhotkey.cpp src/optionsdialog.cpp src/windowactivation.cpp \
    src/theme.cpp src/icons.cpp src/widgets.cpp \
    src/terminal.cpp
HEADERS += src/client.h src/resultstore.h src/resultsview.h src/mainwindow.h src/rangeset.h \
    src/instance.h src/shortcuts.h src/globalhotkey.h src/optionsdialog.h src/windowactivation.h \
    src/theme.h src/icons.h src/widgets.h \
    src/terminal.h
RESOURCES += resources.qrc

OBJECTS_DIR = obj
MOC_DIR = moc
RCC_DIR = rcc
