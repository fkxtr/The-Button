fn main() {
    #[cfg(feature = "desktop")]
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "start_download",
            "confirm_download",
            "cancel_download",
        ]),
    ))
    .expect("Tauri build configuration failed");
}
