//! Dragging stem files out of the plugin window into the DAW.
//!
//! macOS: a real AppKit file drag (NSDraggingSession with file URLs), which every DAW accepts as
//! a dropped audio file. Elsewhere `start_file_drag` returns false and the editor falls back to
//! showing the files in the file manager.

use std::path::PathBuf;

#[cfg(target_os = "macos")]
pub fn start_file_drag(paths: &[PathBuf]) -> bool {
    mac::start(paths)
}

#[cfg(not(target_os = "macos"))]
pub fn start_file_drag(_paths: &[PathBuf]) -> bool {
    false
}

/// Show the file in Finder / Explorer.
pub fn reveal(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn();
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let dir = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject, AnyProtocol, ClassBuilder, Sel};
    use objc2::{msg_send, sel, ClassType};
    use objc2_foundation::{NSArray, NSObject, NSPoint, NSRect, NSSize, NSString};
    use std::ffi::CString;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    const NS_DRAG_OPERATION_COPY: usize = 1;

    extern "C-unwind" fn source_mask(_this: &AnyObject, _sel: Sel, _session: *mut AnyObject, _ctx: isize) -> usize {
        NS_DRAG_OPERATION_COPY
    }

    extern "C-unwind" fn ended(_this: &AnyObject, _sel: Sel, _session: *mut AnyObject, _pt: NSPoint, _op: usize) {
        END_FLAG.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub static END_FLAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// The drag source class. The name is unique per loaded binary, so the CLAP and the VST3 copy
    /// can both be loaded in one host without their classes clashing.
    fn source_class() -> &'static AnyClass {
        static CLASS: OnceLock<usize> = OnceLock::new();
        let ptr = *CLASS.get_or_init(|| {
            let name = CString::new(format!(
                "StemSplitDragSource_{:x}",
                &END_FLAG as *const _ as usize
            ))
            .unwrap();
            if let Some(c) = AnyClass::get(&name) {
                return c as *const AnyClass as usize;
            }
            let mut b = ClassBuilder::new(&name, NSObject::class()).expect("drag source class");
            if let Some(p) = AnyProtocol::get(c"NSDraggingSource") {
                b.add_protocol(p);
            }
            unsafe {
                b.add_method(
                    sel!(draggingSession:sourceOperationMaskForDraggingContext:),
                    source_mask as extern "C-unwind" fn(_, _, _, _) -> _,
                );
                b.add_method(
                    sel!(draggingSession:endedAtPoint:operation:),
                    ended as extern "C-unwind" fn(_, _, _, _, _),
                );
            }
            b.register() as *const AnyClass as usize
        });
        unsafe { &*(ptr as *const AnyClass) }
    }

    pub fn start(paths: &[PathBuf]) -> bool {
        if paths.is_empty() {
            return false;
        }
        unsafe {
            let Some(app_class) = AnyClass::get(c"NSApplication") else {
                return false;
            };
            let app: Option<Retained<AnyObject>> = msg_send![app_class, sharedApplication];
            let Some(app) = app else { return false };
            let event: Option<Retained<AnyObject>> = msg_send![&*app, currentEvent];
            let Some(event) = event else { return false };
            let window: Option<Retained<AnyObject>> = msg_send![&*event, window];
            let Some(window) = window else { return false };
            let content: Option<Retained<AnyObject>> = msg_send![&*window, contentView];
            let Some(content) = content else { return false };
            let loc: NSPoint = msg_send![&*event, locationInWindow];
            // The view under the mouse is baseview's NSView (the plugin editor).
            let hit: Option<Retained<AnyObject>> = msg_send![&*content, hitTest: loc];
            let view = hit.unwrap_or(content);
            let in_view: NSPoint = msg_send![&*view, convertPoint: loc, fromView: std::ptr::null::<AnyObject>()];

            let Some(url_class) = AnyClass::get(c"NSURL") else { return false };
            let Some(item_class) = AnyClass::get(c"NSDraggingItem") else { return false };
            let Some(ws_class) = AnyClass::get(c"NSWorkspace") else { return false };
            let ws: Retained<AnyObject> = msg_send![ws_class, sharedWorkspace];

            let mut items: Vec<Retained<AnyObject>> = Vec::new();
            for (i, p) in paths.iter().enumerate() {
                let s = NSString::from_str(&p.to_string_lossy());
                let url: Option<Retained<AnyObject>> = msg_send![url_class, fileURLWithPath: &*s];
                let Some(url) = url else { continue };
                let alloc: Allocated<AnyObject> = msg_send![item_class, alloc];
                let item: Option<Retained<AnyObject>> = msg_send![alloc, initWithPasteboardWriter: &*url];
                let Some(item) = item else { continue };
                let icon: Option<Retained<AnyObject>> = msg_send![&*ws, iconForFile: &*s];
                let off = i as f64 * 6.0;
                let frame = NSRect::new(
                    NSPoint::new(in_view.x - 16.0 + off, in_view.y - 16.0 - off),
                    NSSize::new(32.0, 32.0),
                );
                let contents: *const AnyObject = icon.as_deref().map_or(std::ptr::null(), |i| i as *const _);
                let _: () = msg_send![&*item, setDraggingFrame: frame, contents: contents];
                items.push(item);
            }
            if items.is_empty() {
                return false;
            }
            let refs: Vec<&AnyObject> = items.iter().map(|i| &**i).collect();
            let array = NSArray::from_slice(&refs);
            let source: Retained<AnyObject> = msg_send![source_class(), new];
            END_FLAG.store(false, std::sync::atomic::Ordering::Relaxed);
            let session: Option<Retained<AnyObject>> = msg_send![
                &*view,
                beginDraggingSessionWithItems: &*array,
                event: &*event,
                source: &*source
            ];
            // The session keeps its source alive for the duration of the drag.
            std::mem::forget(source);
            session.is_some()
        }
    }
}

/// True once the last drag finished (macOS); always true elsewhere.
pub fn drag_ended() -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::END_FLAG.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}
