cmake_minimum_required(VERSION 3.6.0)
project(fcitx5-unicode-dbus LANGUAGES CXX)

if(CMAKE_INSTALL_PREFIX_INITIALIZED_TO_DEFAULT)
    set(CMAKE_INSTALL_PREFIX "/usr" CACHE PATH "" FORCE)
endif()

find_package(Fcitx5Core REQUIRED)
find_package(Fcitx5Module REQUIRED COMPONENTS DBus Unicode)

add_library(unicode-dbus MODULE unicode-dbus.cpp)
target_link_libraries(unicode-dbus Fcitx5::Core Fcitx5::Module::DBus Fcitx5::Module::Unicode)

install(TARGETS unicode-dbus DESTINATION "${FCITX_INSTALL_ADDONDIR}")
install(FILES unicode-dbus.conf DESTINATION "${FCITX_INSTALL_PKGDATADIR}/addon")
