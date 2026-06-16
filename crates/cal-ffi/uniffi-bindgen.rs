//! Standalone entry point for UniFFI's binding generator, version-locked to the
//! `uniffi` runtime this crate links against (Mozilla's recommended setup). See
//! the `[[bin]]` note in `Cargo.toml` for invocation.

fn main() {
    uniffi::uniffi_bindgen_main()
}
