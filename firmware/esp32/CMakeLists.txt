cmake_minimum_required(VERSION 3.19)
include($ENV{IDF_PATH}/tools/cmake/project.cmake)
option(PICTO_DIAGNOSTICS "Report radio timing counters to the host (see main/diag.h)" OFF)
project(picto_relay)

set(stock_net80211 "$ENV{IDF_PATH}/components/esp_wifi/lib/esp32/libnet80211.a")
# The compatibility wrapper depends on libpp's private metadata ABI.
set(stock_pp "$ENV{IDF_PATH}/components/esp_wifi/lib/esp32/libpp.a")
file(SHA256 "${stock_pp}" stock_pp_sha256)
if(NOT stock_pp_sha256 STREQUAL "d98fdd39e21e446414470b73e440d5cd710ca5dc4d8a48f9305c9c30986534b4")
    message(FATAL_ERROR "Unsupported ESP32 libpp.a; revalidate the private queue ABI")
endif()
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${stock_pp}")
set(picto_net80211 "${CMAKE_CURRENT_BINARY_DIR}/picto_ieee80211_output.o")
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
    "${CMAKE_CURRENT_LIST_DIR}/prepare_radio.py" "${stock_net80211}")
idf_build_get_property(picto_python PYTHON)
execute_process(COMMAND "${picto_python}" "${CMAKE_CURRENT_LIST_DIR}/prepare_radio.py"
    "${stock_net80211}" "${picto_net80211}" "${CMAKE_AR}" "${CMAKE_OBJCOPY}"
    COMMAND_ERROR_IS_FATAL ANY)
# A direct object supplies the output routines before archive extraction, so
# the matching original archive member is not pulled into the final binary.
target_sources(${PROJECT_NAME}.elf PRIVATE "${picto_net80211}")

foreach(member wl_chm ieee80211_ioctl if_hwctrl esf_buf hal_mac hal_mac_tx lmac wdev)
    target_sources(${PROJECT_NAME}.elf PRIVATE "${CMAKE_CURRENT_BINARY_DIR}/picto_hot_${member}.o")
endforeach()

add_custom_command(TARGET ${PROJECT_NAME}.elf POST_BUILD
    COMMAND "${picto_python}" "${CMAKE_CURRENT_LIST_DIR}/verify_radio_layout.py"
        "$<TARGET_FILE:${PROJECT_NAME}.elf>" "${CMAKE_OBJDUMP}"
    VERBATIM)
set_property(TARGET ${PROJECT_NAME}.elf APPEND PROPERTY LINK_DEPENDS
    "${CMAKE_CURRENT_LIST_DIR}/verify_radio_layout.py")
