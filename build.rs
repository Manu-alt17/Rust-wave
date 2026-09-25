fn main() {
    embuild::espidf::sysenv::output();
    // Set by esp-idf-sys only when the diagnostic PM-profiling overlay
    // (sdkconfig.defaults.pm-profiling) enables CONFIG_PM_PROFILING.
    println!("cargo:rustc-check-cfg=cfg(esp_idf_pm_profiling)");
}
