#include "usbdisk.h"

#include <stdlib.h>

#include "driver/sdmmc_host.h"
#include "esp_check.h"
#include "esp_log.h"
#include "esp_system.h"
#include "ff.h"
#include "sdmmc_cmd.h"
#include "soc/rtc_cntl_struct.h"
#include "tinyusb.h"
#include "tusb_msc_storage.h"

static const char *TAG = "usbdisk";

/* Never format the card. esp_tinyusb 1.x (tusb_msc_storage.c) remounts the
 * card on the firmware side when the PC ejects the disk
 * (tud_msc_start_stop_cb) or the cable comes out (tud_umount_cb), and when
 * that mount fails with FR_NO_FILESYSTEM or FR_INT_ERR it formats the card
 * on the spot -- no option turns that off in 1.x. A card the PC left in a
 * format FatFs cannot read here (exFAT, which every microSD from 64 GB up
 * ships with), or a FAT damaged by a cable pulled mid-write, would lose the
 * whole library just by being ejected.
 *
 * CMakeLists.txt links with --wrap=f_mkfs, so every f_mkfs call in the
 * firmware lands here and fails instead. Nothing in it formats on purpose. */
FRESULT __wrap_f_mkfs(const TCHAR *path, const MKFS_PARM *opt, void *work, UINT len)
{
    (void) path;
    (void) opt;
    (void) work;
    (void) len;
    ESP_LOGE(TAG, "refusing to format the SD card");
    return FR_DENIED;
}

/* Same bus speed the firmware mounts the card at (SDMMC_STABLE_SPEED_KHZ in
 * src/storage.rs). USB 1.1 Full Speed caps the transfer well below it. */
#define USBDISK_SD_SPEED_KHZ 10000

/* --- USB descriptors ---
 *
 * From ESP-IDF's tusb_msc example. VID/PID are the example's (0x303A is
 * Espressif's vendor ID): fine for a personal device, and inventing new ones
 * risks finding out late that some OS treats them oddly. */

#define EPNUM_MSC 1
#define TUSB_DESC_TOTAL_LEN (TUD_CONFIG_DESC_LEN + TUD_MSC_DESC_LEN)

enum {
    ITF_NUM_MSC = 0,
    ITF_NUM_TOTAL
};

enum {
    EDPT_MSC_OUT = 0x01,
    EDPT_MSC_IN = 0x81,
};

static tusb_desc_device_t s_device_desc = {
    .bLength = sizeof(s_device_desc),
    .bDescriptorType = TUSB_DESC_DEVICE,
    .bcdUSB = 0x0200,
    .bDeviceClass = TUSB_CLASS_MISC,
    .bDeviceSubClass = MISC_SUBCLASS_COMMON,
    .bDeviceProtocol = MISC_PROTOCOL_IAD,
    .bMaxPacketSize0 = CFG_TUD_ENDPOINT0_SIZE,
    .idVendor = 0x303A,
    .idProduct = 0x4002,
    .bcdDevice = 0x100,
    .iManufacturer = 0x01,
    .iProduct = 0x02,
    .iSerialNumber = 0x03,
    .bNumConfigurations = 0x01,
};

static const uint8_t s_fs_config_desc[] = {
    TUD_CONFIG_DESCRIPTOR(1, ITF_NUM_TOTAL, 0, TUSB_DESC_TOTAL_LEN, TUSB_DESC_CONFIG_ATT_REMOTE_WAKEUP, 100),
    TUD_MSC_DESCRIPTOR(ITF_NUM_MSC, 0, EDPT_MSC_OUT, EDPT_MSC_IN, 64),
};

static const char *s_strings[] = {
    (const char[]) { 0x09, 0x04 }, /* 0: English (0x0409) */
    "RustMix Wave",                /* 1: manufacturer */
    "RustMix Wave microSD",        /* 2: product, the name the PC shows */
    "000001",                      /* 3: serial number */
};

