cmake_minimum_required(VERSION 3.18)
project(DinurdoOpenAL LANGUAGES C CXX)
if(POLICY CMP0135)
    cmake_policy(SET CMP0135 NEW)
endif()

# Same upstream release, archive hash and EAX settings as JAEnhanced PR #10.
# The source archive is local so routine builds need no network access.
include(FetchContent)
set(ALSOFT_EAX ON CACHE BOOL "Legacy EAX extensions" FORCE)
set(ALSOFT_UTILS OFF CACHE BOOL "Utilities" FORCE)
set(ALSOFT_EXAMPLES OFF CACHE BOOL "Examples" FORCE)
set(ALSOFT_TESTS OFF CACHE BOOL "Tests" FORCE)
set(ALSOFT_INSTALL OFF CACHE BOOL "Standalone install" FORCE)
set(ALSOFT_INSTALL_CONFIG OFF CACHE BOOL "Configuration install" FORCE)
set(ALSOFT_INSTALL_HRTF_DATA OFF CACHE BOOL "External HRTFs" FORCE)
set(ALSOFT_INSTALL_AMBDEC_PRESETS OFF CACHE BOOL "AmbDec presets" FORCE)
set(ALSOFT_UPDATE_BUILD_VERSION OFF CACHE BOOL "Git version lookup" FORCE)
set(ALSOFT_ENABLE_MODULES OFF CACHE BOOL "C++ modules" FORCE)
set(LIBTYPE SHARED)
FetchContent_Declare(openalsoft
    URL "${CMAKE_CURRENT_SOURCE_DIR}/openal-soft-1.25.2.tar.gz"
    URL_HASH SHA256=fb27e5839aa11f0e5b9d33756965291fad5d6909ab928ea1f796f4a1a6877894)
FetchContent_MakeAvailable(openalsoft)
set_target_properties(OpenAL PROPERTIES OUTPUT_NAME "OpenALSoft.x86_64")
foreach(config RELEASE RELWITHDEBINFO MINSIZEREL DEBUG)
    set_target_properties(OpenAL PROPERTIES
        RUNTIME_OUTPUT_DIRECTORY_${config} "${CMAKE_BINARY_DIR}/../..")
endforeach()
configure_file("${openalsoft_SOURCE_DIR}/COPYING"
    "${CMAKE_BINARY_DIR}/../../licenses/OpenAL-Soft-LGPL-2.0.txt" COPYONLY)

# Match the PR's dynamic MSVC runtime, including C++20 atomic wait support.
# Stage app-local redistributables so users need no separate installer.
set(CMAKE_INSTALL_SYSTEM_RUNTIME_LIBS_SKIP TRUE)
include(InstallRequiredSystemLibraries)
set(required_crt msvcp140.dll msvcp140_atomic_wait.dll vcruntime140.dll vcruntime140_1.dll)
foreach(runtime IN LISTS CMAKE_INSTALL_SYSTEM_RUNTIME_LIBS)
    get_filename_component(filename "${runtime}" NAME)
    string(TOLOWER "${filename}" filename)
    if(filename IN_LIST required_crt)
        configure_file("${runtime}" "${CMAKE_BINARY_DIR}/../../${filename}" COPYONLY)
        list(REMOVE_ITEM required_crt "${filename}")
    endif()
endforeach()
if(required_crt)
    message(FATAL_ERROR "Missing Visual C++ redistributables: ${required_crt}")
endif()
