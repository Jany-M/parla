use parla_core::events::{Event, EventSink};
use parla_core::models::Cancel;
use parla_core::service::Service;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

pub struct AppState {
    pub svc: Arc<Service>,
    pub install_cancel: Mutex<Cancel>,
    pub import_cancel: Mutex<Arc<AtomicBool>>,
}

impl AppState {
    pub fn new(svc: Arc<Service>) -> Self {
        Self { svc, install_cancel: Mutex::new(Cancel::default()), import_cancel: Mutex::new(Arc::new(AtomicBool::new(false))) }
    }
}

/// Core events → every window (main + overlay) as "parla-event".
pub struct TauriSink(pub AppHandle);

impl EventSink for TauriSink {
    fn emit(&self, e: Event) {
        let _ = self.0.emit("parla-event", e);
    }
}
