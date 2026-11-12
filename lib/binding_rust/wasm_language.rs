use std::{
    error,
    ffi::{CStr, CString},
    fmt,
    mem::{self, MaybeUninit},
    os::raw::c_char,
};

pub use wasmtime_c_api::wasmtime;

use crate::{Language, LanguageError, Parser, ffi, ts_free};

// Force Cargo to include wasmtime-c-api as a dependency of this crate,
// even though it is only used by the C code.
#[expect(unused, reason = "forces Cargo to link wasmtime-c-api")]
fn use_wasmtime() {
    wasmtime_c_api::wasm_engine_new();
}

#[repr(C)]
#[derive(Clone)]
#[allow(non_camel_case_types)]
pub struct wasm_engine_t {
    pub(crate) engine: wasmtime::Engine,
}

pub struct WasmStore(*mut ffi::TSWasmStore);

unsafe impl Send for WasmStore {}
unsafe impl Sync for WasmStore {}

#[derive(Debug, PartialEq, Eq)]
pub struct WasmError {
    pub kind: WasmErrorKind,
    pub message: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WasmErrorKind {
    Parse,
    Compile,
    Instantiate,
    /// EXTENSION: resource-policy installation or enforcement failure.
    Resource,
    Other,
}

/// EXTENSION: construction options for [`WasmStore::new_with_options`].
///
/// `fuel` is installed on the store before any guest code runs (the engine
/// must have fuel consumption enabled, or construction fails with
/// [`WasmErrorKind::Resource`]). `memory_limit_bytes` is installed as the
/// store's resource limiter before its memory is created; values above the
/// integration's 128 MiB hard maximum or below its construction floor fail
/// construction, never silently clamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WasmStoreOptions {
    pub fuel: u64,
    pub memory_limit_bytes: u64,
}

impl WasmStore {
    pub fn new(engine: &wasmtime::Engine) -> Result<Self, WasmError> {
        unsafe {
            let mut error = MaybeUninit::<ffi::TSWasmError>::uninit();
            let store = ffi::ts_wasm_store_new(
                std::ptr::from_ref::<wasmtime::Engine>(engine)
                    .cast_mut()
                    .cast(),
                error.as_mut_ptr(),
            );
            if store.is_null() {
                Err(WasmError::new(error.assume_init()))
            } else {
                Ok(Self(store))
            }
        }
    }

    /// EXTENSION: create a Wasm store with an explicit resource policy
    /// installed on the integration's actual store before memory creation
    /// and before any guest execution.
    pub fn new_with_options(
        engine: &wasmtime::Engine,
        options: WasmStoreOptions,
    ) -> Result<Self, WasmError> {
        unsafe {
            let mut error = MaybeUninit::<ffi::TSWasmError>::uninit();
            let c_options = ffi::TSWasmStoreOptions {
                fuel: options.fuel,
                memory_limit_bytes: options.memory_limit_bytes,
            };
            let store = ffi::ts_wasm_store_new_with_options(
                std::ptr::from_ref::<wasmtime::Engine>(engine)
                    .cast_mut()
                    .cast(),
                &raw const c_options,
                error.as_mut_ptr(),
            );
            if store.is_null() {
                Err(WasmError::new(error.assume_init()))
            } else {
                Ok(Self(store))
            }
        }
    }

    /// EXTENSION: the store's remaining fuel, `None` when the engine
    /// has no fuel consumption enabled.
    #[must_use]
    pub fn fuel_remaining(&self) -> Option<u64> {
        let mut fuel = 0_u64;
        unsafe { ffi::ts_wasm_store_fuel_remaining(self.0, &raw mut fuel).then_some(fuel) }
    }

    pub fn load_language(&mut self, name: &str, bytes: &[u8]) -> Result<Language, WasmError> {
        let name = CString::new(name).unwrap();
        unsafe {
            let mut error = MaybeUninit::<ffi::TSWasmError>::uninit();
            let language = ffi::ts_wasm_store_load_language(
                self.0,
                name.as_ptr(),
                bytes.as_ptr().cast::<c_char>(),
                bytes.len() as u32,
                error.as_mut_ptr(),
            );
            if language.is_null() {
                Err(WasmError::new(error.assume_init()))
            } else {
                Ok(Language(language))
            }
        }
    }

    #[must_use]
    pub fn language_count(&self) -> usize {
        unsafe { ffi::ts_wasm_store_language_count(self.0) }
    }
}

impl WasmError {
    unsafe fn new(error: ffi::TSWasmError) -> Self {
        let message = unsafe { CStr::from_ptr(error.message) }
            .to_str()
            .unwrap()
            .to_string();
        unsafe { ts_free(error.message.cast()) };
        Self {
            kind: match error.kind {
                ffi::TSWasmErrorKindParse => WasmErrorKind::Parse,
                ffi::TSWasmErrorKindCompile => WasmErrorKind::Compile,
                ffi::TSWasmErrorKindInstantiate => WasmErrorKind::Instantiate,
                ffi::TSWasmErrorKindResource => WasmErrorKind::Resource,
                _ => WasmErrorKind::Other,
            },
            message,
        }
    }
}

impl Language {
    #[must_use]
    pub fn is_wasm(&self) -> bool {
        unsafe { ffi::ts_language_is_wasm(self.0) }
    }
}

impl Parser {
    pub fn set_wasm_store(&mut self, store: WasmStore) -> Result<(), LanguageError> {
        unsafe { ffi::ts_parser_set_wasm_store(self.0.as_ptr(), store.0) };
        mem::forget(store);
        Ok(())
    }

    pub fn take_wasm_store(&mut self) -> Option<WasmStore> {
        let ptr = unsafe { ffi::ts_parser_take_wasm_store(self.0.as_ptr()) };
        if ptr.is_null() {
            None
        } else {
            Some(WasmStore(ptr))
        }
    }
}

impl Drop for WasmStore {
    fn drop(&mut self) {
        unsafe { ffi::ts_wasm_store_delete(self.0) };
    }
}

impl fmt::Display for WasmError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let kind = match self.kind {
            WasmErrorKind::Parse => "Failed to parse Wasm",
            WasmErrorKind::Compile => "Failed to compile Wasm",
            WasmErrorKind::Instantiate => "Failed to instantiate Wasm module",
            WasmErrorKind::Resource => "Failed to install the Wasm resource policy",
            WasmErrorKind::Other => "Unknown error",
        };
        write!(f, "{kind}: {}", self.message)
    }
}

impl error::Error for WasmError {}
