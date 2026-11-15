idf_component_register(
    SRCS "platform.c" "bridge.c" "client.c" "link.c" "radio_compat.c" "radio_prepared.c"
    INCLUDE_DIRS "."
    PRIV_REQUIRES esp_wifi esp_event esp_timer nvs_flash esp_driver_uart)
# radio_compat.c intercepts these two private SDK routines.
target_link_libraries(${COMPONENT_LIB} INTERFACE
    "-Wl,--wrap=ppTxPkt" "-Wl,--wrap=hal_mac_tx_config_edca")
if(PICTO_DIAGNOSTICS)
    target_compile_definitions(${COMPONENT_LIB} PRIVATE PICTO_DIAGNOSTICS=1)
endif()

# Reported to the host in HELLO so a firmware/server mismatch can name the build.
# Captured when CMake configures; run `idf.py reconfigure` to refresh it.
execute_process(COMMAND git describe --always --dirty
    WORKING_DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    OUTPUT_VARIABLE picto_build OUTPUT_STRIP_TRAILING_WHITESPACE ERROR_QUIET)
if(NOT picto_build)
    set(picto_build "unknown")
endif()
target_compile_definitions(${COMPONENT_LIB} PRIVATE "PICTO_BUILD=\"${picto_build}\"")
