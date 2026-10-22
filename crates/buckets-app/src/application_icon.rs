//! Set the process icon even when launched as a bare development executable.
//! A bundle supplies Finder and switcher identity; this sets the live Dock image.
#[cfg(target_os = "macos")]
pub fn install() -> anyhow::Result<()> {
    use anyhow::Context;
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let main_thread =
        MainThreadMarker::new().context("Application icon requires the main thread")?;
    let bytes = NSData::with_bytes(include_bytes!("../assets/icon/Buckets.icns"));
    let image = NSImage::initWithData(NSImage::alloc(), &bytes)
        .context("Could not decode the bundled Buckets icon")?;
    let application = NSApplication::sharedApplication(main_thread);
    // SAFETY: We always supply a valid decoded image (never nil), on AppKit's
    // main thread. AppKit retains the image for the running application's icon.
    unsafe { application.setApplicationIconImage(Some(&image)) };
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn install() -> anyhow::Result<()> {
    Ok(())
}
