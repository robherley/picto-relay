"""Expose a narrowly scoped validator hook in a private copy of the pinned blob.

No instructions are patched. The validator's ELF binding and a stock alias change; hot raw-TX
sections are relocated to instruction RAM without changing their bytes. Fail closed for any unrecognized library version.
"""
import hashlib
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from elftools.elf.elffile import ELFFile

def sdk_iram_renames(elf):
    # ESP-IDF's optimized Wi-Fi mappings match archive names. Direct private
    # objects otherwise fall through wifi_default and move this code to flash.
    # This dedicated build enables IRAM, RX IRAM and EXTRA IRAM optimization.
    prefixes = (".wifi0iram", ".wifirxiram", ".wifislprxiram", ".wifiorslpiram", ".wifiextrairam")
    return {section.name: ".iram1.picto" + section.name for section in elf.iter_sections()
            if section.name.startswith(prefixes)}

source, destination, ar, objcopy = sys.argv[1:]
expected = "7c1ddfef0d84707614efd4aa9fae42f05c841b25cfecc2324f97e126d607cae2"
if hashlib.sha256(Path(source).read_bytes()).hexdigest() != expected:
    raise SystemExit("Unrecognized ESP32 libnet80211.a; refusing to modify symbol bindings")
with tempfile.TemporaryDirectory(prefix="picto-radio-") as temporary:
    member = Path(temporary) / "ieee80211_output.o"
    member.write_bytes(subprocess.check_output([ar, "p", source, member.name]))
    with member.open("rb") as handle:
        elf = ELFFile(handle)
        symbol = elf.get_section_by_name(".symtab").get_symbol_by_name("ieee80211_raw_frame_sanity_check")[0]
        section = elf.get_section(symbol["st_shndx"])
        if section.name != ".text.ieee80211_raw_frame_sanity_check":
            raise SystemExit("Unexpected validator section")
        # Xtensa text sections can start with literal pools before the entry.
        entry = symbol["st_value"]
        post = elf.get_section_by_name(".symtab").get_symbol_by_name("ieee80211_post_hmac_tx")[0]
        post_section = elf.get_section(post["st_shndx"]).name
        if post_section != ".wifiextrairam.2":
            raise SystemExit("Unexpected raw submission section")
        post_entry = post["st_value"]
        code = {s.name: s.data() for s in elf.iter_sections() if s["sh_flags"] & 4}
        sdk_sections = sdk_iram_renames(elf)
    hot = ["esp_wifi_80211_tx", "ieee80211_raw_frame_sanity_check", "ieee80211_output_raw_process"]
    renames = {**sdk_sections, **{f".text.{name}": f".iram1.picto_{name}" for name in hot}}
    subprocess.run([objcopy, "--weaken-symbol=ieee80211_raw_frame_sanity_check",
                    "--weaken-symbol=ieee80211_post_hmac_tx",
                    f"--add-symbol=picto_stock_post_hmac_tx={renames[post_section]}:{post_entry:#x},global,function",
                    f"--add-symbol=picto_stock_sanity=.iram1.picto_ieee80211_raw_frame_sanity_check:{entry:#x},global,function",
                    *[f"--rename-section={old}={new}" for old, new in renames.items()],
                    str(member)], check=True)
    with member.open("rb") as handle:
        elf = ELFFile(handle)
        if {renames.get(name, name): data for name, data in code.items()} != {
            s.name: s.data() for s in elf.iter_sections() if s["sh_flags"] & 4
        }:
            raise SystemExit("Executable section bytes changed unexpectedly")
        alias = elf.get_section_by_name(".symtab").get_symbol_by_name("picto_stock_sanity")[0]
        if alias["st_value"] != entry:
            raise SystemExit("Original validator alias has wrong entry point")
        alias = elf.get_section_by_name(".symtab").get_symbol_by_name("picto_stock_post_hmac_tx")[0]
        if alias["st_value"] != post_entry:
            raise SystemExit("Original raw submission alias has wrong entry point")
    shutil.copyfile(member, destination)

# Remaining direct helpers on the measured raw-send path. Supply complete private
# objects before archive extraction, so each SDK object's state still exists once.
pp = Path(source).with_name("libpp.a")
if hashlib.sha256(pp.read_bytes()).hexdigest() != "d98fdd39e21e446414470b73e440d5cd710ca5dc4d8a48f9305c9c30986534b4":
    raise SystemExit("Unrecognized ESP32 libpp.a; refusing hot-helper relocation")
helpers = [
    (source, "wl_chm.o", ["chm_get_current_channel", "chm_get_band_from_chan"]),
    (source, "ieee80211_ioctl.o", ["wifi_get_macaddr", "config_is_cache_tx_buf_enabled"]),
    (pp, "if_hwctrl.o", ["ic_ebuf_alloc", "ic_get_default_sched", "ic_get_80211_tx_rate"]),
    (pp, "esf_buf.o", ["esf_buf_alloc_dynamic"]),
    (pp, "hal_mac.o", ["hal_now", "hal_mac_tx_set_ppdu", "mac_tx_set_plcp0", "mac_tx_set_duration"]),
    (pp, "hal_mac_tx.o", ["mac_tx_set_plcp1", "mac_tx_set_plcp2", "mac_tx_set_htsig"]),
    (pp, "lmac.o", ["lmacIsIdle", "lmacIsLongFrame"]),
    (pp, "wdev.o", ["wDev_isNANPktInValidSlot"]),
]
for archive, member_name, names in helpers:
    target = Path(destination).with_name("picto_hot_" + member_name)
    target.write_bytes(subprocess.check_output([ar, "p", str(archive), member_name]))
    with target.open("rb") as handle:
        elf = ELFFile(handle)
        symbols = elf.get_section_by_name(".symtab")
        renames = sdk_iram_renames(elf)
        for name in names:
            symbol = symbols.get_symbol_by_name(name)[0]
            section = elf.get_section(symbol["st_shndx"])
            expected_section = ".wifiextrairam.4" if name == "esf_buf_alloc_dynamic" else f".text.{name}"
            if section.name != expected_section:
                raise SystemExit(f"Unexpected hot-helper section for {name}")
            renames[section.name] = f".iram1.picto_{name}"
        code = {s.name: s.data() for s in elf.iter_sections() if s["sh_flags"] & 4}
    subprocess.run([objcopy, *[f"--rename-section={old}={new}" for old,new in renames.items()], str(target)], check=True)
    with target.open("rb") as handle:
        elf = ELFFile(handle)
        if {renames.get(name,name):data for name,data in code.items()} != {
            s.name:s.data() for s in elf.iter_sections() if s["sh_flags"] & 4
        }:
            raise SystemExit(f"Executable bytes changed in {member_name}")
