//! Native file URLs for Finder interoperability. GPUI's text clipboard remains
//! available for remote location URIs. Call from explicit copy/paste actions.
use anyhow::Result;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
pub fn write_paths(paths: &[PathBuf]) -> Result<()> {
    use anyhow::{Context, ensure};
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSPasteboard, NSPasteboardWriting};
    use objc2_foundation::{NSArray, NSString, NSURL};
    ensure!(!paths.is_empty(), "Select a file first");
    let urls = paths
        .iter()
        .map(|path| {
            let absolute = if path.is_absolute() {
                path.clone()
            } else {
                std::env::current_dir()?.join(path)
            };
            let text = absolute
                .to_str()
                .context("Finder clipboard requires a Unicode path")?;
            Ok(NSURL::fileURLWithPath(&NSString::from_str(text)))
        })
        .collect::<Result<Vec<_>>>()?;
    let objects = urls
        .iter()
        .map(|url| ProtocolObject::<dyn NSPasteboardWriting>::from_ref(&**url))
        .collect::<Vec<_>>();
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    ensure!(
        pasteboard.writeObjects(&NSArray::from_slice(&objects)),
        "Could not write file URLs to the macOS clipboard"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn read_paths() -> Result<Vec<PathBuf>> {
    use objc2::ClassType;
    use objc2_app_kit::NSPasteboard;
    use objc2_foundation::{NSArray, NSURL};
    let classes = NSArray::from_slice(&[NSURL::class()]);
    // SAFETY: NSArray contains only NSURL's class, and no untyped options are
    // supplied. Every returned object is downcast before NSURL methods are used.
    let objects =
        unsafe { NSPasteboard::generalPasteboard().readObjectsForClasses_options(&classes, None) };
    let mut paths = Vec::new();
    if let Some(objects) = objects {
        for object in objects.iter() {
            if let Some(url) = object.downcast_ref::<NSURL>()
                && url.isFileURL()
                && let Some(path) = url.path()
            {
                paths.push(PathBuf::from(path.to_string()));
            }
        }
    }
    Ok(paths)
}

#[cfg(not(target_os = "macos"))]
pub fn write_paths(_paths: &[PathBuf]) -> Result<()> {
    Ok(())
}
#[cfg(not(target_os = "macos"))]
pub fn read_paths() -> Result<Vec<PathBuf>> {
    Ok(Vec::new())
}
