"""Fail the build if timing-critical routines (SDK or ours) fall back into flash."""
import sys
import subprocess
from elftools.elf.elffile import ELFFile

REQUIRED_IRAM = (
    "esp_wifi_80211_tx", "ieee80211_raw_frame_sanity_check",
    "ieee80211_output_raw_process", "ieee80211_output_process",
    "ieee80211_post_hmac_tx", "ic_interface_enabled", "esf_buf_alloc",
    "esf_buf_alloc_dynamic", "hal_mac_tx_config_edca", "hal_mac_txq_enable",
    "chm_get_current_channel", "chm_get_band_from_chan", "wifi_get_macaddr",
    "config_is_cache_tx_buf_enabled", "ic_ebuf_alloc", "ic_get_default_sched",
    "ic_get_80211_tx_rate", "hal_now", "__wrap_ppTxPkt",
    "hal_mac_tx_set_ppdu", "mac_tx_set_plcp0", "mac_tx_set_plcp1",
    "mac_tx_set_plcp2", "mac_tx_set_duration", "mac_tx_set_htsig",
    "lmacIsIdle", "lmacIsLongFrame", "wDev_isNANPktInValidSlot",
    # Our own poll-to-reply path (PICTO_HOT / IRAM_ATTR in main/).
    "bridge_receive", "bridge_tx_complete", "client_receive", "client_tick",
    "client_tx_complete", "client_transfer_body",
    "picto_platform_tx",
    "picto_radio_try_prepared", "picto_radio_submit_prepared", "picto_radio_flush_reply",
)


def verify(path):
    with open(path, "rb") as handle:
        elf = ELFFile(handle)
        table = elf.get_section_by_name(".symtab")
        for name in REQUIRED_IRAM:
            matches = table.get_symbol_by_name(name) or []
            if len(matches) != 1:
                raise ValueError(f"Expected one linked definition of {name}")
            address = matches[0]["st_value"]
            if not 0x40080000 <= address < 0x400A0000:
                raise ValueError(f"{name} is outside ESP32 instruction RAM: {address:#x}")


if __name__ == "__main__":
    verify(sys.argv[1])
    disassembly = subprocess.check_output([
        sys.argv[2], "-d", "--disassemble=esp_wifi_80211_tx", sys.argv[1]
    ], text=True)
    if not any("call8" in line and "<ieee80211_post_hmac_tx>" in line
               for line in disassembly.splitlines()):
        raise ValueError("Raw API does not call the prepared-buffer interception hook")
    print(f"Verified {len(REQUIRED_IRAM)} timing-critical routines in instruction RAM")
