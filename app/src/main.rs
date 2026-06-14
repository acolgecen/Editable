#![deny(unsafe_op_in_unsafe_fn)]

//! Editable — a fast macOS editor for large CSV files.
//!
//! The crate is split into three layers:
//! - [`state`] owns the open document, selection, undo history, and statistics.
//! - [`selection`] is the pure selection model shared by `state` and `ui`.
//! - [`ui`] is the AppKit front end; one `ui::Delegate` backs each window.

mod selection;
mod state;
mod ui;

use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::MainThreadMarker;
use ui::Delegate;

fn main() {
    let mtm = MainThreadMarker::new().expect("Editable must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let delegate = Delegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
}
