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
