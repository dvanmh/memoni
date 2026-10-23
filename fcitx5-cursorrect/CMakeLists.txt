cmake_minimum_required(VERSION 3.6.0)
project(fcitx5-cursorrect LANGUAGES CXX)

if(CMAKE_INSTALL_PREFIX_INITIALIZED_TO_DEFAULT)
    set(CMAKE_INSTALL_PREFIX "/usr" CACHE PATH "" FORCE)
endif()

find_package(Fcitx5Core REQUIRED)
find_package(Fcitx5Module REQUIRED COMPONENTS DBus)

add_library(cursorrect MODULE cursorrect.cpp)
target_link_libraries(cursorrect Fcitx5::Core Fcitx5::Module::DBus)

install(TARGETS cursorrect DESTINATION "${FCITX_INSTALL_ADDONDIR}")
install(FILES cursorrect.conf DESTINATION "${FCITX_INSTALL_PKGDATADIR}/addon")
