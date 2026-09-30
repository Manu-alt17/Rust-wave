idf_component_register(
    SRCS "usbdisk.c"
    INCLUDE_DIRS "include"
    PRIV_REQUIRES sdmmc esp_driver_sdmmc fatfs
)

# Nothing in this firmware may ever format the SD card: every call to
# FatFs's f_mkfs lands in usbdisk.c's __wrap_f_mkfs, which refuses. See
# the comment there for the esp_tinyusb behaviour this guards against.
target_link_libraries(${COMPONENT_LIB} INTERFACE "-Wl,--wrap=f_mkfs")
