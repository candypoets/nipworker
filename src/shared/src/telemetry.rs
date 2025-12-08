use std::cell::RefCell;
use std::sync::Once;
use tracing::{Level, Span};
use wasm_bindgen::prelude::*;

// Global one-time initialization for the logging backend
static INIT: Once = Once::new();

// Keep the entered guard in thread-local storage (no Sync needed)
thread_local! {
    static COMPONENT_GUARD: RefCell<Option<tracing::span::Entered<'static>>> = RefCell::new(None);
}

#[wasm_bindgen]
pub fn init_tracing(level: String) {
    let lvl = parse_level(&level);
    init(lvl);
}

pub fn init(level: Level) {
    INIT.call_once(|| {
        console_error_panic_hook::set_once();

        let mut builder = tracing_wasm::WASMLayerConfigBuilder::new();
        builder
            .set_max_level(level)
            .set_console_config(tracing_wasm::ConsoleConfig::ReportWithConsoleColor)
            .set_report_logs_in_timings(true);

        let config = builder.build();
        tracing_wasm::set_as_global_default_with_config(config);

        tracing::info!("Tracing initialized at level = {}", level);
    });
}

// Same as `init`, but also enters a permanent span with `crate = component`
// so every event shows the crate label via span context in the console.
pub fn init_with_component(level: Level, component: &str) {
    INIT.call_once(|| {
        console_error_panic_hook::set_once();

        let mut builder = tracing_wasm::WASMLayerConfigBuilder::new();
        builder
            .set_max_level(level)
            .set_console_config(tracing_wasm::ConsoleConfig::ReportWithConsoleColor)
            .set_report_logs_in_timings(true);
        let config = builder.build();
        tracing_wasm::set_as_global_default_with_config(config);

        // Create a span and keep it entered via a thread-local guard.
        // We leak the span once per worker to get a 'static ref; acceptable in this context.
        let span = tracing::info_span!("component", crate = component);
        let span_static: &'static Span = Box::leak(Box::new(span));
        COMPONENT_GUARD.with(|cell| {
            let guard = span_static.enter();
            *cell.borrow_mut() = Some(guard);
        });

        tracing::info!(
            "Tracing initialized at level = {} (component={})",
            level,
            component
        );
    });
}

fn parse_level(s: &str) -> Level {
    match s.to_ascii_lowercase().as_str() {
        "error" => Level::ERROR,
        "warn" | "warning" => Level::WARN,
        "info" => Level::INFO,
        "debug" => Level::DEBUG,
        "trace" => Level::TRACE,
        _ => Level::INFO,
    }
}
