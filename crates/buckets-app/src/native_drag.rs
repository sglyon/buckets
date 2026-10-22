//! Export local files as native dragging items so Finder and other applications
//! receive real file URLs, rather than an application-private drag payload.
use anyhow::Result;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
mod mac {
    use anyhow::{Context, Result, ensure};
    use objc2::{
        AnyThread, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::ProtocolObject,
    };
    use objc2_app_kit::{
        NSApplication, NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession,
        NSDraggingSource, NSPasteboardWriting, NSView, NSWorkspace,
    };
    use objc2_foundation::{
        MainThreadMarker, NSArray, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
        NSURL,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::{cell::OnceCell, path::PathBuf};

    define_class!(
        // SAFETY: NSObject has no subclass requirements; all callbacks have the
        // declared AppKit signatures and run on the main application thread.
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        struct BucketsFileDragSource;
        unsafe impl NSObjectProtocol for BucketsFileDragSource {}
        unsafe impl NSDraggingSource for BucketsFileDragSource {
            #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
            fn operation(
                &self,
                _session: &NSDraggingSession,
                _context: NSDraggingContext,
            ) -> NSDragOperation {
                // Outgoing drags copy. Destructive moves use the explicit Move
                // command, with its collision checks and status reporting.
                NSDragOperation::Copy
            }
            #[unsafe(method(draggingSession:endedAtPoint:operation:))]
            fn ended(
                &self,
                _session: &NSDraggingSession,
                _point: NSPoint,
                operation: NSDragOperation,
            ) {
                if std::env::var_os("BUCKETS_TRACE_DRAG").is_some() {
                    eprintln!("Native file drag ended: {operation:?}");
                }
            }
        }
    );
    thread_local! { static SOURCE: OnceCell<Retained<BucketsFileDragSource>> = const { OnceCell::new() }; }

    pub fn start(window: &gpui::Window, paths: &[PathBuf]) -> Result<bool> {
        if paths.is_empty() {
            return Ok(false);
        }
        let mtm =
            MainThreadMarker::new().context("Native file dragging must run on the UI thread")?;
        let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window)
            .map_err(|e| anyhow::anyhow!("Native window handle unavailable: {e}"))?
            .as_raw()
        else {
            return Ok(false);
        };
        // SAFETY: The AppKit raw window handle guarantees ns_view is a live NSView
        // for this borrowed GPUI window. It is used synchronously on the UI thread.
        let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
        let Some(event) = NSApplication::sharedApplication(mtm).currentEvent() else {
            return Ok(false);
        };
        let position = view.convertPoint_fromView(event.locationInWindow(), None);
        let workspace = NSWorkspace::sharedWorkspace();
        let mut items = Vec::with_capacity(paths.len());
        for (i, path) in paths.iter().enumerate() {
            ensure!(
                path.is_absolute(),
                "Drag source must be an absolute file path"
            );
            let text = NSString::from_str(
                path.to_str()
                    .context("Native dragging requires a Unicode path")?,
            );
            let url = NSURL::fileURLWithPath(&text);
            let writer = ProtocolObject::<dyn NSPasteboardWriting>::from_ref(&*url);
            let item = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), writer);
            let image = workspace.iconForFile(&text);
            let offset = (i.min(4) * 3) as f64;
            let frame = NSRect::new(
                NSPoint::new(position.x + offset, position.y + offset),
                NSSize::new(32., 32.),
            );
            // SAFETY: AppKit accepts NSImage as the contents for a dragging frame.
            // Both image and item are retained during session creation.
            unsafe {
                item.setDraggingFrame_contents(frame, Some(image.as_ref()));
            }
            items.push(item);
        }
        SOURCE.with(|source| {
            let source = source.get_or_init(|| {
                // SAFETY: NSObject's init returns this retained subclass instance.
                unsafe { msg_send![BucketsFileDragSource::alloc(mtm), init] }
            });
            let session = view.beginDraggingSessionWithItems_event_source(
                &NSArray::from_retained_slice(&items),
                &event,
                ProtocolObject::from_ref(&**source),
            );
            session.setAnimatesToStartingPositionsOnCancelOrFail(true);
            // GPUI 0.2.2's macOS drop decoder still reads the legacy filenames
            // property list. Supply it alongside modern file URLs so an exported
            // drag can also be dropped back into another Buckets pane.
            let names = paths
                .iter()
                .map(|path| NSString::from_str(path.to_str().expect("validated Unicode path")))
                .collect::<Vec<_>>();
            let names = NSArray::from_retained_slice(&names);
            let legacy_type = NSString::from_str("NSFilenamesPboardType");
            let pasteboard = session.draggingPasteboard();
            // SAFETY: This declares a standard pasteboard type without an owner;
            // the property list is an NSArray containing only NSString paths.
            unsafe {
                pasteboard.addTypes_owner(&NSArray::from_slice(&[&*legacy_type]), None);
                if !pasteboard.setPropertyList_forType(names.as_ref(), &legacy_type) {
                    eprintln!("Could not add legacy filenames to native drag pasteboard");
                }
            }
        });
        Ok(true)
    }
}

#[cfg(target_os = "macos")]
pub fn start(window: &gpui::Window, paths: &[PathBuf]) -> Result<bool> {
    mac::start(window, paths)
}
#[cfg(not(target_os = "macos"))]
pub fn start(_window: &gpui::Window, _paths: &[PathBuf]) -> Result<bool> {
    Ok(false)
}
