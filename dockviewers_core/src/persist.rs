//! Layout persistence to `localStorage`. wasm-only: the pure model/api compile
//! everywhere, but touching `window` is gated so `cargo check` stays green natively.
//! Pairs with [`crate::model::serial`] for the actual encoding.

/// Read a saved layout JSON string under `key`, if present.
#[cfg(target_arch = "wasm32")]
pub fn read(key: &str) -> Option<String> {
	web_sys::window()?.local_storage().ok()??.get_item(key).ok()?
}

/// Persist a layout JSON string under `key`.
#[cfg(target_arch = "wasm32")]
pub fn write(key: &str, json: &str) {
	let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else {
		return;
	};
	// A full/blocked localStorage is non-fatal: the layout just isn't persisted this tick.
	let _ = storage.set_item(key, json);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn read(_key: &str) -> Option<String> {
	None
}

#[cfg(not(target_arch = "wasm32"))]
pub fn write(_key: &str, _json: &str) {}