/* --- Raw SD card ---
 *
 * TinyUSB wants the card initialized but NOT mounted: the PC owns the
 * filesystem. The same sequence as esp_vfs_fat_sdmmc_mount() minus the
 * final FAT mount. */
static esp_err_t init_card_raw(int clk, int cmd, int d0, int d1, int d2, int d3, sdmmc_card_t **out_card)
{
    sdmmc_host_t host = SDMMC_HOST_DEFAULT();
    host.max_freq_khz = USBDISK_SD_SPEED_KHZ;

    sdmmc_slot_config_t slot = SDMMC_SLOT_CONFIG_DEFAULT();
    slot.width = 4;
    slot.clk = clk;
    slot.cmd = cmd;
    slot.d0 = d0;
    slot.d1 = d1;
    slot.d2 = d2;
    slot.d3 = d3;
    slot.flags |= SDMMC_SLOT_FLAG_INTERNAL_PULLUP;

    sdmmc_card_t *card = malloc(sizeof(sdmmc_card_t));
    if (card == NULL) {
        return ESP_ERR_NO_MEM;
    }

    esp_err_t err = (*host.init)();
    if (err != ESP_OK) {
        free(card);
        return err;
    }

    err = sdmmc_host_init_slot(host.slot, &slot);
    if (err == ESP_OK) {
        /* One attempt. The example retries forever waiting for a card to be
         * inserted: here that would mean a stuck device and, with USB-OTG
         * about to take the PHY, no programming port either. Better to fail
         * and let the caller go back to normal mode. */
        err = sdmmc_card_init(&host, card);
    }
    if (err != ESP_OK) {
        if (host.flags & SDMMC_HOST_FLAG_DEINIT_ARG) {
            host.deinit_p(host.slot);
        } else {
            (*host.deinit)();
        }
        free(card);
        return err;
    }

    *out_card = card;
    return ESP_OK;
}

esp_err_t usbdisk_start(int clk, int cmd, int d0, int d1, int d2, int d3)
{
    sdmmc_card_t *card = NULL;
    ESP_RETURN_ON_ERROR(init_card_raw(clk, cmd, d0, d1, d2, d3, &card), TAG, "SD card init failed");

    /* Not mounted on the firmware side: the card belongs to the PC from the
     * start, and disk mode only ends by restarting. (The library still
     * remounts it after an eject -- see __wrap_f_mkfs above.) */
    const tinyusb_msc_sdmmc_config_t storage_cfg = {
        .card = card,
        .callback_mount_changed = NULL,
        .callback_premount_changed = NULL,
        .mount_config = {
            .format_if_mount_failed = false,
            .max_files = 5,
            .allocation_unit_size = 0,
        },
    };
    ESP_RETURN_ON_ERROR(tinyusb_msc_storage_init_sdmmc(&storage_cfg), TAG, "MSC storage not created");

    const tinyusb_config_t tusb_cfg = {
        .device_descriptor = &s_device_desc,
        .string_descriptor = s_strings,
        .string_descriptor_count = sizeof(s_strings) / sizeof(s_strings[0]),
        .external_phy = false,
        .configuration_descriptor = s_fs_config_desc,
    };

    /* From here the USB pins belong to USB-OTG: the serial port and the log
     * with it disappear. This is the last message anyone will see. */
    ESP_LOGW(TAG, "starting USB disk: the serial port disappears until the next restart");
    ESP_RETURN_ON_ERROR(tinyusb_driver_install(&tusb_cfg), TAG, "USB driver not installed");
    return ESP_OK;
}

void usbdisk_release_phy(void)
{
    /* Power-on value of both bits: 0 (soc/rtc_cntl_reg.h). With
     * sw_hw_usb_phy_sel at 0 the PHY is back under hardware control, which
     * gives it to the serial port: as far as USB goes, a cold boot. */
    RTCCNTL.usb_conf.sw_usb_phy_sel = 0;
    RTCCNTL.usb_conf.sw_hw_usb_phy_sel = 0;
}

void usbdisk_restart(void)
{
    usbdisk_release_phy();
    esp_restart();
}
